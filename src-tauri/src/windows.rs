//! Showing, sizing and placing the two windows.
//!
//! Both are declared in `tauri.conf.json` with `visible: false`. Creating them
//! up front and hiding them means the first popover open is instant; creating
//! them lazily would put a webview boot between the click and anything
//! appearing. The cost is that nothing may flash on launch, which is what the
//! `visible: false` is for.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_positioner::{Position, WindowExt};

use crate::state::AppState;

/// The tray popover.
pub const POPOVER: &str = "popover";
/// The settings window.
pub const SETTINGS: &str = "settings";
/// Emitted when the settings window should switch tab.
pub const OPEN_SETTINGS_EVENT: &str = "open-settings";

/// Whether the popover has a native vibrancy material behind it.
static VIBRANCY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True once [`apply_vibrancy`] succeeded. The frontend reads it through
/// `Status.vibrancy` and only then makes its page transparent: a transparent
/// page with no material behind it is a popover you can see the desktop
/// through.
pub fn vibrancy() -> bool {
    VIBRANCY.load(std::sync::atomic::Ordering::Relaxed)
}

/// macOS only: put an `NSVisualEffectView` (the `Popover` material, always
/// active so it does not grey out when the popover loses key status) behind
/// the popover. Needs `macOSPrivateApi` and a transparent window, both set in
/// tauri.conf.json. Linux has no equivalent: WebKitGTK behind a transparent
/// page shows the desktop, so it is never attempted there.
pub fn apply_vibrancy(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        use tauri::window::{Effect, EffectState, EffectsBuilder};
        let Some(window) = popover(app) else { return };
        let effects = EffectsBuilder::new()
            .effect(Effect::Popover)
            .state(EffectState::Active)
            .radius(10.0)
            .build();
        match window.set_effects(effects) {
            Ok(()) => VIBRANCY.store(true, std::sync::atomic::Ordering::Relaxed),
            Err(e) => tracing::warn!(error = %e, "no popover vibrancy"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// The popover window, if it exists.
pub fn popover(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(POPOVER)
}

/// Show the popover at the tray, sized from `[ui].popover`.
pub fn show_popover(app: &AppHandle) {
    let Some(window) = popover(app) else { return };
    let state = app.state::<Arc<AppState>>();

    // ⚠ The size is NOT set here. The frontend owns the height — it is the only
    // side that knows how tall the content is — and it has already called
    // `resize_popover`, because both windows are created and loaded at startup
    // rather than on first click. Setting max_height here would show a 720px
    // window and then snap it shorter, which reads as a glitch.

    // The positioner knows where the tray is only because `on_tray_icon_event`
    // hands it every tray event; without that it silently falls back to the
    // window's current position, which is why the tray handler in tray.rs calls
    // it before doing anything else.
    //
    // macOS has its menu bar at the top, so the popover hangs below the icon.
    // A Linux panel may be at either edge, and TrayCenter is the variant that
    // does not assume.
    let position = if cfg!(target_os = "macos") {
        Position::TrayBottomCenter
    } else {
        Position::TrayCenter
    };
    if window.move_window(position).is_err() {
        let _ = window.center();
    }

    let _ = window.show();
    let _ = window.set_focus();
    // Opening the popover is one of the two out-of-band poll reasons. The core
    // gates it: a second open within five seconds is dropped.
    state.request_poll(bridgewatch_core::poll::PollNow::PopoverOpened);
}

/// Hide the popover.
pub fn hide_popover(app: &AppHandle) {
    if let Some(window) = popover(app) {
        let _ = window.hide();
    }
}

/// How recently a blur-hide counts as "the popover was open when you clicked".
///
/// ⛔ Clicking the tray icon while the popover has focus fires TWO things in
/// order: the window loses focus (so hide-on-blur hides it) and then the tray
/// click arrives. A naive toggle reads `is_visible() == false` at that point
/// and re-opens the popover, so clicking the icon to dismiss it does nothing
/// visible. Remembering when the last blur-hide happened is what closes that
/// hole; anything under this window is treated as "it was open".
pub const BLUR_HIDE_GRACE: Duration = Duration::from_millis(300);

/// Record that the popover was hidden, for [`toggle_popover`]'s benefit.
pub fn note_popover_hidden(app: &AppHandle) {
    app.state::<Arc<AppState>>().lock().popover_hidden_at = Some(Instant::now());
}

/// Show it if it is hidden, hide it if it is not. The left-click behaviour.
pub fn toggle_popover(app: &AppHandle) {
    let Some(window) = popover(app) else { return };

    let just_hidden = app
        .state::<Arc<AppState>>()
        .lock()
        .popover_hidden_at
        .is_some_and(|t| t.elapsed() < BLUR_HIDE_GRACE);

    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        note_popover_hidden(app);
    } else if !just_hidden {
        show_popover(app);
    }
}

/// Show the settings window, optionally on a named tab.
///
/// The tab is BOTH emitted and parked in the shared state. Emitting alone
/// loses the race on a first open — the window's JavaScript is not listening
/// yet — and parking alone would not move an already-open window off whatever
/// tab it is on.
pub fn show_settings(app: &AppHandle, tab: Option<&str>) {
    if let Some(tab) = tab {
        let state = app.state::<Arc<AppState>>();
        state.lock().settings_tab.replace(tab.to_string());
        let _ = app.emit_to(SETTINGS, OPEN_SETTINGS_EVENT, tab);
    }
    if let Some(window) = app.get_webview_window(SETTINGS) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
