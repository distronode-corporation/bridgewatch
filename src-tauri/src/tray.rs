//! The tray icon and its menu.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use bridgewatch_core::config::ProjectRef;
use bridgewatch_core::poll::PollNow;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;
use crate::windows;

/// The tray's id. `TrayIconBuilder::with_id` fixes it so `tray_by_id` can find
/// the same icon again to swap its image every tick.
pub const TRAY_ID: &str = "bridgewatch";

/// The tooltip's product name, on the platforms that have a tooltip.
///
/// ⛔ `TrayIconBuilder::title` is NOT the way to name the tray, on either
/// desktop we ship. Read at the versions in Cargo.lock (tauri 2.11.5
/// `src/tray/mod.rs`, tray-icon 0.24.2): the macOS backend implements it as
/// `NSStatusBarButton::setTitle` (`platform_impl/macos/mod.rs`) and the GTK
/// backend as `AppIndicator::set_label` (`platform_impl/gtk/mod.rs`). Both draw
/// the string as TEXT beside the icon, in the menu bar and in the panel, and
/// bridgewatch's presence there is an icon and nothing else.
///
/// ⚠ So the Linux tray still reports the binary name, `bridgewatch-app`, as
/// its StatusNotifierItem `Title` (seen over D-Bus and in shells that show it).
/// That property defaults to `g_get_application_name()`; `libappindicator`
/// exposes `set_title` and tray-icon never calls it, so it cannot be set
/// through Tauri 2. The route that would work is `glib::set_application_name`
/// before Tauri starts, which means a direct dependency on the archived
/// gtk-rs 0.18 `glib`. Not taken for a string most desktops never display.
/// `tooltip` is documented "**Linux:** Unsupported", so Linux gets neither.
#[cfg(not(target_os = "linux"))]
const TRAY_LABEL: &str = "bridgewatch";

/// The "Launch at login" checkbox.
///
/// ⛔ `TrayIcon` has no `menu()` getter in Tauri 2 — only `set_menu` — so the
/// only way to flip a checkbox after the menu is built is to have kept the item
/// itself. Without this the checkbox would show what was ASKED for rather than
/// what the OS did, which on a failed `enable()` is a lie.
static AUTOSTART_ITEM: OnceLock<CheckMenuItem<tauri::Wry>> = OnceLock::new();

const ID_SHOW: &str = "show";
const ID_PIPELINES: &str = "pipelines";
const ID_REFRESH: &str = "refresh";
const ID_RELOAD: &str = "reload";
const ID_OPEN_CONFIG: &str = "open-config";
const ID_SETTINGS: &str = "settings";
const ID_WIZARD: &str = "wizard";
const ID_AUTOSTART: &str = "autostart";
const ID_QUIT: &str = "quit";

/// Where this process's tray image files go on Linux, or `None` elsewhere.
///
/// ⛔ Linux's AppIndicator takes an icon as a FILE PATH, so `tray-icon` writes
/// every image it is handed to disk, by default into a directory shared by
/// every app using the crate. It deletes the previous file on each swap and the
/// current one when the tray is dropped, but a process that is killed (logout,
/// `kill`, a crash) never drops it, and those files stayed forever. So each run
/// gets a directory of its own, named for its pid, which [`remove_icon_dir`]
/// deletes on exit and the next start sweeps if the owner is gone.
pub fn icon_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
        .join("bridgewatch");
    sweep_icon_dirs(&base, |pid| {
        Path::new("/proc").join(pid.to_string()).exists()
    });
    Some(icon_dir_for(&base, std::process::id()))
}

fn icon_dir_for(base: &Path, pid: u32) -> PathBuf {
    base.join(format!("tray-{pid}"))
}

/// Remove every `tray-<pid>` directory under `base` whose process is gone.
fn sweep_icon_dirs(base: &Path, alive: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("tray-"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != std::process::id() && !alive(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Drop the tray (which deletes its current image file) and remove this
/// process's icon directory. Called on exit.
pub fn remove_icon_dir(app: &AppHandle) {
    let _ = app.remove_tray_by_id(TRAY_ID);
    if let Some(dir) = ICON_DIR.get() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The directory [`build`] handed to the tray, for [`remove_icon_dir`].
static ICON_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Build the tray icon, its menu and its event handlers.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let launch_at_login = app.autolaunch().is_enabled().unwrap_or(false);

    // ⛔ "Show status" is first because Linux is the platform that needs it.
    // Most AppIndicator implementations deliver NO left-click event at all —
    // the shell opens the menu instead — so on those desktops this item is the
    // only way to reach the popover. It is present on macOS too rather than
    // conditionally compiled: one harmless item is cheaper than two menus.
    let show = MenuItem::with_id(app, ID_SHOW, "Show status", true, None::<&str>)?;
    let pipelines =
        MenuItem::with_id(app, ID_PIPELINES, "Open pipelines page", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, ID_REFRESH, "Refresh now", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, ID_RELOAD, "Reload config", true, None::<&str>)?;
    let open_config =
        MenuItem::with_id(app, ID_OPEN_CONFIG, "Open config file", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings…", true, None::<&str>)?;
    let wizard = MenuItem::with_id(app, ID_WIZARD, "Setup wizard…", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        ID_AUTOSTART,
        "Launch at login",
        true,
        launch_at_login,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit bridgewatch", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show,
            &pipelines,
            &refresh,
            &PredefinedMenuItem::separator(app)?,
            &reload,
            &open_config,
            &settings,
            &wizard,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // Through `drawable_for`, like every later swap: the tray is never built
    // without an image (a status item with none is invisible).
    let initial = crate::icons::drawable_for(
        &app.state::<Arc<AppState>>()
            .config()
            .map(|c| c.icon)
            .unwrap_or_default(),
        bridgewatch_core::verdict::IconState::Unknown,
    );

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // Without this macOS opens the menu on a left click too, and the
        // popover can never be toggled.
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            // This must run for EVERY event, not just clicks: it is how the
            // positioner learns where the tray icon is, and a popover placed
            // without it lands in the middle of the screen.
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                windows::toggle_popover(tray.app_handle());
            }
        });

    // See [`TRAY_LABEL`]: a tooltip where the platform has one, and never a
    // title, which both desktops draw as text beside the icon.
    #[cfg(not(target_os = "linux"))]
    {
        builder = builder.tooltip(TRAY_LABEL);
    }

    builder = builder
        .icon(initial.image)
        .icon_as_template(initial.template);
    if let Some(dir) = icon_dir() {
        builder = builder.temp_dir_path(&dir);
        let _ = ICON_DIR.set(dir);
    }

    builder.build(app)?;
    let _ = AUTOSTART_ITEM.set(autostart);
    Ok(())
}

/// Reflect the OS's idea of the login item back into the menu's checkbox.
///
/// Called after the checkbox is toggled, because `enable()` can fail — a
/// LaunchAgent directory that is not writable, for one — and a checkbox that
/// shows what was asked for rather than what happened is a lie.
pub fn sync_autostart_check(app: &AppHandle) {
    let enabled = app.autolaunch().is_enabled().unwrap_or(false);
    if let Some(item) = AUTOSTART_ITEM.get() {
        let _ = item.set_checked(enabled);
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        ID_SHOW => windows::show_popover(app),
        ID_PIPELINES => {
            // ⛔ Through `links::open`, like every other link: the URL is
            // derived from a GitLab-supplied `web_url`, and the Rust opener has
            // no scope, so a hostile instance could otherwise have this menu
            // item open `file://` anything.
            if let Some(url) = pipelines_url(app) {
                let config = app.state::<Arc<AppState>>().config();
                if let Err(e) = crate::links::open(app, &url, config.as_ref()) {
                    tracing::warn!(error = %e, "could not open the pipelines page");
                }
            } else {
                tracing::warn!("no pipelines URL could be derived from the configuration");
            }
        }
        ID_REFRESH => {
            app.state::<Arc<AppState>>().request_poll(PollNow::Manual);
        }
        ID_RELOAD => {
            crate::commands::reload_from_disk(app, true);
        }
        ID_OPEN_CONFIG => {
            let path = app.state::<Arc<AppState>>().config_path.clone();
            if let Err(e) = app.opener().open_path(path.to_string_lossy(), None::<&str>) {
                tracing::warn!(error = %e, "could not open the config file");
            }
        }
        ID_SETTINGS => windows::show_settings(app, None),
        ID_WIZARD => crate::wizard::show_wizard(app),
        ID_AUTOSTART => {
            let enabled = app.autolaunch().is_enabled().unwrap_or(false);
            // The config file records the result too, so the Settings pane
            // and the menu cannot disagree.
            if let Err(e) = crate::commands::set_login_item(app, !enabled) {
                tracing::warn!(error = %e, "could not change the login item");
            }
        }
        ID_QUIT => app.exit(0),
        other => tracing::debug!(id = other, "unhandled tray menu id"),
    }
}

/// A pipeline's `web_url` minus its trailing id: the project's pipelines
/// index. `None` when the URL does not end in an id.
fn pipelines_index(web_url: &str) -> Option<String> {
    let (head, tail) = web_url.rsplit_once('/')?;
    (!tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())).then(|| head.to_string())
}

/// Where "Open pipelines page" goes.
///
/// Derived from the snapshot rather than assembled from the configuration
/// wherever possible: a pipeline row's `web_url` is a URL GitLab itself
/// produced, and its parent directory is the project's pipelines index. A
/// project addressed by NUMERIC id has no URL that can be constructed without
/// asking the API what its path is, so the fallback only works for a
/// `group/path` project — and returning nothing is better than opening a 404.
pub fn pipelines_url(app: &AppHandle) -> Option<String> {
    let state = app.state::<Arc<AppState>>();

    let from_snapshot = {
        let inner = state.lock();
        let watches = &inner.snapshot.watches;
        watches
            .iter()
            .find(|w| w.role.is_primary())
            .or_else(|| watches.first())
            .and_then(|w| w.rows.first())
            .and_then(|r| r.web_url.as_deref())
            .and_then(pipelines_index)
    };
    if from_snapshot.is_some() {
        return from_snapshot;
    }

    let config = state.config()?;
    let watch = config
        .watches
        .iter()
        .find(|w| w.role.is_primary())
        .or_else(|| config.watches.first())?;
    let account = config.accounts.get(&watch.account)?;
    match &watch.project {
        ProjectRef::Path(path) => Some(format!(
            "{}/{}/-/pipelines",
            account.base_url.trim_end_matches('/'),
            path.trim_matches('/')
        )),
        ProjectRef::Id(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostile_web_url_never_reaches_the_opener() {
        // M12: the menu item derives its URL from GitLab's `web_url`.
        let config = bridgewatch_core::config::parse_str(
            "[accounts.gl]\nbase_url = \"https://gitlab.com\"\n",
            Path::new("x.toml"),
        )
        .unwrap()
        .config;
        let good = pipelines_index("https://gitlab.com/g/p/-/pipelines/42").unwrap();
        assert_eq!(good, "https://gitlab.com/g/p/-/pipelines");
        assert!(crate::links::check(&good, Some(&config)).is_ok());
        for hostile in [
            "file:///etc/passwd/1",
            "https://evil.example/g/p/-/pipelines/1",
        ] {
            let url = pipelines_index(hostile).unwrap();
            assert!(crate::links::check(&url, Some(&config)).is_err(), "{url}");
        }
        assert_eq!(pipelines_index("https://gitlab.com/g/p"), None);
    }

    #[test]
    fn a_dead_runs_icon_directory_is_swept_and_a_live_one_is_not() {
        let base = std::env::temp_dir().join(format!("bridgewatch-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for pid in [11, 22] {
            let dir = icon_dir_for(&base, pid);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("tray-icon-bridgewatch-3.png"), b"png").unwrap();
        }
        std::fs::create_dir_all(base.join("unrelated")).unwrap();

        sweep_icon_dirs(&base, |pid| pid == 22);

        assert!(
            !icon_dir_for(&base, 11).exists(),
            "a dead run's files were left"
        );
        assert!(
            icon_dir_for(&base, 22).exists(),
            "a live run's files were removed"
        );
        assert!(base.join("unrelated").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
