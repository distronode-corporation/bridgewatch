//! The IPC surface. Everything the two windows can ask the shell to do.
//!
//! The rule the whole packet is built on: **no business logic lives here or in
//! TypeScript**. A command either hands the frontend something the core
//! produced, or hands the core something the user typed. The verdict engine,
//! the validator and the TOML editor are all the core's.

use std::sync::Arc;

use bridgewatch_core::config::edit::{ConfigEditor, Edit, EditValue};
use bridgewatch_core::poll::PollNow;
use bridgewatch_core::token::{self, Secret, SystemTokenProvider};
use bridgewatch_core::verdict::Snapshot;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::config::Validation;
use crate::state::{AppState, Status};

type Shared<'a> = State<'a, Arc<AppState>>;

/// The latest snapshot, for a window's first paint.
///
/// Every later frame arrives on the `snapshot` event; this exists so a window
/// that opens between ticks is not blank for up to a minute.
#[tauri::command]
pub fn get_snapshot(state: Shared<'_>) -> Snapshot {
    state.snapshot()
}

/// Shell-level status: the config path, its diagnostics, poll timings.
#[tauri::command]
pub fn get_status(app: AppHandle, state: Shared<'_>) -> Status {
    state.status(app.autolaunch().is_enabled().unwrap_or(false))
}

/// Poll now. Returns false when the request was dropped by the rate gate.
#[tauri::command]
pub fn refresh_now(state: Shared<'_>) -> bool {
    state.request_poll(PollNow::Manual)
}

/// Called when the popover becomes visible. Returns whether a poll was started.
#[tauri::command]
pub fn popover_opened(state: Shared<'_>) -> bool {
    state.request_poll(PollNow::PopoverOpened)
}

/// The raw text of the config file, for the "Edit as text" tab.
#[tauri::command]
///
/// A missing file reads as empty (a first launch whose wizard was skipped),
/// which is also what the compare-and-swap treats it as, so the text tab's
/// Save creates it.
pub fn read_config_text(state: Shared<'_>) -> Result<String, String> {
    read_or_empty(&state.config_path).map_err(|e| e.to_string())
}

/// Validate candidate text without writing anything.
#[tauri::command]
pub fn validate_config_text(text: String, state: Shared<'_>) -> Validation {
    Validation::of(&text, &state.config_path)
}

/// Write candidate text, but only if it validates, only if the file still
/// holds `base`, and only after confirmation when it makes a sensitive change.
///
/// ⛔ The order matters and is the whole point: parse first, write second. A
/// settings pane that writes and then discovers the file is broken has taken
/// the user's monitoring down to tell them about a typo.
///
/// `base` is the text the caller's copy was read from. A file that has changed
/// since is not overwritten; the answer carries `conflict: true` and the
/// window decides what to do (the text tab asks). `confirm` is the id of a
/// confirmation this shell issued for exactly this text; see `guard`.
#[tauri::command]
pub fn save_config_text(
    text: String,
    base: String,
    confirm: Option<String>,
    app: AppHandle,
    state: Shared<'_>,
) -> Validation {
    commit(&app, &state, &base, text, confirm.as_deref())
}

/// Apply a batch of typed edits through the core's format-preserving editor.
///
/// This is what every form control in the settings pane writes with, rather
/// than serialising a whole `Config` back out: `toml_edit` keeps the comments,
/// the key order and the `[[watches]]` order, so editing one poll interval does
/// not reflow somebody's annotated file.
///
/// ⚠ A `ui.launch_at_login` edit also registers or unregisters the OS login
/// item, and the file then records what the OS did. The key used to be written
/// and read by nothing, so the checkbox said "on" while the OS said off.
#[tauri::command]
pub fn apply_config_edits(
    edits: Vec<Edit>,
    confirm: Option<String>,
    app: AppHandle,
    state: Shared<'_>,
) -> Validation {
    let raw = match read_or_empty(&state.config_path) {
        Ok(raw) => raw,
        Err(e) => return Validation::error(format!("could not read the configuration: {e}")),
    };

    let mut editor = match ConfigEditor::new(&raw) {
        Ok(editor) => editor,
        Err(e) => return Validation::error(e.to_string()),
    };
    if let Err(e) = editor.apply(&edits) {
        return Validation::error(e.to_string());
    }

    let mut result = commit(&app, &state, &raw, editor.to_toml(), confirm.as_deref());
    if result.ok
        && let Some(wanted) = launch_at_login_edit(&edits)
        && let Err(e) = set_login_item(&app, wanted)
    {
        result.ok = false;
        result.diagnostics.push(crate::config::DiagnosticView {
            severity: "error".into(),
            path: "ui.launch_at_login".into(),
            message: format!("the file was saved, but the login item could not be changed: {e}"),
            line: None,
            col: None,
        });
    }
    result
}

/// The value a batch of edits gives `ui.launch_at_login`, if it touches it.
/// The last edit wins, as it does in the editor; unsetting it means the
/// default, which is off.
fn launch_at_login_edit(edits: &[Edit]) -> Option<bool> {
    edits.iter().rev().find_map(|e| match e {
        Edit::Set {
            path,
            value: EditValue::Boolean(b),
        } if path == "ui.launch_at_login" => Some(*b),
        Edit::Unset { path } if path == "ui.launch_at_login" => Some(false),
        _ => None,
    })
}

/// The single write path for both windows: validate, guard, compare-and-swap,
/// reload.
fn commit(
    app: &AppHandle,
    state: &AppState,
    base: &str,
    text: String,
    confirm: Option<&str>,
) -> Validation {
    let path = &state.config_path;
    let validation = match admit(
        path,
        base,
        &text,
        state.config().as_ref(),
        &state.confirmations,
        confirm,
    ) {
        Ok(v) => v,
        Err(refused) => return refused,
    };
    write_admitted(app, state, base, &text, validation)
}

/// The second half of [`commit`]: compare-and-swap an ADMITTED text into the
/// file and reload. Split out for the wizard, which has a credential to store
/// between admitting its text and writing it.
pub(crate) fn write_admitted(
    app: &AppHandle,
    state: &AppState,
    base: &str,
    text: &str,
    validation: Validation,
) -> Validation {
    let path = &state.config_path;
    match crate::config::write_if_unchanged(path, base, text) {
        Ok(()) => {}
        Err(crate::config::WriteError::Conflict) => {
            let mut refused = Validation::error(format!(
                "{} changed on disk since this window read it; nothing was written",
                path.display()
            ));
            refused.conflict = true;
            return refused;
        }
        Err(crate::config::WriteError::Io(e)) => {
            return Validation::error(format!("could not write {}: {e}", path.display()));
        }
    }
    reload_from_disk(app, false);
    validation
}

/// Everything a write must pass before it touches the disk: it loads, and
/// any sensitive change in it has been confirmed. `Ok` carries the validation
/// to answer with; `Err` is the refusal.
///
/// ⛔ Every route to the file (a form edit, "Edit as text", a move, the
/// wizard) goes through this, which is what makes the command-source and
/// host-change confirmation binding rather than advisory (Lo13, M11).
pub(crate) fn admit(
    path: &std::path::Path,
    base: &str,
    text: &str,
    running: Option<&bridgewatch_core::config::Config>,
    confirmations: &crate::guard::Confirmations,
    confirm: Option<&str>,
) -> Result<Validation, Validation> {
    let validation = Validation::of(text, path);
    if !validation.ok {
        return Err(validation);
    }
    let Ok(candidate) = bridgewatch_core::config::parse_str(text, path) else {
        return Err(validation);
    };
    // What the write changes is measured against the file it replaces, or,
    // when that does not load, against what is running.
    let before = bridgewatch_core::config::parse_str(base, path)
        .ok()
        .map(|l| l.config)
        .or_else(|| running.cloned());
    let changes = crate::guard::sensitive_changes(before.as_ref(), &candidate.config);
    if !changes.is_empty() && !confirmations.redeem(confirm, text) {
        return Err(Validation {
            ok: false,
            confirm: Some(confirmations.issue(text, changes)),
            ..validation
        });
    }
    Ok(validation)
}

/// The file's text, with a missing file reading as empty.
pub(crate) fn read_or_empty(path: &std::path::Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        other => other,
    }
}

/// The event the settings window listens to, so a copy it holds of the file
/// is never older than the file without it knowing.
pub const CONFIG_CHANGED_EVENT: &str = "config-changed";

/// Re-read the file from disk and restart the poller if it is usable.
///
/// ⚠ A failed reload deliberately leaves the RUNNING configuration alone. The
/// diagnostics are published, the popover shows them in its error strip, and
/// the poller keeps using the last good config — because the alternative is
/// that a typo saved at 3pm silently stops the monitoring until somebody
/// notices the tray has gone grey.
///
/// ⚠ Text that is byte-identical to the last text seen is skipped unless
/// `force` is set. Every GUI save used to rebuild the poller TWICE, once from
/// the save and once from the file watcher seeing the save land, and each
/// rebuild re-resolves every token (a `command` source runs again, a Keychain
/// prompt can appear again) and drops every cache. "Reload config" forces,
/// since a user asking for it wants the rebuild even for an unchanged file,
/// e.g. after fixing a credential.
pub fn reload_from_disk(app: &AppHandle, force: bool) {
    let state = app.state::<Arc<AppState>>();
    let path = state.config_path.clone();
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    if !state.note_disk_text(&raw, force) {
        return;
    }
    // ONE read, parsed once: validating one read and loading another let a
    // write landing in between publish diagnostics for text that never ran.
    let validation = Validation::of(&raw, &path);
    let loaded = bridgewatch_core::config::parse_str(&raw, &path).ok();

    let changed = {
        let mut inner = state.lock();
        inner.validation = validation;
        match loaded {
            Some(l) => {
                inner.config = Some(l.config);
                true
            }
            None => false,
        }
    };

    if let Some(config) = state.config() {
        crate::logging::apply_config_level(&config.log.level);
    }
    let _ = app.emit(CONFIG_CHANGED_EVENT, ());

    if changed {
        state.request_reload();
    } else {
        // Nothing to restart, but the popover's error strip has to change.
        crate::poller::republish_errors(app);
    }
}

/// The configuration as JSON, with every default filled in, for the settings
/// form to read its current values from.
///
/// ⚠ `job_order` is not redundant. `serde_json` orders a map's keys
/// alphabetically, and `[watches.jobs]` is **first match wins**, so the JSON
/// alone would tell the settings pane that `kics-iac-sast` comes before
/// `re:^verify:web_coverage_full` when the file says the opposite. The core
/// goes to some trouble to recover that order from the document; throwing it
/// away one layer later would apply the wrong override.
#[derive(serde::Serialize)]
pub struct ConfigJson {
    /// `Config`, serialised.
    pub config: serde_json::Value,
    /// Per watch, in configuration order, the job-override patterns in FILE
    /// order.
    pub job_order: Vec<Vec<String>>,
}

/// The current configuration, or `None` when nothing has ever loaded.
///
/// ⚠ `ui.launch_at_login` is reported as the OS has it, not as the file does:
/// the file is a mirror the OS can disagree with (the login item removed in
/// System Settings, or a config copied from another machine), and the checkbox
/// that reads it has to show what will actually happen at login.
#[tauri::command]
pub fn get_config_json(app: AppHandle, state: Shared<'_>) -> Option<ConfigJson> {
    let mut config = state.config()?;
    if let Ok(enabled) = app.autolaunch().is_enabled() {
        config.ui.launch_at_login = enabled;
    }
    let job_order = config
        .watches
        .iter()
        .map(|w| w.jobs.entries().iter().map(|(k, _)| k.clone()).collect())
        .collect();
    Some(ConfigJson {
        config: serde_json::to_value(&config).unwrap_or(serde_json::Value::Null),
        job_order,
    })
}

/// The largest stylesheet `read_theme_css` returns.
pub const THEME_CSS_MAX_BYTES: u64 = 256 * 1024;

/// The contents of `[ui].theme_css`, if it is set and readable.
///
/// ⛔ Returned as TEXT rather than as a path for the webview to load. A
/// stylesheet reached with `convertFileSrc` needs Tauri's `asset:` protocol
/// enabled and scoped, which is a filesystem read granted to the webview for
/// the sake of one optional file. Reading it here costs a command and lets the
/// CSP stay free of `asset:` entirely.
#[tauri::command]
pub fn read_theme_css(state: Shared<'_>) -> Option<String> {
    let config = state.config()?;
    let path = config.ui.theme_css.trim();
    if path.is_empty() {
        return None;
    }
    let path = bridgewatch_core::config::expand_tilde(std::path::Path::new(path));
    match read_stylesheet(&path) {
        Ok(css) => Some(css),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not read [ui].theme_css");
            None
        }
    }
}

/// Read a stylesheet, and nothing that is not one.
///
/// ⛔ `theme_css` is a path the webview can set (`apply_config_edits`) and this
/// command returns the file's CONTENTS to the webview, so without a limit the
/// pair is "read any file the user can read": point it at `~/.ssh/id_ed25519`,
/// ask for the theme. Requiring a `.css` name and capping the size keeps the
/// feature and removes the file reader.
fn read_stylesheet(path: &std::path::Path) -> Result<String, String> {
    let is_css = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("css"));
    if !is_css {
        return Err("only a file ending in .css is read as a theme".into());
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > THEME_CSS_MAX_BYTES {
        return Err(format!(
            "larger than {} KiB; not a stylesheet this app will load",
            THEME_CSS_MAX_BYTES / 1024
        ));
    }
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

/// Move a watch up or down in `[[watches]]`. See `reorder` for why this is
/// not a core `Edit` yet, and for what the first version got wrong.
#[tauri::command]
pub fn move_watch(id: String, delta: i32, app: AppHandle, state: Shared<'_>) -> Validation {
    let raw = match std::fs::read_to_string(&state.config_path) {
        Ok(raw) => raw,
        Err(e) => return Validation::error(format!("could not read the configuration: {e}")),
    };
    match crate::reorder::move_watch(&raw, &id, delta) {
        Ok(Some(text)) => commit(&app, &state, &raw, text, None),
        Ok(None) => Validation {
            ok: true,
            ..Validation::default()
        },
        Err(e) => Validation::error(e),
    }
}

/// Reload the configuration from disk. The menu item and the settings pane
/// both call this.
#[tauri::command]
pub fn reload_config(app: AppHandle) {
    reload_from_disk(&app, true);
}

/// Open the config file in whatever the OS thinks owns `.toml`.
#[tauri::command]
pub fn open_config_file(app: AppHandle, state: Shared<'_>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(state.config_path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open a link in the browser, if it points at a configured account's host.
/// The popover's only way to open anything; see `links`.
#[tauri::command]
pub fn open_link(url: String, app: AppHandle, state: Shared<'_>) -> Result<(), String> {
    crate::links::open(&app, &url, state.config().as_ref())
}

/// The pipelines index for the primary watch, if one can be derived.
#[tauri::command]
pub fn pipelines_url(app: AppHandle) -> Option<String> {
    crate::tray::pipelines_url(&app)
}

/// Show the settings window, optionally on a tab.
#[tauri::command]
pub fn open_settings(app: AppHandle, tab: Option<String>) {
    crate::windows::show_settings(&app, tab.as_deref());
}

/// Take the tab the menu asked settings to open on, clearing it.
#[tauri::command]
pub fn take_settings_tab(state: Shared<'_>) -> Option<String> {
    state.lock().settings_tab.take()
}

/// Size the popover to its content, clamped by `[ui].popover`.
///
/// ⚠ The height has to come from the FRONTEND. `max_height` is a ceiling, not a
/// height — a popover showing one green pipeline should not be 720px of empty
/// space — and only the webview knows how tall the content actually is. The
/// width is taken from the configuration rather than from the caller so a
/// layout bug cannot resize the window to something unusable.
#[tauri::command]
pub fn resize_popover(height: f64, app: AppHandle, state: Shared<'_>) {
    let Some(window) = crate::windows::popover(&app) else {
        return;
    };
    let (width, max) = state
        .config()
        .map(|c| (c.ui.popover.width as f64, c.ui.popover.max_height as f64))
        .unwrap_or((440.0, 720.0));
    let clamped = height.clamp(80.0, max.max(80.0));
    let _ = window.set_size(tauri::LogicalSize::new(width, clamped));
}

/// Hide the popover. The frontend calls this on Escape and after a click that
/// opened a browser.
#[tauri::command]
pub fn hide_popover(app: AppHandle) {
    crate::windows::hide_popover(&app);
}

/// Quit.
#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

/// Write bridgewatch's own credential-store entry for an account.
///
/// ⛔ The token is passed through to the core and never stored in the config
/// file, never logged, and never echoed back: `token = { own = true }` is how
/// it is read again. The frontend sends it once and forgets it.
#[tauri::command]
pub fn set_own_token(account: String, token: String, state: Shared<'_>) -> Result<(), String> {
    if token.trim().is_empty() {
        return Err("the token is empty".into());
    }
    token::set_own_token(&account, &Secret::new(token.trim()), &SystemTokenProvider)
        .map_err(|e| e.to_string())?;
    state.request_reload();
    Ok(())
}

/// Remove bridgewatch's own credential-store entry for an account.
#[tauri::command]
pub fn clear_own_token(account: String) -> Result<(), String> {
    token::clear_own_token(&account, &SystemTokenProvider).map_err(|e| e.to_string())
}

/// Read the OS's login-item state.
#[tauri::command]
pub fn get_launch_at_login(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Register or unregister the login item, and record the intent in the file.
#[tauri::command]
pub fn set_launch_at_login(enabled: bool, app: AppHandle) -> Result<bool, String> {
    set_login_item(&app, enabled)
}

/// Change the OS login item, mirror what the OS then reports into the file,
/// and put the tray's checkbox right. Every route to the setting ends here:
/// the tray menu, the command, and a form edit of `ui.launch_at_login`.
pub fn set_login_item(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    // Report what the OS says, not what was asked for, and record it even on
    // failure so the file does not keep claiming the request succeeded.
    let actual =
        app.autolaunch()
            .is_enabled()
            .unwrap_or(if result.is_ok() { enabled } else { !enabled });
    set_launch_at_login_in_config(app, actual);
    crate::tray::sync_autostart_check(app);
    result.map_err(|e| e.to_string())?;
    Ok(actual)
}

/// Mirror the login-item state into `[ui].launch_at_login`.
///
/// Best effort on purpose: the OS registration is the thing that actually
/// happens at login, and failing to record it in the file is not a reason to
/// refuse the change.
pub fn set_launch_at_login_in_config(app: &AppHandle, enabled: bool) {
    let state = app.state::<Arc<AppState>>();
    let path = state.config_path.clone();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut editor) = ConfigEditor::new(&raw) else {
        return;
    };
    if editor
        .apply(&[Edit::Set {
            path: "ui.launch_at_login".into(),
            value: EditValue::Boolean(enabled),
        }])
        .is_err()
    {
        return;
    }
    let toml = editor.to_toml();
    if toml != raw && bridgewatch_core::config::parse_str(&toml, &path).is_ok() {
        // Compare-and-swap like every other write: losing this mirror to a
        // concurrent edit is harmless, overwriting that edit is not.
        let _ = crate::config::write_if_unchanged(&path, &raw, &toml);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Confirmations;
    use std::path::Path;

    const OWN: &str = "[accounts.gl]\nbase_url = \"https://gitlab.com\"\ntoken = { own = true }\n";
    const CMD: &str = "[accounts.gl]\nbase_url = \"https://gitlab.com\"\ntoken = { command = [\"sh\", \"-c\", \"x\"] }\n";

    #[test]
    fn a_command_source_typed_as_text_is_parked_until_confirmed() {
        // M11 / Lo13: "Edit as text" used to write a `command` source in one
        // call; the confirmation lived only in the token form.
        let c = Confirmations::default();
        let p = Path::new("x.toml");
        let refused = admit(p, OWN, CMD, None, &c, None).expect_err("written unconfirmed");
        let request = refused.confirm.expect("a confirmation request");
        assert!(
            request.changes[0].contains("`sh -c x`"),
            "{:?}",
            request.changes
        );

        // The id releases exactly that text, once.
        assert!(admit(p, OWN, CMD, None, &c, Some(&request.id)).is_ok());
        assert!(admit(p, OWN, CMD, None, &c, Some(&request.id)).is_err());
    }

    #[test]
    fn an_ordinary_edit_needs_no_confirmation_and_a_bad_one_is_refused() {
        let c = Confirmations::default();
        let p = Path::new("x.toml");
        let edited = OWN.replace("gitlab.com\"", "gitlab.com/\"");
        assert!(admit(p, OWN, &edited, None, &c, None).is_ok());
        let broken = admit(p, OWN, "[accounts.gl]\nbase_url = \n", None, &c, None)
            .expect_err("a broken file was admitted");
        assert!(broken.confirm.is_none());
    }

    #[test]
    fn a_form_edit_of_launch_at_login_reaches_the_os() {
        // H13: the key was written and read by nothing.
        let set = |b| Edit::Set {
            path: "ui.launch_at_login".into(),
            value: EditValue::Boolean(b),
        };
        assert_eq!(launch_at_login_edit(&[set(true)]), Some(true));
        assert_eq!(launch_at_login_edit(&[set(true), set(false)]), Some(false));
        assert_eq!(
            launch_at_login_edit(&[Edit::Unset {
                path: "ui.launch_at_login".into()
            }]),
            Some(false)
        );
        assert_eq!(
            launch_at_login_edit(&[Edit::Set {
                path: "ui.theme_css".into(),
                value: EditValue::String(String::new()),
            }]),
            None
        );
    }

    #[test]
    fn theme_css_reads_a_stylesheet_and_nothing_else() {
        // M11: `theme_css` is settable from the webview and its CONTENTS come
        // back to the webview, so it must not be a general file reader.
        let dir = std::env::temp_dir().join(format!("bw-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let css = dir.join("t.css");
        std::fs::write(&css, ":root{}").unwrap();
        assert_eq!(read_stylesheet(&css).unwrap(), ":root{}");
        let key = dir.join("id_ed25519");
        std::fs::write(&key, "secret").unwrap();
        assert!(read_stylesheet(&key).is_err());
        let big = dir.join("big.css");
        std::fs::write(&big, vec![b' '; THEME_CSS_MAX_BYTES as usize + 1]).unwrap();
        assert!(read_stylesheet(&big).is_err());
        assert!(read_stylesheet(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
