//! What the shell knows between ticks, and how the two halves ask each other
//! for work.
//!
//! Everything mutable lives behind one `std::sync::Mutex`. The critical
//! sections are a struct copy at most, so a synchronous lock held across no
//! await point is both simpler and cheaper than an async one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bridgewatch_core::config::Config;
use bridgewatch_core::poll::{POLL_NOW_MIN_GAP, PollNow};
use bridgewatch_core::verdict::Snapshot;
use serde::Serialize;
use tokio::sync::Notify;

use crate::config::Validation;

/// The mutable half.
pub struct Inner {
    /// The last snapshot published, which is what a newly opened window paints
    /// before the next tick arrives.
    pub snapshot: Snapshot,
    /// The last configuration that LOADED. A failed hot reload leaves this
    /// alone on purpose: a typo in the file must not stop the monitoring.
    pub config: Option<Config>,
    /// The result of the most recent load or reload, good or bad.
    pub validation: Validation,
    /// When the last tick completed.
    pub last_poll: Option<Instant>,
    /// When the next one is due, so the debug pane can count down.
    pub next_poll_at: Option<Instant>,
    /// True when this run created the config file from the shipped example.
    pub seeded: bool,
    /// A tab the settings window should open on, parked here because the menu
    /// can ask for one before that window's JavaScript is listening.
    pub settings_tab: Option<String>,
    /// When the popover was last hidden. See `windows::BLUR_HIDE_GRACE`.
    pub popover_hidden_at: Option<Instant>,
    /// The file's text as the last reload read it, so a reload of identical
    /// text can be skipped. See `commands::reload_from_disk`.
    pub disk_text: Option<String>,
}

/// Shared application state, reachable from every command and from the poller.
pub struct AppState {
    /// The file being watched and edited.
    pub config_path: PathBuf,
    /// The mutable half.
    inner: Mutex<Inner>,
    /// Raised to ask the poller to tick immediately.
    pub poll_now: Arc<Notify>,
    /// Raised to ask the poller to rebuild itself from the current config.
    pub reload: Arc<Notify>,
    /// The outstanding confirmation for a sensitive write, if any.
    pub confirmations: crate::guard::Confirmations,
}

/// The shell-level status the popover's debug pane and the settings banner
/// read. Distinct from [`Snapshot`], which is entirely the core's.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The effective config path, as resolved.
    pub config_path: String,
    /// True when this run wrote the example because nothing was there.
    pub seeded: bool,
    /// No file yet at the DEFAULT path: a first launch the setup wizard owns.
    /// Read live, so it clears the moment the wizard (or a text-tab Save)
    /// writes the file.
    pub first_run: bool,
    /// Whether the current file loads.
    pub config_ok: bool,
    /// Its diagnostics, errors and warnings together.
    pub diagnostics: Vec<crate::config::DiagnosticView>,
    /// Seconds since the last tick, or `None` before the first.
    pub since_last_poll_secs: Option<u64>,
    /// Seconds until the next tick, floored at zero.
    pub next_poll_secs: Option<u64>,
    /// Whether the OS reports bridgewatch as a login item.
    pub launch_at_login: bool,
    /// `macos`, `linux`, ... so the frontend can say the right thing about
    /// left-clicking a tray icon.
    pub platform: String,
    /// True when the popover has a native vibrancy material behind it (macOS).
    pub vibrancy: bool,
}

impl AppState {
    /// Build the shared state around a resolved configuration.
    pub fn new(resolved: crate::config::Resolved) -> Self {
        Self {
            config_path: resolved.path,
            inner: Mutex::new(Inner {
                snapshot: Snapshot::empty(),
                config: resolved.loaded.map(|l| l.config),
                validation: resolved.validation,
                last_poll: None,
                next_poll_at: None,
                seeded: resolved.seeded,
                settings_tab: None,
                popover_hidden_at: None,
                disk_text: resolved.text,
            }),
            poll_now: Arc::new(Notify::new()),
            reload: Arc::new(Notify::new()),
            confirmations: Default::default(),
        }
    }

    /// Record the file's text as a reload read it. Returns whether the reload
    /// should go ahead: always when `force`d, otherwise only when the text
    /// differs from what the last reload saw.
    pub fn note_disk_text(&self, raw: &str, force: bool) -> bool {
        let mut inner = self.lock();
        if !force && inner.disk_text.as_deref() == Some(raw) {
            return false;
        }
        inner.disk_text = Some(raw.to_string());
        true
    }

    /// Lock the mutable half.
    ///
    /// ⛔ Poison is recovered from, not propagated. Every critical section here
    /// is a field assignment or a clone, so a panic elsewhere while the lock
    /// was held cannot leave `Inner` half-written; but a poisoned lock that
    /// every later caller `expect`s turns ONE panic in the poller into a panic
    /// in every command, the tray menu and the next poller, permanently.
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ask for an out-of-band poll, honouring the core's rate gate.
    ///
    /// Returns whether the request was passed on. `PollNow::PopoverOpened` is
    /// the one that is dropped when it arrives too soon: opening and closing
    /// the popover is a thing people do idly, and it must not become a request
    /// storm. A manual refresh, a wake and a config change all bypass it,
    /// which is [`PollNow::bypasses_gate`]'s judgement, not this file's.
    pub fn request_poll(&self, reason: PollNow) -> bool {
        let allowed = {
            let inner = self.lock();
            reason.bypasses_gate()
                || inner
                    .last_poll
                    .is_none_or(|t| t.elapsed() >= POLL_NOW_MIN_GAP)
        };
        if allowed {
            self.poll_now.notify_one();
        }
        allowed
    }

    /// Ask the poller to drop itself and rebuild from the current config.
    pub fn request_reload(&self) {
        self.reload.notify_one();
    }

    /// The current shell status.
    pub fn status(&self, launch_at_login: bool) -> Status {
        let inner = self.lock();
        Status {
            config_path: self.config_path.display().to_string(),
            seeded: inner.seeded,
            first_run: self.config_path == bridgewatch_core::config::default_path()
                && !self.config_path.exists(),
            config_ok: inner.validation.ok,
            diagnostics: inner.validation.diagnostics.clone(),
            since_last_poll_secs: inner.last_poll.map(|t| t.elapsed().as_secs()),
            next_poll_secs: inner
                .next_poll_at
                .map(|t| t.saturating_duration_since(Instant::now()).as_secs()),
            launch_at_login,
            platform: std::env::consts::OS.to_string(),
            vibrancy: crate::windows::vibrancy(),
        }
    }

    /// Record a completed tick.
    pub fn record_tick(&self, snapshot: Snapshot, next_interval: Duration) {
        let mut inner = self.lock();
        inner.snapshot = snapshot;
        let now = Instant::now();
        inner.last_poll = Some(now);
        // `checked_add`: an interval derived from a server's `Retry-After` is
        // not something to trust with a panic.
        inner.next_poll_at = now.checked_add(next_interval);
    }

    /// The latest snapshot, for first paint, with the configuration's own
    /// complaints on top while the file does not load.
    pub fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        crate::poller::with_config_errors(inner.snapshot.clone(), &inner.validation)
    }

    /// The last configuration that loaded.
    pub fn config(&self) -> Option<Config> {
        self.lock().config.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(text: &str) -> AppState {
        AppState::new(crate::config::Resolved {
            path: PathBuf::from("/nonexistent/config.toml"),
            seeded: false,
            first_run: false,
            loaded: None,
            validation: Validation::default(),
            text: Some(text.to_string()),
        })
    }

    #[test]
    fn the_watcher_seeing_a_save_this_process_made_does_not_rebuild_again() {
        // M4: a GUI save reloads once itself, then the file watcher sees the
        // same bytes land and used to rebuild the poller a second time.
        let state = state_with("a");
        assert!(
            state.note_disk_text("b", false),
            "a real change is a reload"
        );
        assert!(
            !state.note_disk_text("b", false),
            "the watcher's echo was not skipped"
        );
        assert!(
            state.note_disk_text("b", true),
            "Reload config must always rebuild"
        );
        assert!(!state.note_disk_text("b", false));
    }

    #[test]
    fn an_idle_popover_open_is_gated_and_a_manual_refresh_is_not() {
        let state = state_with("");
        state.record_tick(Snapshot::empty(), Duration::from_secs(60));
        assert!(!state.request_poll(PollNow::PopoverOpened));
        assert!(state.request_poll(PollNow::Manual));
    }

    #[test]
    fn a_huge_retry_after_cannot_panic_the_tick_bookkeeping() {
        let state = state_with("");
        state.record_tick(Snapshot::empty(), Duration::MAX);
        assert!(state.status(false).next_poll_secs.is_none());
    }
}
