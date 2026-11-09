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
    // Opening the popover is one of the out-of-band poll reasons. The core
    // gates it: a second request within `POLL_NOW_MIN_GAP` (two seconds) of
    // the last one passed on is dropped, which is what makes this and the
    // webview's own focus handler one tick between them.
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

/// How soon after the popover first gains the focus a focus loss is the
/// global shortcut's own flapping rather than a blur.
///
/// ⛔ Shown from the shortcut, the popover's focus flaps: on X11 (the harness
/// in tools/real-device, measured) it went in, out, in and out within a
/// millisecond of first gaining the focus, as the key grab that caught the
/// shortcut let go, and hide-on-blur hid it before it was ever drawn. Counted
/// from that first focus, not from the shortcut, so a slow show still gets
/// the whole window; and short, because a person cannot click elsewhere this
/// fast, so a real blur is never taken for flapping (Seer, #91).
pub const SHORTCUT_FLAP: Duration = Duration::from_millis(60);

/// When, after a shortcut show, the focus is asked for again if the flapping
/// left the popover without it.
pub const SHORTCUT_SETTLE: Duration = Duration::from_millis(250);

/// The focus state of the last shortcut show.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShortcutFocus {
    /// When the shortcut showed the popover.
    pub shown_at: Option<Instant>,
    /// When the popover first gained the focus after that.
    pub focused_at: Option<Instant>,
    /// A real blur happened since: the focus is not asked for again.
    pub blurred: bool,
}

impl ShortcutFocus {
    /// A new shortcut show, at `now`.
    pub fn shown(now: Instant) -> Self {
        Self {
            shown_at: Some(now),
            focused_at: None,
            blurred: false,
        }
    }

    fn settling(&self, now: Instant) -> bool {
        self.shown_at
            .is_some_and(|s| now.saturating_duration_since(s) < SHORTCUT_SETTLE)
    }

    /// The popover gained the focus. The first time after a show is what
    /// [`SHORTCUT_FLAP`] counts from.
    pub fn on_focus(&mut self, now: Instant) {
        if self.settling(now) && self.focused_at.is_none() {
            self.focused_at = Some(now);
        }
    }

    /// The popover lost the focus. Returns true when that is the shortcut's
    /// flapping and is to be ignored; otherwise it is a real blur, which also
    /// stops the focus being asked for again.
    pub fn on_blur(&mut self, now: Instant) -> bool {
        let flap = self.settling(now)
            && self
                .focused_at
                .is_some_and(|f| now.saturating_duration_since(f) < SHORTCUT_FLAP);
        if !flap {
            self.blurred = true;
        }
        flap
    }

    /// Whether to ask for the focus again once [`SHORTCUT_SETTLE`] has
    /// passed: only when no real blur happened.
    pub fn refocus(&self) -> bool {
        self.shown_at.is_some() && !self.blurred
    }
}

/// The popover gained or lost the focus: see [`ShortcutFocus`]. Returns true
/// for a focus loss that is the shortcut's flapping.
pub fn shortcut_focus_event(app: &AppHandle, focused: bool) -> bool {
    let state = app.state::<Arc<AppState>>();
    let mut inner = state.lock();
    let now = Instant::now();
    if focused {
        inner.popover_shortcut.on_focus(now);
        false
    } else {
        inner.popover_shortcut.on_blur(now)
    }
}

/// Whether the popover counts as open for the global shortcut: showing, or
/// hidden by a blur within [`BLUR_HIDE_GRACE`].
///
/// ⛔ Read when the shortcut is PRESSED and acted on when it is released. The
/// press itself takes the keyboard (the key grab), which blurs an open popover
/// and hides it, so by the release it always looked closed and the shortcut
/// could only ever open it.
pub fn popover_open_for_shortcut(app: &AppHandle) -> bool {
    let Some(window) = popover(app) else {
        return false;
    };
    let just_hidden = app
        .state::<Arc<AppState>>()
        .lock()
        .popover_hidden_at
        .is_some_and(|t| t.elapsed() < BLUR_HIDE_GRACE);
    just_hidden || window.is_visible().unwrap_or(false)
}

/// The global shortcut, on release: hide the popover if it was open at the
/// press, else show it and take the focus again once the shortcut's own focus
/// changes have settled.
pub fn toggle_popover_from_shortcut(app: &AppHandle, was_open: bool) {
    let Some(window) = popover(app) else { return };
    if was_open {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            note_popover_hidden(app);
        }
        return;
    }
    app.state::<Arc<AppState>>().lock().popover_shortcut = ShortcutFocus::shown(Instant::now());
    show_popover(app);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHORTCUT_SETTLE);
        let refocus = app
            .state::<Arc<AppState>>()
            .lock()
            .popover_shortcut
            .refocus();
        if refocus
            && let Some(window) = popover(&app)
            && window.is_visible().unwrap_or(false)
        {
            let _ = window.set_focus();
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The flapping measured on the harness: in, out, in, out within a
    /// millisecond of first gaining the focus. All of it is ignored, and the
    /// focus is asked for again once it has settled.
    #[test]
    fn the_shortcuts_own_flapping_is_ignored_and_the_focus_asked_again() {
        let t0 = Instant::now();
        let mut f = ShortcutFocus::shown(t0);
        f.on_focus(t0 + ms(15));
        assert!(f.on_blur(t0 + ms(15)));
        f.on_focus(t0 + ms(16));
        assert!(f.on_blur(t0 + ms(16)));
        assert!(f.refocus());
    }

    /// A slow show: the window counts from the first focus, not from the
    /// shortcut, so flapping 200 ms after the shortcut is still flapping.
    #[test]
    fn a_slow_show_still_gets_the_whole_flap_window() {
        let t0 = Instant::now();
        let mut f = ShortcutFocus::shown(t0);
        f.on_focus(t0 + ms(200));
        assert!(f.on_blur(t0 + ms(210)));
    }

    /// ⛔ A person clicking elsewhere soon after the shortcut is a real blur:
    /// not ignored, and the focus is not pulled back afterwards (Seer, #91).
    #[test]
    fn a_real_blur_soon_after_is_not_ignored_and_the_focus_is_not_pulled_back() {
        let t0 = Instant::now();
        let mut f = ShortcutFocus::shown(t0);
        f.on_focus(t0 + ms(15));
        assert!(!f.on_blur(t0 + ms(15) + SHORTCUT_FLAP));
        assert!(!f.refocus());
    }

    /// A focus loss before any focus was gained, and any after the show has
    /// settled, is a blur like any other; so is one with no shortcut show.
    #[test]
    fn without_a_recent_shortcut_focus_a_blur_is_a_blur() {
        let t0 = Instant::now();
        let mut f = ShortcutFocus::shown(t0);
        assert!(!f.on_blur(t0 + ms(5)));
        let mut f = ShortcutFocus::shown(t0);
        f.on_focus(t0 + ms(10));
        assert!(!f.on_blur(t0 + SHORTCUT_SETTLE));
        let mut f = ShortcutFocus::default();
        f.on_focus(t0);
        assert!(!f.on_blur(t0));
        assert!(!f.refocus());
    }
}
