//! Noticing that the machine slept, without asking the operating system.
//!
//! A poll loop sleeps on the monotonic clock, and [`Instant`] **excludes
//! suspend** on both macOS (`CLOCK_UPTIME_RAW`) and Linux (`CLOCK_MONOTONIC`).
//! A laptop that slept for an hour in the middle of a sixty-second idle wait
//! therefore wakes with most of that wait still to run, and shows an hour-old
//! view until it ends. The wall clock ([`SystemTime`]) keeps running while the
//! machine sleeps, so between two readings taken a few seconds apart the wall
//! clock running far ahead of the monotonic one means the machine was asleep
//! in between, or the clock was stepped forward. Either way a poll now is the
//! right answer.
//!
//! ⛔ No logind, D-Bus or IOKit sleep notification is used, on purpose: those
//! differ per platform and per desktop, need a bus connection the CLI does not
//! otherwise have, and are absent in a container or a minimal session. Two
//! clocks are everywhere.
//!
//! A wall clock stepped BACKWARDS (NTP correcting a fast clock) is ignored: it
//! says nothing about sleep, and a poll is not needed for it.

use std::time::{Duration, Instant, SystemTime};

/// How often a [`WakeWatch`] reads the two clocks while it waits.
///
/// A sleep on the monotonic clock resumes with whatever was left of it, so a
/// wake is noticed at most this long after the machine is back.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);

/// How far the wall clock has to run ahead of the monotonic clock between two
/// readings to count as a wake.
///
/// Two readings [`CHECK_EVERY`] apart agree to well under a second on a
/// machine that stayed awake (an NTP slew moves the wall clock by at most half
/// a millisecond a second). A suspend shorter than this goes unnoticed, which
/// costs nothing: the view it leaves behind is no older than this.
pub const MIN_JUMP: Duration = Duration::from_secs(10);

/// The two clocks a [`WakeWatch`] compares. A trait so tests can step one
/// without the other.
pub trait Clocks {
    /// The monotonic clock, which stops while the machine sleeps.
    fn mono(&self) -> Instant;
    /// The wall clock, which does not.
    fn wall(&self) -> SystemTime;
}

/// The real clocks.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClocks;

impl Clocks for SystemClocks {
    fn mono(&self) -> Instant {
        Instant::now()
    }

    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Watches the two clocks for a jump that means the machine slept.
#[derive(Debug, Clone)]
pub struct WakeWatch<C: Clocks = SystemClocks> {
    clocks: C,
    mono: Instant,
    wall: SystemTime,
}

impl WakeWatch<SystemClocks> {
    /// A watch on the real clocks, starting now.
    pub fn new() -> Self {
        Self::with_clocks(SystemClocks)
    }
}

impl Default for WakeWatch<SystemClocks> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Clocks> WakeWatch<C> {
    /// A watch on the given clocks, starting from their current readings.
    pub fn with_clocks(clocks: C) -> Self {
        let (mono, wall) = (clocks.mono(), clocks.wall());
        Self { clocks, mono, wall }
    }

    /// Start again from the clocks' current readings, forgetting any jump
    /// since the last one.
    ///
    /// A poll loop calls this as a tick starts: a jump before it is answered
    /// by that very tick, and counting it again afterwards would poll twice.
    pub fn reset(&mut self) {
        self.mono = self.clocks.mono();
        self.wall = self.clocks.wall();
    }

    /// Read both clocks. Returns how far the wall clock ran ahead of the
    /// monotonic clock since the previous reading, when that is at least
    /// [`MIN_JUMP`]; `None` otherwise, a backward step of the wall clock
    /// included.
    pub fn check(&mut self) -> Option<Duration> {
        let (mono, wall) = (self.clocks.mono(), self.clocks.wall());
        let mono_elapsed = mono.saturating_duration_since(self.mono);
        let wall_elapsed = wall.duration_since(self.wall).unwrap_or(Duration::ZERO);
        self.mono = mono;
        self.wall = wall;
        let ahead = wall_elapsed.saturating_sub(mono_elapsed);
        (ahead >= MIN_JUMP).then_some(ahead)
    }

    /// Wait until a [`check`](Self::check) every [`CHECK_EVERY`] finds a
    /// wake, and return how far the wall clock jumped.
    ///
    /// Cancel-safe: meant as one arm of a `select!` that is re-entered, and a
    /// cancelled wait loses nothing, since the last reading is kept in `self`.
    pub async fn woke(&mut self) -> Duration {
        loop {
            tokio::time::sleep(CHECK_EVERY).await;
            if let Some(jump) = self.check() {
                return jump;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Two clocks the test moves by hand.
    #[derive(Clone)]
    struct Hand(Arc<Mutex<(Instant, SystemTime)>>);

    impl Hand {
        fn new() -> Self {
            Self(Arc::new(Mutex::new((Instant::now(), SystemTime::now()))))
        }
        /// Both clocks move: the machine was awake.
        fn awake(&self, d: Duration) {
            let mut t = self.0.lock().unwrap();
            t.0 += d;
            t.1 += d;
        }
        /// Only the wall clock moves: the machine slept.
        fn asleep(&self, d: Duration) {
            self.0.lock().unwrap().1 += d;
        }
        fn wall_back(&self, d: Duration) {
            self.0.lock().unwrap().1 -= d;
        }
    }

    impl Clocks for Hand {
        fn mono(&self) -> Instant {
            self.0.lock().unwrap().0
        }
        fn wall(&self) -> SystemTime {
            self.0.lock().unwrap().1
        }
    }

    #[test]
    fn a_machine_that_stayed_awake_never_reads_as_a_wake() {
        let hand = Hand::new();
        let mut watch = WakeWatch::with_clocks(hand.clone());
        for _ in 0..100 {
            hand.awake(CHECK_EVERY);
            assert_eq!(watch.check(), None);
        }
        // A long gap between readings is not a wake either when both clocks
        // agree: a busy loop that checked late.
        hand.awake(Duration::from_secs(3600));
        assert_eq!(watch.check(), None);
    }

    #[test]
    fn the_wall_clock_running_ahead_reads_as_a_wake_once() {
        let hand = Hand::new();
        let mut watch = WakeWatch::with_clocks(hand.clone());
        hand.asleep(Duration::from_secs(3600));
        hand.awake(Duration::from_secs(2));
        assert_eq!(watch.check(), Some(Duration::from_secs(3600)));
        // Answered: the next reading starts from the new pair.
        hand.awake(CHECK_EVERY);
        assert_eq!(watch.check(), None);
    }

    #[test]
    fn a_short_suspend_or_a_slew_is_below_the_threshold() {
        let hand = Hand::new();
        let mut watch = WakeWatch::with_clocks(hand.clone());
        hand.asleep(MIN_JUMP - Duration::from_millis(1));
        assert_eq!(watch.check(), None);
        hand.asleep(MIN_JUMP);
        assert_eq!(watch.check(), Some(MIN_JUMP));
    }

    #[test]
    fn a_wall_clock_stepped_backwards_is_ignored() {
        let hand = Hand::new();
        let mut watch = WakeWatch::with_clocks(hand.clone());
        hand.wall_back(Duration::from_secs(600));
        hand.awake(CHECK_EVERY);
        assert_eq!(watch.check(), None);
        // ...and the reading after it is measured from the stepped clock, so
        // the step is not mistaken for a jump forward later.
        hand.awake(CHECK_EVERY);
        assert_eq!(watch.check(), None);
    }

    #[test]
    fn reset_forgets_a_jump_already_answered() {
        let hand = Hand::new();
        let mut watch = WakeWatch::with_clocks(hand.clone());
        hand.asleep(Duration::from_secs(900));
        watch.reset();
        hand.awake(CHECK_EVERY);
        assert_eq!(watch.check(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn woke_returns_within_one_check_of_the_jump() {
        let hand = Hand::new();
        let mut watch = WakeWatch::with_clocks(hand.clone());
        let mover = hand.clone();
        // Tokio's paused clock drives the checks; the hand clocks follow it,
        // and the machine "sleeps" for an hour after the third check.
        let jumped = tokio::spawn(async move {
            for i in 0.. {
                tokio::time::sleep(CHECK_EVERY).await;
                mover.awake(CHECK_EVERY);
                if i == 2 {
                    mover.asleep(Duration::from_secs(3600));
                    break;
                }
            }
        });
        let started = tokio::time::Instant::now();
        let jump = watch.woke().await;
        jumped.await.unwrap();
        assert_eq!(jump, Duration::from_secs(3600));
        assert!(
            started.elapsed() <= CHECK_EVERY * 4,
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_wait_keeps_the_last_reading() {
        let hand = Hand::new();
        let mut watch = WakeWatch::with_clocks(hand.clone());
        // A select that keeps winning on another arm cancels the wait before
        // its first check; the jump that happened meanwhile is still found by
        // the next wait.
        hand.asleep(Duration::from_secs(120));
        tokio::select! {
            _ = watch.woke() => panic!("no check has run yet"),
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        assert_eq!(watch.woke().await, Duration::from_secs(120));
    }
}
