//! How often to poll, and how to back away when GitLab says to stop.

use std::time::{Duration, Instant, SystemTime};

use crate::client::ClientError;
use crate::config::{PollConfig, RateLimitBackoff};

/// The shortest gap an explicit `poll_now` hint will honour.
///
/// Opening and closing the popover repeatedly must not turn into a request
/// storm, so a hint inside this window is dropped. Two seconds rather than the
/// original five: with `live_secs = 5` a five-second gate refused nearly every
/// open, so the popover showed a view as old as the live interval itself.
pub const POLL_NOW_MIN_GAP: Duration = Duration::from_secs(2);

/// The longest `Retry-After` that is taken literally.
///
/// ⛔ The header is attacker- and accident-controlled and it is added to an
/// [`Instant`]: `Instant + Duration` **panics** on overflow, and a panic in the
/// poll task is permanent and silent — the tray keeps its last icon and nothing
/// works again until the process is restarted. A `Retry-After` measured in years
/// is a broken header rather than an instruction, so it is clamped. Twenty-four
/// hours is far beyond anything GitLab sends and far short of anything that can
/// overflow.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Why an out-of-band poll was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollNow {
    /// The popover was opened.
    PopoverOpened,
    /// The machine woke from sleep, so the cached view may be very stale.
    Wake,
    /// The configuration changed.
    ConfigChanged,
    /// A human asked.
    Manual,
}

impl PollNow {
    /// Whether this reason is allowed to skip the rate gate.
    ///
    /// A wake or a config change is rare and consequential; a popover open is
    /// neither, and is the one that needs limiting.
    pub fn bypasses_gate(&self) -> bool {
        matches!(
            self,
            PollNow::Wake | PollNow::ConfigChanged | PollNow::Manual
        )
    }
}

/// A moment recorded on both clocks.
///
/// ⚠️ [`Instant`] is monotonic and **excludes suspend** on both macOS
/// (`CLOCK_UPTIME_RAW`) and Linux (`CLOCK_MONOTONIC`), so a laptop that slept
/// for eight hours reports an elapsed time of seconds. Anything gating on "has
/// it been long enough" would then refuse the very poll that resume needs.
/// [`SystemTime`] does not stop while the machine sleeps but can jump backwards
/// under NTP, in which case `elapsed` fails and the monotonic reading stands.
/// Taking the larger of the two is wrong in neither direction.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    mono: Instant,
    wall: SystemTime,
}

impl Stamp {
    fn now() -> Self {
        Self {
            mono: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    fn elapsed(&self) -> Duration {
        self.mono
            .elapsed()
            .max(self.wall.elapsed().unwrap_or(Duration::ZERO))
    }
}

/// The interval schedule for one watch, including error backoff.
#[derive(Debug, Clone)]
pub struct PollPolicy {
    live: Duration,
    idle: Duration,
    max_backoff: Duration,
    consecutive_errors: u32,
    retry_after: Option<Duration>,
    last_poll: Option<Stamp>,
}

impl PollPolicy {
    /// Build a policy from a watch's `poll` block and its account's backoff
    /// ceiling.
    pub fn new(poll: &PollConfig, backoff: &RateLimitBackoff) -> Self {
        Self {
            live: Duration::from_secs(poll.live_secs.max(1)),
            idle: Duration::from_secs(poll.idle_secs.max(1)),
            max_backoff: Duration::from_secs(backoff.max_secs.max(1)),
            consecutive_errors: 0,
            retry_after: None,
            last_poll: None,
        }
    }

    /// The interval to wait before the next tick.
    ///
    /// `any_live` is the honest one: a pipeline parked at a gate counts as
    /// settled, because it will still be parked in twenty seconds.
    pub fn interval(&self, any_live: bool) -> Duration {
        let base = if any_live { self.live } else { self.idle };
        if self.consecutive_errors == 0 {
            return base;
        }
        // Double per consecutive error, capped. `Retry-After` wins when the
        // server named a number: it knows when it will forgive us.
        let doubled = base
            .checked_mul(2u32.saturating_pow(self.consecutive_errors.min(16)))
            .unwrap_or(self.max_backoff)
            .min(self.max_backoff);
        match self.retry_after {
            Some(r) => doubled.max(r).min(self.max_backoff.max(r)),
            None => doubled,
        }
    }

    /// Record a tick that worked, clearing any backoff.
    pub fn on_success(&mut self) {
        self.consecutive_errors = 0;
        self.retry_after = None;
        self.last_poll = Some(Stamp::now());
    }

    /// Record a tick that failed.
    ///
    /// ⛔ **Every** class of error extends the backoff, not only the ones
    /// `should_back_off` names. A 404 (a project renamed, a token that lost
    /// sight of it), a decode failure or an unexpected status will not fix
    /// itself inside a minute either, and asking at full pace is how one
    /// misconfiguration turns into a rate limit on the whole account. A 401 will
    /// not fix itself by being asked less often, but it is equally pointless to
    /// hammer.
    pub fn on_error(&mut self, error: &ClientError) {
        self.last_poll = Some(Stamp::now());
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        if let Some(secs) = error.retry_after() {
            self.retry_after = Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER));
        }
    }

    /// How many consecutive failures have been recorded.
    pub fn consecutive_errors(&self) -> u32 {
        self.consecutive_errors
    }

    /// How long since the last poll, on whichever clock says longer.
    pub fn since_last_poll(&self) -> Option<Duration> {
        self.last_poll.as_ref().map(Stamp::elapsed)
    }

    /// Whether this watch should sit out a tick that has come round while it is
    /// still backing off.
    ///
    /// ⛔ Backoff is per watch but the sleep between ticks is the `min` over all
    /// of them, so without this a watch that has just been told 429 is asked
    /// again on the healthiest watch's cadence — and `Retry-After` is ignored in
    /// everything but name. A watch with no errors never defers: the tick
    /// remains "poll everything now", which is what every request-count claim in
    /// the tests rests on.
    pub fn should_defer(&self, any_live: bool) -> bool {
        if self.consecutive_errors == 0 && self.retry_after.is_none() {
            return false;
        }
        match self.since_last_poll() {
            None => false,
            Some(elapsed) => elapsed < self.interval(any_live),
        }
    }

    /// Whether an out-of-band poll should be honoured now.
    pub fn allows_poll_now(&self, reason: PollNow) -> bool {
        if reason.bypasses_gate() {
            return true;
        }
        match self.since_last_poll() {
            None => true,
            Some(elapsed) => elapsed >= POLL_NOW_MIN_GAP,
        }
    }

    /// Pretend the last poll happened now. Used by the poller after a tick it
    /// did not drive itself.
    pub fn mark_polled(&mut self) {
        self.last_poll = Some(Stamp::now());
    }

    /// Pretend the last poll happened `mono_ago` ago on the monotonic clock and
    /// `wall_ago` ago on the wall clock.
    ///
    /// The two arguments are not redundant: they disagree by exactly the time
    /// the machine spent suspended, and that disagreement is the only way to
    /// exercise the resume path without sleeping a laptop inside a test.
    pub fn mark_polled_ago(&mut self, mono_ago: Duration, wall_ago: Duration) {
        let now = Stamp::now();
        self.last_poll = Some(Stamp {
            mono: now.mono.checked_sub(mono_ago).unwrap_or(now.mono),
            wall: now.wall.checked_sub(wall_ago).unwrap_or(now.wall),
        });
    }
}
