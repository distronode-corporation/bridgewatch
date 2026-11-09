//! The background task: build a [`Poller`], tick it, and push what comes out at
//! the tray, the windows and the OS notification centre.
//!
//! The core's `Poller::run` loop is not used here. It sleeps the interval
//! itself, and this shell has to be able to cut that sleep short for a manual
//! refresh, a popover open or a configuration change — so the loop is driven a
//! tick at a time and the sleep is a `select!` arm.
//!
//! A wake from sleep cuts it short too ([`WakeWatch`]): the sleep runs on the
//! monotonic clock, which stops while the machine sleeps, so without it a
//! laptop opened after an hour kept its hour-old view until the rest of the
//! interval had run.

use std::sync::Arc;
use std::time::Duration;

use bridgewatch_core::eta::EtaHistory;
use bridgewatch_core::notify::{Notification, NotifyLedger};
use bridgewatch_core::poll::{Poller, WakeWatch};
use bridgewatch_core::verdict::{IconState, Snapshot};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::{ActionPerformed, NotificationExt};

use crate::config::Validation;
use crate::state::AppState;

/// The event every window listens to for a new frame.
pub const SNAPSHOT_EVENT: &str = "snapshot";

/// How long to wait before trying to build the poller again after it could
/// not be built (almost always: a token could not be resolved).
///
/// ⛔ There used to be no retry at all: a failed build waited for a config
/// reload and nothing else. A Linux login start that runs before the Secret
/// Service is up, or one dismissed Keychain prompt, therefore meant `unknown`
/// until the user happened to edit the file, and "Refresh now" did nothing.
/// Now it retries on its own, backing off, and a refresh or a reload retries
/// at once.
pub fn build_retry_delay(attempt: u32) -> Duration {
    const FIRST: u64 = 5;
    const CAP: u64 = 300;
    Duration::from_secs(FIRST.saturating_mul(1u64 << attempt.min(16)).min(CAP))
}

/// Run until the process exits, restarting the poll loop if it panics.
///
/// ⛔ A panic anywhere in the loop used to end the task for good: the tray kept
/// its last icon, "Refresh now" and "Reload" notified a task that no longer
/// existed, and nothing said so. The loop now runs as a child task, and a
/// crash is reported in the popover and followed by a fresh loop.
pub async fn run(app: AppHandle, state: Arc<AppState>) {
    let crash_app = app.clone();
    let crash_state = state.clone();
    supervise(
        move || run_loop(app.clone(), state.clone()),
        move |message| {
            tracing::error!(%message, "the poll loop crashed; restarting it");
            set_icon(&crash_app, IconState::Unknown);
            let mut snapshot = crash_state.lock().snapshot.clone();
            snapshot.errors.push(format!(
                "the poll loop crashed and was restarted: {message}"
            ));
            publish(&crash_app, &crash_state, snapshot);
        },
        CRASH_RESTART_DELAY,
    )
    .await;
}

/// Pause between a crash and the restart, so a loop that panics at once does
/// not spin.
pub const CRASH_RESTART_DELAY: Duration = Duration::from_secs(5);

/// Run `make()` as a task; when it panics, report it and run it again. Returns
/// when a run ends normally.
pub async fn supervise<F, Fut>(make: F, on_crash: impl Fn(String), delay: Duration)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    loop {
        match tokio::spawn(make()).await {
            Ok(()) => return,
            Err(e) => {
                let message = match e.try_into_panic() {
                    Ok(payload) => payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic".to_string()),
                    Err(e) => e.to_string(),
                };
                on_crash(message);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// The poll loop proper.
///
/// The outer loop owns poller construction, so a configuration reload is
/// "break the inner loop and build a new one". A `Poller` is cheap — the
/// expensive parts are the HTTP client and the compiled patterns, both of which
/// have to be rebuilt anyway when the config changes.
async fn run_loop(app: AppHandle, state: Arc<AppState>) {
    let mut failed_builds: u32 = 0;
    let mut wake = WakeWatch::new();
    loop {
        let Some(config) = state.config() else {
            // No usable configuration. Show it in the tray and wait to be told
            // something changed rather than spinning.
            set_icon(&app, IconState::Unknown);
            publish(&app, &state, Snapshot::empty());
            state.set_clients(Default::default());
            state.set_refused(Vec::new());
            state.reload.notified().await;
            continue;
        };

        // ⚠ `Poller::from_config` resolves the token of every account a watch
        // uses, which on macOS can raise a Keychain prompt. That is why this
        // runs here, on the background task after the app is up, and not in
        // `setup`: the core's README asks for resolution to happen in response
        // to a user action having started the app, never on a timer with
        // nobody watching.
        // ⛔ Per account (D4): one account whose token is missing or bound to
        // another server fails ITS watches, with the reason on them, and the
        // others keep polling. Only a poller that cannot be built at all
        // (a transport, the configuration) lands in the arm below.
        // ⛔ Through `state.approvals`, never the bare system store: an entry
        // whose prompt was cancelled is not asked about again by this build,
        // nor by any rebuild after a reload, until a person says so.
        let mut poller = match Poller::from_config_per_account(&config, &state.approvals) {
            Ok(p) => {
                failed_builds = 0;
                state.set_clients(p.clients().clone());
                state.set_refused(p.refused_accounts().to_vec());
                p.with_ledger(NotifyLedger::default_path())
                    .with_eta_history(EtaHistory::default_path())
                    .with_commit_titles(config.ui.commit_titles)
            }
            Err(e) => {
                state.set_clients(Default::default());
                state.set_refused(Vec::new());
                let delay = build_retry_delay(failed_builds);
                failed_builds = failed_builds.saturating_add(1);
                tracing::error!(error = %e, retry_in = ?delay, "poller could not be built");
                set_icon(&app, IconState::Unknown);
                let mut snapshot = Snapshot::empty();
                snapshot.errors.push(format!(
                    "cannot start polling: {e} (retrying in {}s, or use Refresh now)",
                    delay.as_secs()
                ));
                publish(&app, &state, snapshot);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = state.poll_now.notified() => {}
                    _ = state.reload.notified() => {}
                    jump = wake.woke() => log_wake(jump),
                }
                continue;
            }
        };

        // ⛔ A refused account is retried IN PLACE (`Poller::retry_refused`),
        // never by rebuilding the poller: a rebuild re-created every watch's
        // state, so each attempt wiped the healthy accounts' backoff, cache
        // and last view. Some causes pass on their own (a keychain that was
        // locked, a Secret Service not up yet at login, a command that timed
        // out), so the attempts follow `build_retry_delay` while any account
        // is refused, and a token save or a Refresh retries at once.
        let mut retries: u32 = 0;
        let mut next_retry = tokio::time::Instant::now() + build_retry_delay(retries);

        loop {
            // A wake before this tick is answered by it.
            wake.reset();
            let tick = poller.tick().await;
            // An account read for the first time by this tick (a sign-in) may
            // have just been refused; the popover's strip follows on its own
            // (`Status`), and the timed retry below leaves it out.

            set_icon(&app, tick.snapshot.icon_state);
            state.record_tick(tick.next_interval);
            publish(&app, &state, tick.snapshot);
            // A pause, or a burst folded into one: see `quiet`.
            let outgoing = state.with_quiet(|q| q.outgoing(chrono::Utc::now(), tick.notifications));
            deliver(&app, &outgoing);
            // A timed pause that ran out says so in the menu by the next tick.
            crate::tray::sync_pause(&app);

            let tick_at = tokio::time::Instant::now() + tick.next_interval;
            let reload = loop {
                let refused = has_timed_retry(
                    poller.refused_accounts(),
                    &state.needs_approval(Some(&config)),
                );
                tokio::select! {
                    _ = tokio::time::sleep_until(tick_at) => break false,
                    _ = state.poll_now.notified() => break false,
                    _ = state.reload.notified() => break true,
                    // Woke from sleep (or the clock was set forward): poll at
                    // once, past the rate gate, as `PollNow::Wake` allows.
                    jump = wake.woke() => {
                        log_wake(jump);
                        break false;
                    }
                    _ = state.retry_refused.notified() => {
                        // A recovery ticks at once; nothing recovered waits on.
                        if retry_refused(&mut poller, &state) {
                            break false;
                        }
                    }
                    _ = tokio::time::sleep_until(next_retry), if refused => {
                        retries = retries.saturating_add(1);
                        next_retry = tokio::time::Instant::now() + build_retry_delay(retries);
                        if retry_refused(&mut poller, &state) {
                            break false;
                        }
                    }
                }
            };
            if reload {
                break;
            }
        }
    }
}

/// Say in the log why a poll came early.
fn log_wake(jump: Duration) {
    tracing::info!(
        slept_secs = jump.as_secs(),
        "the wall clock ran ahead of the monotonic clock (a wake from sleep); polling now"
    );
}

/// Whether any refused account is due a timed retry: one refused for any
/// reason but a cancelled or denied prompt.
///
/// ⛔ An account waiting for approval is never retried on a timer. The retry
/// read it again, and on macOS every read of an item whose dialog was
/// cancelled is a new dialog: a Cancel raised another one 5, 10 and 20
/// seconds later. It waits for Retry (or Refresh); a locked keychain, a
/// Secret Service not up yet and a command that timed out keep the backoff.
pub fn has_timed_retry(refused: &[String], needs_approval: &[String]) -> bool {
    refused.iter().any(|a| !needs_approval.contains(a))
}

/// One in-place retry of the poller's refused accounts, and the shell's view
/// of the result. Returns whether any account recovered.
///
/// ⚠ Blocking for as long as the credential reads take, like building the
/// poller on this task is.
fn retry_refused(poller: &mut Poller, state: &AppState) -> bool {
    if poller.refused_accounts().is_empty() {
        return false;
    }
    let recovered = poller.retry_refused();
    state.set_clients(poller.clients().clone());
    state.set_refused(poller.refused_accounts().to_vec());
    !recovered.is_empty()
}

/// The configuration's own complaints, as error-strip lines. Empty when the
/// file loads.
pub fn config_errors(validation: &Validation) -> Vec<String> {
    if validation.ok {
        return Vec::new();
    }
    let mut out: Vec<String> = validation
        .diagnostics
        .iter()
        .filter(|d| d.severity == "error")
        .map(|d| match (d.line, &d.path) {
            (Some(line), p) if !p.is_empty() => format!("{p} (line {line}): {}", d.message),
            (Some(line), _) => format!("line {line}: {}", d.message),
            (None, p) if !p.is_empty() => format!("{p}: {}", d.message),
            _ => d.message.clone(),
        })
        .collect();
    if out.is_empty() {
        out.push("the configuration file does not load; open Settings to fix it".to_string());
    }
    out
}

/// Put the configuration's complaints at the top of a snapshot's errors.
///
/// ⛔ Applied to EVERY published snapshot, not once. The strip used to be
/// filled by one republish after a failed reload, and the next tick (which
/// runs on the last good configuration and knows nothing about the file)
/// replaced it: sixty seconds after a bad save the popover said nothing was
/// wrong while the file on disk would not load.
pub fn with_config_errors(mut snapshot: Snapshot, validation: &Validation) -> Snapshot {
    let mut errors = config_errors(validation);
    for e in snapshot.errors.drain(..) {
        if !errors.contains(&e) {
            errors.push(e);
        }
    }
    snapshot.errors = errors;
    snapshot
}

/// Publish a snapshot to every window and remember it for first paint.
fn publish(app: &AppHandle, state: &AppState, snapshot: Snapshot) {
    // The stored copy stays RAW (what the poller said); the configuration's
    // complaints are merged on the way out, here and in `AppState::snapshot`,
    // so a fixed file cannot leave a stale line behind.
    let snapshot = {
        let mut inner = state.lock();
        inner.snapshot = snapshot.clone();
        with_config_errors(snapshot, &inner.validation)
    };
    if let Err(e) = app.emit(SNAPSHOT_EVENT, &snapshot) {
        tracing::warn!(error = %e, "could not emit a snapshot");
    }
}

/// Swap the tray image for the state's glyph.
///
/// The icon config is read fresh each time so that editing `[icon]` in the file
/// takes effect on the next tick without a restart.
pub fn set_icon(app: &AppHandle, icon_state: IconState) {
    let icon_config = app
        .state::<Arc<AppState>>()
        .config()
        .map(|c| c.icon)
        .unwrap_or_default();
    let drawable = crate::icons::drawable_for(&icon_config, icon_state);
    if let Some(symbol) = &drawable.fell_back_from {
        tracing::warn!(
            symbol,
            state = icon_state.as_str(),
            "tray glyph does not draw; showing the unknown glyph"
        );
    }
    let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) else {
        tracing::warn!("the tray icon is gone; nothing to set");
        return;
    };
    set_tooltip(&tray, icon_state);
    apply_icon(&tray, &LAST_ICON, drawable, std::time::Instant::now());
}

/// The state the tooltip currently names.
///
/// ⚠ Deliberately not folded into [`IconMemo`]: two states can share one image,
/// because a glyph that does not draw falls back to the unknown one, and the
/// tooltip has to follow the STATE rather than the picture.
#[cfg(not(target_os = "linux"))]
static LAST_TOOLTIP: std::sync::Mutex<Option<IconState>> = std::sync::Mutex::new(None);

/// Name the current state in the tray's tooltip: "bridgewatch: deployed".
///
/// Cheap because it runs where the icon is swapped, so it costs a main-thread
/// hop per genuine state change rather than one per tick.
///
/// ⛔ Nothing on Linux: tauri 2.11.5 documents `set_tooltip` as "**Linux:**
/// Unsupported" and tray-icon 0.24.2's GTK backend is a bare `Ok(())`, so the
/// call would change nothing. Linux has no lever at all for this; the doc on
/// `TRAY_LABEL` in tray.rs says why.
#[cfg(not(target_os = "linux"))]
fn set_tooltip(tray: &tauri::tray::TrayIcon, icon_state: IconState) {
    let mut last = LAST_TOOLTIP.lock().unwrap_or_else(|e| e.into_inner());
    if *last == Some(icon_state) {
        return;
    }
    match tray.set_tooltip(Some(format!("bridgewatch: {icon_state}"))) {
        // Not remembered on failure, so the next state change tries again.
        Ok(()) => *last = Some(icon_state),
        Err(e) => tracing::warn!(error = %e, "could not set the tray tooltip"),
    }
}

#[cfg(target_os = "linux")]
fn set_tooltip(_tray: &tauri::tray::TrayIcon, _icon_state: IconState) {}

/// Where a tray image goes. The real one is the Tauri tray; tests use a fake.
pub trait TraySink {
    /// Set the image and its template flag in ONE step.
    fn put(&self, image: tauri::image::Image<'static>, template: bool) -> Result<(), String>;
}

impl TraySink for tauri::tray::TrayIcon {
    fn put(&self, image: tauri::image::Image<'static>, template: bool) -> Result<(), String> {
        // ⛔ One call, never `set_icon` then `set_icon_as_template`. On macOS
        // tray-icon 0.24's `set_icon` installs the new NSImage with template
        // OFF, and the flag is only put back by a second main-thread call that
        // re-sets the same NSImage. The builtin template glyphs are pure black
        // on transparent, so between the two calls (and for as long as AppKit
        // does not redraw after the second) the icon is black on a dark menu
        // bar: present, clickable and invisible. This is the most likely cause
        // of the icon vanishing for ~20 minutes on 2026-09-18 (not reproduced:
        // the app is not run by the lanes). The atomic setter builds the image
        // with the flag before AppKit sees it; on Linux it is `set_icon`.
        self.set_icon_with_as_template(Some(image), template)
            .map_err(|e| e.to_string())
    }
}

/// Hand `drawable` to the tray unless it is the image already showing.
/// Returns whether the tray was asked to change.
pub fn apply_icon(
    sink: &impl TraySink,
    memo: &IconMemo,
    drawable: crate::icons::Drawable,
    now: std::time::Instant,
) -> bool {
    let key = IconMemo::key(&drawable);
    // ⚠ Skip an unchanged image. On Linux every set writes a new PNG file
    // (tray-icon 0.24 deletes the previous one), so an unconditional call per
    // tick was a file written and deleted every few seconds for nothing. The
    // memo expires, though, so the image is re-asserted now and then: if the
    // OS ever loses it, the loss is bounded.
    if memo.is_current(key, now) {
        return false;
    }
    match sink.put(drawable.image, drawable.template) {
        Ok(()) => memo.remember(key, now),
        Err(e) => {
            // Not remembered, so the next tick tries again rather than
            // believing a set that never happened.
            memo.forget();
            tracing::warn!(error = %e, "could not set the tray icon");
        }
    }
    true
}

/// The image last handed to the tray.
static LAST_ICON: IconMemo = IconMemo::new();

/// How long an unchanged tray image is trusted before it is set again.
pub const ICON_REASSERT: Duration = Duration::from_secs(300);

/// Remembers the last tray image, so an identical one is not set again.
pub struct IconMemo(std::sync::Mutex<Option<((u64, bool), std::time::Instant)>>);

impl IconMemo {
    /// Nothing remembered yet.
    pub const fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }

    /// What identifies an image: its pixels, size and template flag.
    pub fn key(d: &crate::icons::Drawable) -> (u64, bool) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        d.image.rgba().hash(&mut h);
        (d.image.width(), d.image.height()).hash(&mut h);
        (h.finish(), d.template)
    }

    /// True when `key` is the image set, and it was set within
    /// [`ICON_REASSERT`] of `now`.
    pub fn is_current(&self, key: (u64, bool), now: std::time::Instant) -> bool {
        let last = self.0.lock().unwrap_or_else(|e| e.into_inner());
        matches!(*last, Some((k, at)) if k == key && now.saturating_duration_since(at) < ICON_REASSERT)
    }

    /// Record a successful set.
    pub fn remember(&self, key: (u64, bool), now: std::time::Instant) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, now));
    }

    /// Forget, so the next image is set whatever it is.
    pub fn forget(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The key a notification's click target travels under, in the plugin's
/// `extra` payload, which every desktop platform hands back on a click.
pub const CLICK_URL_KEY: &str = "bridgewatch_url";

/// The action type a notification with a click target carries on macOS: one
/// action, [`OPEN_ACTION`], registered at startup (`register_click_action`).
pub const OPEN_ACTION_TYPE: &str = "bridgewatch-open";

/// The one action of [`OPEN_ACTION_TYPE`]: the banner's "Open" button.
pub const OPEN_ACTION: &str = "open";

/// Whether a notification with a click target carries [`OPEN_ACTION_TYPE`].
///
/// ⛔ macOS only, and not decoration: without an action, no click on a
/// notification is ever delivered there. `tauri-plugin-notification` 2.8.0
/// waits for the response with notify-rust 4.18.2's `wait_for_response`
/// (`desktop.rs`, `fn wait`), which sends the notification through
/// mac-notification-sys 0.6.15 with `asynchronous(false)` and never calls
/// `wait_for_click(true)` (it is commented out in
/// `macos/nsusernotifications.rs`). mac-notification-sys waits only when the
/// notification `needs_response()`: a main button, a close button or
/// `wait_for_click` (`notification.rs`). A notification with none of the three
/// is sent fire-and-forget: `send` returns `None` once it is delivered, the
/// plugin's handler sees `Closed(Expired)` and reports nothing, and the click
/// that comes later reaches a delegate with no one waiting for it
/// (`rust_notification_activated` finds no pending entry). One action is a
/// main button, so the plugin's thread waits, and a click on the banner
/// (`Click`, reported as `tap`) or on "Open" comes back to
/// [`on_notification_action`].
///
/// Linux reports a click through the D-Bus `default` action, which the plugin
/// adds on its own, and an action would add a button there for nothing;
/// Windows reports a click without one.
pub const fn carries_open_action() -> bool {
    cfg!(target_os = "macos")
}

/// Register [`OPEN_ACTION_TYPE`], once, at startup, where
/// [`carries_open_action`].
pub fn register_click_action<R: tauri::Runtime>(
    notifier: &tauri_plugin_notification::Notification<R>,
) {
    if !carries_open_action() {
        return;
    }
    let open = tauri_plugin_notification::ActionType::builder(OPEN_ACTION_TYPE)
        .actions(vec![
            tauri_plugin_notification::Action::builder(OPEN_ACTION, "Open").build(),
        ])
        .build();
    if let Err(e) = notifier.register_action_types(vec![open]) {
        tracing::warn!(error = %e, "could not register the notification's Open action");
    }
}

/// The environment variable that makes the app click its own notifications,
/// for a harness that cannot click (see [`simulated_click`]).
pub const SIMULATE_CLICK_ENV: &str = "BRIDGEWATCH_SIMULATE_NOTIFICATION_CLICK";

/// How long after showing a notification the simulated click comes, so the
/// banner is on screen when it does.
pub const SIMULATED_CLICK_DELAY: Duration = Duration::from_secs(2);

/// Hand a tick's notifications to the OS.
///
/// The core has already decided what is worth interrupting somebody over, has
/// rendered the templates and has deduplicated against its ledger. There is
/// nothing to decide here; only to deliver.
///
/// The click target (`n.url`, from the watch's `notify.click`) rides in the
/// notification's `extra` and comes back to [`on_notification_action`] when
/// the notification is clicked. `tauri-plugin-notification` reports a click on
/// desktop since 2.6.0 (`Notification::on_action`, action `tap`): on Linux and
/// the BSDs through the D-Bus `default` action, on macOS and Windows through
/// notify-rust's `wait_for_response`, which on macOS needs the notification to
/// carry an action ([`carries_open_action`]). ⚠️ Windows reports it only while
/// the toast is on screen, not from the Action Center, and a Linux
/// notification server that does not offer actions (`notify-send --action`
/// support) reports nothing; in both cases the same link is one click away in
/// the popover.
fn deliver(app: &AppHandle, notifications: &[Notification]) {
    let simulate = std::env::var_os(SIMULATE_CLICK_ENV).is_some_and(|v| v == "1");
    for n in notifications {
        let mut builder = app.notification().builder().title(&n.title);
        if !n.body.is_empty() {
            builder = builder.body(&n.body);
        }
        if let Some(url) = &n.url {
            builder = builder.extra(CLICK_URL_KEY, url);
            if carries_open_action() {
                builder = builder.action_type_id(OPEN_ACTION_TYPE);
            }
        }
        if let Err(e) = builder.show() {
            tracing::warn!(error = %e, watch = %n.watch, "could not show a notification");
            continue;
        }
        if simulate && let Some(performed) = n.url.as_deref().and_then(simulated_click) {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(SIMULATED_CLICK_DELAY);
                tracing::info!(
                    env = SIMULATE_CLICK_ENV,
                    "simulating a click on the notification just shown"
                );
                on_notification_action(&app, &performed);
            });
        }
    }
}

/// What the plugin hands [`on_notification_action`] for a click (`tap`) on a
/// notification [`deliver`] attached `url` to: `{ actionId, inputValue,
/// notification }`, where `notification` is the builder's `NotificationData`
/// serialised whole (camelCase, `extra` included). Written out by hand from
/// `tauri-plugin-notification` 2.8.0's `desktop.rs` (`performed`) and
/// `models.rs` (`NotificationData`), because neither is constructible outside
/// the crate.
///
/// ⚠️ For [`SIMULATE_CLICK_ENV`] and the tests: it runs the SAME handler a
/// real click runs, from the same payload, so it proves the routing and the
/// opening, and NOT that macOS delivered a click.
pub fn simulated_click(url: &str) -> Option<ActionPerformed> {
    performed_payload("tap", serde_json::json!({ CLICK_URL_KEY: url }))
}

/// The plugin's `ActionPerformed` for `action`, carrying `extra`.
fn performed_payload(action: &str, extra: serde_json::Value) -> Option<ActionPerformed> {
    serde_json::from_value(serde_json::json!({
        "actionId": action,
        "inputValue": null,
        "notification": {
            "id": 1_234_567,
            "channelId": null,
            "title": "bridgewatch",
            "body": null,
            "schedule": null,
            "largeBody": null,
            "summary": null,
            "actionTypeId": null,
            "group": null,
            "groupSummary": false,
            "sound": null,
            "inboxLines": [],
            "icon": null,
            "largeIcon": null,
            "iconColor": null,
            "attachments": [],
            "extra": extra,
            "ongoing": false,
            "autoCancel": false,
            "silent": false,
            "when": null,
            "usesChronometer": false,
            "chronometerCountDown": false,
        },
    }))
    .ok()
}

/// The URL a performed notification action should open: the click target
/// [`deliver`] attached, for a click on the notification itself (`tap`) or
/// on its "Open" button ([`OPEN_ACTION`], macOS). Anything else (another
/// action, a notification without a target, a payload the platform
/// described differently) opens nothing.
pub fn click_url_of(performed: &ActionPerformed) -> Option<&str> {
    if !matches!(performed.action_id(), "tap" | OPEN_ACTION) {
        return None;
    }
    performed
        .notification()?
        .extra()
        .get(CLICK_URL_KEY)?
        .as_str()
        .filter(|u| !u.trim().is_empty())
}

/// A notification was clicked: open its target, through the same door as
/// every other link (`links::open`, which refuses a host that is not a
/// configured account's). Runs on the plugin's own thread.
///
/// ⛔ One info line every time it fires with a target, naming the source and
/// the URL: the iMac's harness proves a click by this line, with no person
/// clicking. A job URL is not a secret, and `links::open` refuses one that
/// carries credentials.
pub fn on_notification_action(app: &AppHandle, performed: &ActionPerformed) {
    let Some(url) = click_url_of(performed) else {
        tracing::debug!(
            action = performed.action_id(),
            "a notification action with nothing to open"
        );
        return;
    };
    tracing::info!(
        source = "notification click",
        action = performed.action_id(),
        url,
        "opening a notification's link"
    );
    let config = app.state::<Arc<AppState>>().config();
    if let Err(e) = crate::links::open(app, url, config.as_ref()) {
        tracing::warn!(error = %e, "a notification's link was not opened");
    }
}

/// Push the current configuration diagnostics out to the windows without
/// waiting for a tick.
///
/// Used when a reload FAILED: the poller keeps running on the last good
/// configuration, so no tick is coming soon. The merge itself happens in
/// `publish`, which is what keeps the strip up on every later tick too.
pub fn republish_errors(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let raw = state.lock().snapshot.clone();
    publish(app, &state, raw);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DiagnosticView;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn performed(action: &str, extra: serde_json::Value) -> ActionPerformed {
        performed_payload(action, extra).expect("the plugin's payload decodes")
    }

    #[test]
    fn a_click_on_a_notification_opens_the_url_deliver_attached() {
        let url = "https://gitlab.example.com/acme/app/-/jobs/42";
        let tapped = performed("tap", serde_json::json!({ CLICK_URL_KEY: url }));
        assert_eq!(click_url_of(&tapped), Some(url));
    }

    /// macOS: a click on the banner's "Open" button opens the same target as
    /// a click on the banner itself.
    #[test]
    fn the_open_button_opens_the_same_url_as_a_click() {
        let url = "https://gitlab.example.com/acme/app/-/jobs/42";
        let open = performed(OPEN_ACTION, serde_json::json!({ CLICK_URL_KEY: url }));
        assert_eq!(click_url_of(&open), Some(url));
    }

    /// The simulated click (`BRIDGEWATCH_SIMULATE_NOTIFICATION_CLICK`) is the
    /// payload a real click hands the handler, so it routes the same way.
    #[test]
    fn a_simulated_click_routes_exactly_as_a_real_one() {
        let url = "https://gitlab.example.com/acme/app/-/jobs/42";
        let simulated = simulated_click(url).expect("the payload decodes");
        assert_eq!(simulated.action_id(), "tap");
        assert_eq!(click_url_of(&simulated), Some(url));
    }

    /// ⛔ The action that makes a macOS click reach the app at all is there
    /// on macOS and nowhere else (a Linux banner would grow a button).
    #[test]
    fn only_macos_notifications_carry_the_open_action() {
        assert_eq!(carries_open_action(), cfg!(target_os = "macos"));
    }

    /// A refused account waits for a person only when its prompt was
    /// refused; any other refusal keeps the timed retry.
    #[test]
    fn an_account_waiting_for_approval_is_never_retried_on_a_timer() {
        let refused = vec!["gl".to_string(), "gh".to_string()];
        assert!(
            !has_timed_retry(&refused, &refused),
            "both wait for approval"
        );
        assert!(
            has_timed_retry(&refused, &["gl".to_string()]),
            "gh has another cause"
        );
        assert!(has_timed_retry(&refused, &[]));
        assert!(!has_timed_retry(&[], &[]));
    }

    #[test]
    fn nothing_opens_for_an_action_button_a_missing_target_or_an_unreadable_payload() {
        let url = "https://gitlab.example.com/acme/app/-/jobs/42";
        let button = performed("snooze", serde_json::json!({ CLICK_URL_KEY: url }));
        assert_eq!(
            click_url_of(&button),
            None,
            "only a click on the notification itself"
        );
        let untargeted = performed("tap", serde_json::json!({}));
        assert_eq!(
            click_url_of(&untargeted),
            None,
            "click = \"none\" attaches nothing"
        );
        let blank = performed("tap", serde_json::json!({ CLICK_URL_KEY: "  " }));
        assert_eq!(click_url_of(&blank), None);
        let not_a_string = performed("tap", serde_json::json!({ CLICK_URL_KEY: 42 }));
        assert_eq!(click_url_of(&not_a_string), None);
        let unreadable: ActionPerformed = serde_json::from_value(serde_json::json!({
            "actionId": "tap", "inputValue": null, "notification": { "id": "not a number" },
        }))
        .unwrap();
        assert_eq!(click_url_of(&unreadable), None);
    }

    fn broken() -> Validation {
        Validation {
            ok: false,
            diagnostics: vec![
                DiagnosticView {
                    severity: "error".into(),
                    path: "poll.idle_secs".into(),
                    message: "expected an integer".into(),
                    line: Some(4),
                    col: Some(12),
                },
                DiagnosticView {
                    severity: "warning".into(),
                    path: "x".into(),
                    message: "a warning is not strip material".into(),
                    line: None,
                    col: None,
                },
            ],
            ..Validation::default()
        }
    }

    #[test]
    fn every_tick_of_a_broken_file_still_carries_its_errors() {
        // H9: the snapshot a tick produces knows nothing about the file. Merged
        // at publish time, a SECOND tick keeps the strip up as well as the first.
        let v = broken();
        for tick in 0..2 {
            let mut from_poller = Snapshot::empty();
            from_poller
                .errors
                .push(format!("watch main: 503 on tick {tick}"));
            let out = with_config_errors(from_poller, &v);
            assert_eq!(
                out.errors[0], "poll.idle_secs (line 4): expected an integer",
                "{out:?}"
            );
            assert_eq!(out.errors.len(), 2, "{:?}", out.errors);
        }
    }

    #[test]
    fn a_loading_file_adds_nothing_and_nothing_is_doubled() {
        let mut s = Snapshot::empty();
        s.errors.push("watch main: 503".into());
        let ok = Validation {
            ok: true,
            ..Validation::default()
        };
        assert_eq!(with_config_errors(s.clone(), &ok).errors, s.errors);

        let once = with_config_errors(s, &broken());
        let twice = with_config_errors(once.clone(), &broken());
        assert_eq!(once.errors, twice.errors);
    }

    /// Records every image handed to it; fails while `fail` is set.
    #[derive(Default)]
    struct FakeTray {
        puts: std::sync::Mutex<Vec<bool>>,
        fail: std::sync::atomic::AtomicBool,
    }

    impl TraySink for FakeTray {
        fn put(&self, image: tauri::image::Image<'static>, template: bool) -> Result<(), String> {
            assert!(
                image.rgba().as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "invisible image set"
            );
            if self.fail.load(Ordering::SeqCst) {
                return Err("main thread gone".into());
            }
            self.puts.lock().unwrap().push(template);
            Ok(())
        }
    }

    fn drawable(state: IconState) -> crate::icons::Drawable {
        let c = bridgewatch_core::config::IconConfig {
            mode: "template".into(),
            ..Default::default()
        };
        crate::icons::drawable_for(&c, state)
    }

    #[test]
    fn an_unchanged_tray_image_is_not_set_again_until_it_expires() {
        // Linux writes a PNG file per set; a steady state must write none,
        // but the image is re-asserted after ICON_REASSERT.
        let tray = FakeTray::default();
        let memo = IconMemo::new();
        let t0 = std::time::Instant::now();
        assert!(apply_icon(&tray, &memo, drawable(IconState::Running), t0));
        assert!(!apply_icon(
            &tray,
            &memo,
            drawable(IconState::Running),
            t0 + Duration::from_secs(5)
        ));
        assert!(apply_icon(
            &tray,
            &memo,
            drawable(IconState::Failed),
            t0 + Duration::from_secs(6)
        ));
        assert!(apply_icon(
            &tray,
            &memo,
            drawable(IconState::Failed),
            t0 + ICON_REASSERT + Duration::from_secs(6)
        ));
        // Template flag travels WITH the image, in the same call.
        assert_eq!(*tray.puts.lock().unwrap(), [true, true, true]);
    }

    #[test]
    fn a_failed_set_is_retried_on_the_next_tick() {
        // The first memo remembered an image before setting it, so a set that
        // failed was never tried again while the state held.
        let tray = FakeTray::default();
        let memo = IconMemo::new();
        let t0 = std::time::Instant::now();
        tray.fail.store(true, Ordering::SeqCst);
        apply_icon(&tray, &memo, drawable(IconState::Running), t0);
        tray.fail.store(false, Ordering::SeqCst);
        assert!(apply_icon(
            &tray,
            &memo,
            drawable(IconState::Running),
            t0 + Duration::from_secs(5)
        ));
        assert_eq!(tray.puts.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_failed_build_backs_off_and_caps() {
        let secs: Vec<u64> = (0..8).map(|a| build_retry_delay(a).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 80, 160, 300, 300]);
        assert_eq!(build_retry_delay(u32::MAX).as_secs(), 300);
    }

    #[tokio::test]
    async fn a_panicking_loop_is_reported_and_restarted() {
        // M3: one panic used to end the poll task for the life of the process.
        let runs = Arc::new(AtomicU32::new(0));
        let crashes = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let r = runs.clone();
        let c = crashes.clone();
        supervise(
            move || {
                let r = r.clone();
                async move {
                    if r.fetch_add(1, Ordering::SeqCst) < 2 {
                        panic!("boom");
                    }
                }
            },
            move |m| c.lock().unwrap().push(m),
            Duration::from_millis(1),
        )
        .await;
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        assert_eq!(*crashes.lock().unwrap(), ["boom", "boom"]);
    }
}
