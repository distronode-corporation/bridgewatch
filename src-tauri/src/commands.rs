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
    let crash = crate::crash::global();
    state.status(
        app.autolaunch().is_enabled().unwrap_or(false),
        (crash.has_address(), crash.is_sending()),
    )
}

/// Poll now. Returns false when the request was dropped by the rate gate.
#[tauri::command]
pub fn refresh_now(state: Shared<'_>) -> bool {
    state.request_poll(PollNow::Manual)
}

/// End a "Pause notifications" (the popover's Resume button).
#[tauri::command]
pub fn resume_notifications(app: AppHandle, state: Shared<'_>) {
    state.with_quiet(|q| q.resume());
    crate::tray::sync_pause(&app);
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
///
/// ⛔ Async, with the work on a blocking thread, like every write: a write
/// that moves an account reads and writes the credential store first
/// (`store_admitted`), and a synchronous command runs on the main thread, so
/// a Keychain prompt would freeze the tray and both windows until answered.
#[tauri::command]
pub async fn save_config_text(
    text: String,
    base: String,
    confirm: Option<String>,
    app: AppHandle,
) -> Validation {
    off_main(app, move |app, state| {
        commit(app, state, &base, text, confirm.as_deref())
    })
    .await
}

/// Run a write (or a reload) on a blocking thread, never on the main one: see
/// [`save_config_text`].
async fn off_main(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &AppState) -> Validation + Send + 'static,
) -> Validation {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Arc<AppState>>().inner().clone();
        work(&app, &state)
    })
    .await
    .unwrap_or_else(|e| Validation::error(format!("the write did not finish: {e}")))
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
pub async fn apply_config_edits(
    edits: Vec<Edit>,
    confirm: Option<String>,
    app: AppHandle,
) -> Validation {
    off_main(app, move |app, state| {
        apply_edits(&edits, confirm.as_deref(), app, state)
    })
    .await
}

/// The body of [`apply_config_edits`], on a blocking thread.
fn apply_edits(
    edits: &[Edit],
    confirm: Option<&str>,
    app: &AppHandle,
    state: &AppState,
) -> Validation {
    let raw = match read_or_empty(&state.config_path) {
        Ok(raw) => raw,
        Err(e) => return Validation::error(format!("could not read the configuration: {e}")),
    };

    let mut editor = match ConfigEditor::new(&raw) {
        Ok(editor) => editor,
        Err(e) => return Validation::error(e.to_string()),
    };
    if let Err(e) = editor.apply(edits) {
        return Validation::error(e.to_string());
    }

    let mut result = commit(app, state, &raw, editor.to_toml(), confirm);
    if result.ok
        && let Some(wanted) = launch_at_login_edit(edits)
        && let Err(e) = set_login_item(app, wanted)
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
    if let Err(refused) = store_admitted(
        &state.config_path,
        base,
        text,
        state.config().as_ref(),
        &SystemTokenProvider,
    ) {
        return refused;
    }
    reload_from_disk(app, false);
    validation
}

/// Put an admitted text on disk: first tie the 1.x own entry of every account
/// it moves to another server to the server it is leaving, then
/// compare-and-swap. `Err` is the refusal, and nothing was written.
///
/// ⛔ That order is the promise the host-move question makes ("its stored token
/// stays with the old host"). A 1.x value is bound on first use to whatever
/// the file says then, so a move written before that first use sent it to the
/// new host. If the entry cannot be bound (a locked store, a failed read), the
/// move is not written at all.
pub(crate) fn store_admitted(
    path: &std::path::Path,
    base: &str,
    text: &str,
    running: Option<&bridgewatch_core::config::Config>,
    provider: &dyn token::TokenProvider,
) -> Result<(), Validation> {
    bind_before_write(path, base, text, running, provider).map_err(Validation::error)?;
    match crate::config::write_if_unchanged(path, base, text) {
        Ok(()) => Ok(()),
        Err(crate::config::WriteError::Conflict) => {
            let mut refused = Validation::error(format!(
                "{} changed on disk since this window read it; nothing was written",
                path.display()
            ));
            refused.conflict = true;
            Err(refused)
        }
        Err(crate::config::WriteError::Io(e)) => Err(Validation::error(format!(
            "could not write {}: {e}",
            path.display()
        ))),
    }
}

/// [`token::bind_before_moves`] for a write: `text` measured against the file
/// it replaces, or, when that does not load, against what is running (the
/// same `before` as [`admit`]).
fn bind_before_write(
    path: &std::path::Path,
    base: &str,
    text: &str,
    running: Option<&bridgewatch_core::config::Config>,
    provider: &dyn token::TokenProvider,
) -> Result<(), String> {
    let after = bridgewatch_core::config::parse_str(text, path)
        .map_err(|_| "the configuration to be written does not load".to_string())?;
    let before = bridgewatch_core::config::parse_str(base, path)
        .ok()
        .map(|l| l.config)
        .or_else(|| running.cloned());
    match before {
        Some(before) => token::bind_before_moves(&before, &after.config, provider)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        None => Ok(()),
    }
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

/// The file's text for a reload: a missing file reads as empty, any other
/// failure is an error that names the file.
fn read_for_reload(path: &std::path::Path) -> Result<String, String> {
    read_or_empty(path).map_err(|e| format!("could not read {}: {e}", path.display()))
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
///
/// ⚠ A file that exists but cannot be read (EACCES, EISDIR, EIO) is reported
/// as exactly that, and the running configuration is kept as for any failed
/// reload. It used to read as empty, so a `chmod 000` told the user to add
/// an account to a file full of them. The last-seen text is forgotten, so the
/// reload after the fix goes ahead even when the file holds what it did
/// before.
///
/// ⚠️ Call it off the main thread (every caller does): a reload that sees an
/// account moved reads the credential store first.
pub fn reload_from_disk(app: &AppHandle, force: bool) {
    let state = app.state::<Arc<AppState>>();
    let path = state.config_path.clone();
    let raw = match read_for_reload(&path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!(error = %e, "reload failed; the running configuration is kept");
            {
                let mut inner = state.lock();
                inner.validation = Validation::error(e);
                inner.disk_text = None;
            }
            let _ = app.emit(CONFIG_CHANGED_EVENT, ());
            crate::poller::republish_errors(app);
            return;
        }
    };
    if !state.note_disk_text(&raw, force) {
        return;
    }
    // ONE read, parsed once: validating one read and loading another let a
    // write landing in between publish diagnostics for text that never ran.
    let validation = Validation::of(&raw, &path);
    let loaded = bridgewatch_core::config::parse_str(&raw, &path).ok();

    // ⛔ An edit this app did not write (another editor, a sync, a script) can
    // move an account too, and nothing asked about it. Before it runs, the
    // 1.x own entry of every moved account is tied to the server it was on,
    // exactly as a write from either window does (`store_admitted`); if that
    // fails, the edit does not run and the running configuration is kept, as
    // for any failed reload. The previous server is the RUNNING one: nothing
    // else records it, so an edit made while the app was not running is not
    // seen as a move (see `token::bind_before_moves`).
    if let Err(e) = bind_before_reload(
        state.config().as_ref(),
        loaded.as_ref().map(|l| &l.config),
        &SystemTokenProvider,
    ) {
        tracing::warn!(error = %e, "reload refused; the running configuration is kept");
        {
            let mut inner = state.lock();
            inner.validation = Validation::error(e);
            inner.disk_text = None;
        }
        let _ = app.emit(CONFIG_CHANGED_EVENT, ());
        crate::poller::republish_errors(app);
        return;
    }

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

    let running = state.config();
    if let Some(config) = &running {
        crate::logging::apply_config_level(&config.log.level);
    }
    // Every route to `[ui].send_crash_reports` ends here (the popover's
    // question, the Settings checkbox, the text tab, an edit in another
    // editor), so this is the one place the answer reaches the reporter. A
    // no takes effect before the next event.
    crate::crash::global().set_consent(crate::crash::consent_of(
        running.as_ref().and_then(|c| c.ui.send_crash_reports),
    ));
    // ⛔ The line the startup one cannot be: on a FIRST run there is no file, so
    // no level is applied, so `[log].level = "info"` in the file the wizard is
    // about to write cannot show the startup line: it was filtered before the
    // file existed. This runs after the level is applied and after the wizard's
    // save, which is what makes a first run say anything at all. A reload that
    // did not load reports that, rather than the counts of the configuration
    // still running.
    tracing::info!(
        "{}",
        crate::logging::config_line(
            &format!("reloaded {}", path.display()),
            if changed { running.as_ref() } else { None },
        )
    );
    let _ = app.emit(CONFIG_CHANGED_EVENT, ());

    if changed {
        state.request_reload();
    } else {
        // Nothing to restart, but the popover's error strip has to change.
        crate::poller::republish_errors(app);
    }
}

/// [`token::bind_before_moves`] for a reload: the file as it now loads,
/// measured against what is running. Nothing to measure (no running
/// configuration, as on a first launch, or a file that does not load) is
/// nothing to bind.
fn bind_before_reload(
    running: Option<&bridgewatch_core::config::Config>,
    loaded: Option<&bridgewatch_core::config::Config>,
    provider: &dyn token::TokenProvider,
) -> Result<(), String> {
    match (running, loaded) {
        (Some(running), Some(loaded)) => token::bind_before_moves(running, loaded, provider)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        _ => Ok(()),
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
pub async fn move_watch(id: String, delta: i32, app: AppHandle) -> Validation {
    off_main(app, move |app, state| {
        move_watch_now(&id, delta, app, state)
    })
    .await
}

/// The body of [`move_watch`], on a blocking thread.
fn move_watch_now(id: &str, delta: i32, app: &AppHandle, state: &AppState) -> Validation {
    let raw = match std::fs::read_to_string(&state.config_path) {
        Ok(raw) => raw,
        Err(e) => return Validation::error(format!("could not read the configuration: {e}")),
    };
    match crate::reorder::move_watch(&raw, id, delta) {
        Ok(Some(text)) => commit(app, state, &raw, text, None),
        Ok(None) => Validation {
            ok: true,
            ..Validation::default()
        },
        Err(e) => Validation::error(e),
    }
}

/// Reload the configuration from disk. The menu item and the settings pane
/// both call this.
///
/// On a blocking thread: a reload that sees an account moved reads the
/// credential store (see [`save_config_text`]).
#[tauri::command]
pub async fn reload_config(app: AppHandle) {
    let _ = tauri::async_runtime::spawn_blocking(move || reload_from_disk(&app, true)).await;
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
///
/// ⚠ Async, with the store call on a blocking thread, like the wizard's: a
/// synchronous command runs on the main thread, so a Keychain prompt (or a
/// slow Secret Service) froze the tray and both windows until it was answered.
#[tauri::command]
pub async fn set_own_token(
    account: String,
    token: String,
    state: Shared<'_>,
) -> Result<(), String> {
    let pasted = token.trim().to_string();
    if pasted.is_empty() {
        return Err("the token is empty".into());
    }
    let state = state.inner().clone();
    // ⛔ The server the token is bound to comes from the FILE, read here, and
    // never from the webview: whoever names the base_url names where the
    // token goes, and a script in the webview must not be the one to say.
    let base_url = account_on_disk(&state.config_path, &account)?.base_url;
    let account_name = account.clone();
    tauri::async_runtime::spawn_blocking(move || {
        token::set_own_token(
            &account,
            &base_url,
            &Secret::new(pasted),
            &SystemTokenProvider,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    // A refused account is retried in place, so no other watch loses its
    // state; an account that was working gets its new token through a
    // reload, as before (its client holds the old one).
    if state.is_refused(&account_name) {
        state.retry_refused.notify_one();
    } else {
        state.request_reload();
    }
    Ok(())
}

/// The account named `account` as the configuration file on disk says it is,
/// read leniently: an unknown key elsewhere in the file must not hide it.
pub(crate) fn account_on_disk(
    path: &std::path::Path,
    account: &str,
) -> Result<bridgewatch_core::config::Account, String> {
    let raw = read_or_empty(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (config, _) = bridgewatch_core::config::parse_lenient(&raw, path).map_err(|e| {
        format!(
            "the configuration file does not load, so the token has no server to belong to: {e}"
        )
    })?;
    config.accounts.get(account).cloned().ok_or_else(|| {
        format!(
            "there is no account {account:?} in the configuration file yet; save the account \
             first, then its token"
        )
    })
}

/// What bridgewatch's own entry for an account holds (none, legacy, bound,
/// or bound elsewhere), against the account's `base_url` as the file on disk
/// says it. Never the token.
///
/// ⚠ Off the main thread: it reads the credential store.
#[tauri::command]
pub async fn own_token_status(
    account: String,
    state: Shared<'_>,
) -> Result<token::OwnTokenStatus, String> {
    let entry = account_on_disk(&state.config_path, &account)?;
    tauri::async_runtime::spawn_blocking(move || {
        token::own_token_status(&account, &entry, &SystemTokenProvider)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Remove bridgewatch's own credential-store entry for an account.
///
/// ⛔ Reloads on success, like [`set_own_token`]. Own tokens are resolved once,
/// when the poller is built, so without the rebuild a forgotten token went on
/// being sent by the poller and by job actions until the next restart. Off the
/// main thread for the same reason as [`set_own_token`].
#[tauri::command]
pub async fn clear_own_token(account: String, state: Shared<'_>) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        token::clear_own_token(&account, &SystemTokenProvider)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    state.request_reload();
    Ok(())
}

/// Change the OS login item, mirror what the OS then reports into the file,
/// and put the tray's checkbox right. Every route to the setting ends here:
/// the tray menu and a form edit of `ui.launch_at_login`.
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

    /// ⛔ `set_own_token` and `own_token_status` read the account from the
    /// FILE: its base_url is where the token is bound, and an account the
    /// file does not have yet gets no token at all.
    #[test]
    fn the_own_token_commands_read_the_account_from_the_file_on_disk() {
        let dir = std::env::temp_dir().join(format!("bw-own-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[accounts.gl]\nbase_url = \"https://gitlab.example.com\"\ntoken = { own = true }\n\
             unknown_key = 1\n",
        )
        .unwrap();
        let account = account_on_disk(&path, "gl").expect("read leniently");
        assert_eq!(account.base_url, "https://gitlab.example.com");
        let missing = account_on_disk(&path, "other").unwrap_err();
        assert!(missing.contains("save the account first"), "{missing}");
        let none = account_on_disk(&dir.join("absent.toml"), "gl").unwrap_err();
        assert!(none.contains("no account \"gl\""), "{none}");
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    #[test]
    fn a_reload_that_cannot_read_the_file_says_so_and_a_missing_one_is_empty() {
        // #13: an unreadable file (here a directory, EISDIR) read as "" and was
        // published as "no accounts defined" over a file full of accounts.
        let dir = std::env::temp_dir().join(format!("bw-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = read_for_reload(&dir).expect_err("an unreadable file read as text");
        assert!(
            err.starts_with(&format!("could not read {}: ", dir.display())),
            "{err}"
        );
        assert_eq!(read_for_reload(&dir.join("missing.toml")).unwrap(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A credential store in memory holding one 1.x (bare) value under
    /// `bridgewatch:gl`; `locked` makes every read and write fail, as a
    /// locked keychain does.
    struct Store {
        entry: std::sync::Mutex<Option<String>>,
        locked: bool,
    }
    impl Store {
        fn legacy(locked: bool) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                entry: std::sync::Mutex::new(Some("glpat-legacy".into())),
                locked,
            })
        }
        fn entry(&self) -> Option<String> {
            self.entry.lock().unwrap().clone()
        }
        fn refuse(&self, service: &str) -> Result<(), token::TokenError> {
            if self.locked {
                return Err(token::TokenError::Store {
                    service: service.into(),
                    message: "the keychain is locked".into(),
                });
            }
            Ok(())
        }
    }
    impl token::TokenProvider for Store {
        fn keyring_get(&self, service: &str, user: &str) -> Result<String, token::TokenError> {
            self.refuse(service)?;
            match (service, user, self.entry()) {
                ("bridgewatch:gl", "token", Some(v)) => Ok(v),
                _ => Err(token::TokenError::NoEntry {
                    service: service.into(),
                    user: user.into(),
                }),
            }
        }
        fn keyring_set(
            &self,
            service: &str,
            _: &str,
            value: &str,
        ) -> Result<(), token::TokenError> {
            self.refuse(service)?;
            *self.entry.lock().unwrap() = Some(value.into());
            Ok(())
        }
        fn keyring_delete(&self, _: &str, _: &str) -> Result<(), token::TokenError> {
            Ok(())
        }
        fn env(&self, _: &str) -> Option<String> {
            None
        }
        fn run(&self, _: &[String]) -> Result<String, token::TokenError> {
            Err(token::TokenError::Empty)
        }
    }

    const ON_NEW: &str =
        "[accounts.gl]\nbase_url = \"https://evil.example\"\ntoken = { own = true }\n";

    /// What the poller would send for `gl` as the file on disk now says it:
    /// the client is built as the poller builds it, and asked one question.
    fn poll_from_disk(path: &Path, store: std::sync::Arc<Store>) -> Vec<String> {
        let account = account_on_disk(path, "gl").expect("the account is on disk");
        let transport = std::sync::Arc::new(bridgewatch_core::client::ScriptTransport::new());
        if let Ok(client) = bridgewatch_core::poll::client_for_account_on(
            "gl",
            &account,
            None,
            store,
            transport.clone(),
            bridgewatch_core::client::RequestRing::new(4),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = rt.block_on(client.current_user());
        }
        transport.requests().into_iter().map(|r| r.url).collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, OWN).unwrap();
        path
    }

    /// ⛔ The Seer finding on the 2.0 release. The host-move question says an
    /// own token "stays with the old host", but a token bridgewatch 1.x stored
    /// is bound to no host until first used, and the first use after a move
    /// bound it to the NEW one. Every write from either window now ties it to
    /// the old host before the file changes, so the poll after the edit sends
    /// nothing.
    #[test]
    fn a_host_edit_ties_a_1x_token_to_the_old_host_before_the_poll() {
        let path = scratch("move-legacy");
        let store = Store::legacy(false);
        store_admitted(&path, OWN, ON_NEW, None, store.as_ref()).expect("written");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ON_NEW);
        let urls = poll_from_disk(&path, store.clone());
        assert!(urls.is_empty(), "a request reached the new host: {urls:?}");
        let entry = store.entry().unwrap();
        assert!(
            entry.starts_with(token::BOUND_V1)
                && entry.contains(r#""origin":"https://gitlab.com""#),
            "the entry is bound to the old host"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// ⛔ With the keychain locked the entry cannot be bound, so the move is
    /// refused with the store's error and the file is not written: the poll
    /// still goes to the old host only.
    #[test]
    fn a_host_edit_with_the_keychain_locked_is_refused_and_writes_nothing() {
        let path = scratch("move-locked");
        let store = Store::legacy(true);
        let refused = store_admitted(&path, OWN, ON_NEW, None, store.as_ref())
            .expect_err("a move with an unbound 1.x token was written");
        let message = &refused.diagnostics[0].message;
        assert!(message.contains("was not moved"), "{message}");
        assert!(message.contains("the keychain is locked"), "{message}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), OWN);
        assert_eq!(store.entry().as_deref(), Some("glpat-legacy"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The same for an edit the app did not write: the reload refuses it, so
    /// the running configuration (the old host) is kept.
    #[test]
    fn a_host_edit_made_in_another_editor_is_bound_or_refused_on_reload() {
        let parse = |raw: &str| {
            bridgewatch_core::config::parse_str(raw, Path::new("x.toml"))
                .unwrap()
                .config
        };
        let (old, new) = (parse(OWN), parse(ON_NEW));
        let locked = Store::legacy(true);
        let e = bind_before_reload(Some(&old), Some(&new), locked.as_ref()).unwrap_err();
        assert!(e.contains("was not moved"), "{e}");
        assert_eq!(locked.entry().as_deref(), Some("glpat-legacy"));

        let open = Store::legacy(false);
        bind_before_reload(Some(&old), Some(&new), open.as_ref()).expect("bound");
        assert!(
            open.entry()
                .unwrap()
                .contains(r#""origin":"https://gitlab.com""#)
        );
        // A first launch has nothing running to measure against.
        let fresh = Store::legacy(true);
        assert!(bind_before_reload(None, Some(&new), fresh.as_ref()).is_ok());
    }
}
