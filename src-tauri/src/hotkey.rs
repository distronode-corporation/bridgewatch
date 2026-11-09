//! The optional global shortcut, `[ui].hotkey`, that shows or hides the popover.
//!
//! ⛔ The plugin is added only once a shortcut is configured, never at startup.
//! Its setup opens the platform's hotkey manager, which on Linux is an X11
//! connection (global-hotkey 0.8 is "Linux (X11 Only)"), and a plugin whose
//! setup fails at build time stops the app from starting at all. A session
//! with no X server, and every user who never sets a shortcut, would pay for a
//! feature they do not use. Added late, a failure is a log line.
//!
//! ⚠️ Wayland has no global shortcut an app can register. Under XWayland the
//! X11 grab fires only while an X11 window has the focus, which in practice is
//! almost never, so the shortcut is off by default everywhere and the log says
//! so when one is set in a Wayland session.
//!
//! Everything here runs on the main thread (`run_on_main_thread`): macOS
//! requires the hotkey manager there, and the plugin's own calls block on the
//! main thread, which run at once when they are already on it.

use std::sync::Mutex;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// The value last applied, so a reload that leaves `[ui].hotkey` alone does
/// not unregister and register it again. `None` until the first apply.
static APPLIED: Mutex<Option<String>> = Mutex::new(None);

/// Whether the popover was open when the shortcut was pressed; taken at the
/// release. See the handler in [`apply_on_main`].
static FOUND_OPEN: Mutex<Option<bool>> = Mutex::new(None);

/// Whether the plugin is in, and whether adding it failed (it is not tried
/// again: the cause, no display server or no permission, does not go away).
static PLUGIN: Mutex<PluginState> = Mutex::new(PluginState::Absent);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluginState {
    Absent,
    Added,
    Failed,
}

/// What `[ui].hotkey` asks for: `Ok(None)` for none (empty or blank), the
/// shortcut, or why it does not parse.
pub fn parse(value: &str) -> Result<Option<Shortcut>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<Shortcut>()
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Whether this is a Wayland session, from the two variables a Wayland
/// desktop sets for its clients.
pub fn is_wayland(wayland_display: Option<&str>, session_type: Option<&str>) -> bool {
    wayland_display.is_some_and(|d| !d.is_empty()) || session_type == Some("wayland")
}

fn wayland_session() -> bool {
    cfg!(target_os = "linux")
        && is_wayland(
            std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        )
}

/// Register `[ui].hotkey` in place of the one before it. Called at startup and
/// after every reload; never fails (a problem is logged).
pub fn apply(app: &AppHandle, value: &str) {
    let value = value.trim().to_string();
    {
        let mut applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
        if applied.as_deref() == Some(value.as_str()) {
            return;
        }
        *applied = Some(value.clone());
    }
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || apply_on_main(&handle, &value)) {
        tracing::warn!(error = %e, "could not reach the main thread to set the global shortcut");
    }
}

fn apply_on_main(app: &AppHandle, value: &str) {
    let shortcut = match parse(value) {
        Ok(shortcut) => shortcut,
        Err(e) => {
            tracing::warn!(hotkey = value, error = %e, "[ui].hotkey does not parse; no global shortcut");
            None
        }
    };
    let mut plugin = PLUGIN.lock().unwrap_or_else(|e| e.into_inner());
    if *plugin == PluginState::Absent && shortcut.is_some() {
        let built = tauri_plugin_global_shortcut::Builder::new()
            // ⛔ Acted on at RELEASE, decided at PRESS. While the keys are down
            // the X server holds the keyboard for the grab that caught them:
            // a popover shown on press got the focus and lost it as the keys
            // came up, and an open one is blurred (and hidden) by the press.
            .with_handler(|app, _shortcut, event| match event.state() {
                ShortcutState::Pressed => {
                    let open = crate::windows::popover_open_for_shortcut(app);
                    *FOUND_OPEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(open);
                }
                ShortcutState::Released => {
                    let open = FOUND_OPEN.lock().unwrap_or_else(|e| e.into_inner()).take();
                    tracing::debug!(was_open = ?open, "global shortcut released; toggling the popover");
                    crate::windows::toggle_popover_from_shortcut(app, open.unwrap_or(false));
                }
            })
            .build();
        *plugin = match app.plugin(built) {
            Ok(()) => PluginState::Added,
            Err(e) => {
                tracing::warn!(error = %e, "global shortcuts are not available here; [ui].hotkey is ignored");
                PluginState::Failed
            }
        };
    }
    if *plugin != PluginState::Added {
        return;
    }
    let shortcuts = app.global_shortcut();
    if let Err(e) = shortcuts.unregister_all() {
        tracing::warn!(error = %e, "could not remove the previous global shortcut");
    }
    let Some(shortcut) = shortcut else { return };
    if wayland_session() {
        tracing::warn!(
            hotkey = value,
            "Wayland session: a global shortcut fires only while an X11 window has the focus"
        );
    }
    match shortcuts.register(shortcut) {
        Ok(()) => tracing::info!(hotkey = value, "global shortcut registered"),
        // Most often another application holds the same combination.
        Err(e) => {
            tracing::warn!(hotkey = value, error = %e, "could not register the global shortcut")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_means_none_and_a_bad_value_says_why() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("   ").unwrap(), None);
        assert!(parse("CmdOrCtrl+Shift+B").unwrap().is_some());
        assert!(parse(" Alt+F12 ").unwrap().is_some());
        assert!(parse("Shift+").is_err());
        assert!(parse("Hyper+Q").is_err());
    }

    #[test]
    fn a_wayland_session_is_told_by_either_variable() {
        assert!(is_wayland(Some("wayland-0"), None));
        assert!(is_wayland(None, Some("wayland")));
        assert!(!is_wayland(Some(""), Some("x11")));
        assert!(!is_wayland(None, None));
    }
}
