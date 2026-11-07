//! "Pause notifications", and what a tick's notifications become on their
//! way to the OS.
//!
//! The core decides what is news and records it in the ledger; this decides
//! only whether to show it now. A pause therefore drops, it does not queue:
//! what arrives while paused is already in the ledger, so resuming does not
//! replay an hour of banners. A pause lives in memory and ends when the app
//! quits.

use bridgewatch_core::notify::{Notification, batch};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

/// How long the menu's timed pause lasts.
pub const PAUSE_FOR: Duration = Duration::hours(1);

/// A pause in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// Until this instant.
    Until(DateTime<Utc>),
    /// Until somebody resumes.
    UntilResumed,
}

/// The pause as the popover reads it: `until` is an RFC 3339 instant, or
/// `null` for "until you resume".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PauseView {
    pub until: Option<DateTime<Utc>>,
}

/// The pause state. `Default` is not paused.
#[derive(Debug, Default, Clone)]
pub struct Quiet {
    pause: Option<Pause>,
}

impl Quiet {
    /// Pause for [`PAUSE_FOR`] from `now`.
    pub fn pause_for_an_hour(&mut self, now: DateTime<Utc>) {
        self.pause = Some(Pause::Until(now + PAUSE_FOR));
    }

    /// Pause until [`Self::resume`].
    pub fn pause_until_resumed(&mut self) {
        self.pause = Some(Pause::UntilResumed);
    }

    /// Show notifications again.
    pub fn resume(&mut self) {
        self.pause = None;
    }

    /// The pause in force at `now`, forgetting one that has run out.
    pub fn current(&mut self, now: DateTime<Utc>) -> Option<Pause> {
        if let Some(Pause::Until(at)) = self.pause
            && now >= at
        {
            self.pause = None;
        }
        self.pause
    }

    /// [`Self::current`] in the popover's shape.
    pub fn view(&mut self, now: DateTime<Utc>) -> Option<PauseView> {
        self.current(now).map(|p| PauseView {
            until: match p {
                Pause::Until(at) => Some(at),
                Pause::UntilResumed => None,
            },
        })
    }

    /// What of one tick's notifications reaches the OS at `now`: nothing while
    /// paused, otherwise the tick's notifications with a burst folded into
    /// one ([`batch`]).
    pub fn outgoing(&mut self, now: DateTime<Utc>, tick: Vec<Notification>) -> Vec<Notification> {
        if tick.is_empty() {
            return tick;
        }
        if self.current(now).is_some() {
            tracing::debug!(count = tick.len(), "notifications paused; not shown");
            return Vec::new();
        }
        batch(tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridgewatch_core::notify::NotifyKind;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
    }

    fn notes(n: u64) -> Vec<Notification> {
        (1..=n)
            .map(|i| Notification {
                watch: "web".into(),
                kind: NotifyKind::Deployed,
                pipeline_id: i,
                title: format!("web #{i}: deployed"),
                body: String::new(),
                url: None,
                key: format!("{i}|deployed|"),
            })
            .collect()
    }

    #[test]
    fn an_hour_s_pause_drops_notifications_and_then_runs_out() {
        let mut quiet = Quiet::default();
        assert_eq!(quiet.outgoing(at(0), notes(1)).len(), 1);

        quiet.pause_for_an_hour(at(0));
        assert_eq!(quiet.current(at(10)), Some(Pause::Until(at(3600))));
        assert!(quiet.outgoing(at(10), notes(2)).is_empty());
        assert!(quiet.outgoing(at(3599), notes(1)).is_empty());

        assert_eq!(quiet.outgoing(at(3600), notes(1)).len(), 1, "it ran out");
        assert_eq!(quiet.current(at(3600)), None);
        assert_eq!(quiet.view(at(3600)), None);
    }

    #[test]
    fn a_pause_until_resumed_lasts_until_resumed() {
        let mut quiet = Quiet::default();
        quiet.pause_until_resumed();
        assert!(quiet.outgoing(at(1_000_000), notes(1)).is_empty());
        assert_eq!(quiet.view(at(1_000_000)), Some(PauseView { until: None }));
        quiet.resume();
        assert_eq!(quiet.outgoing(at(1_000_001), notes(1)).len(), 1);
    }

    #[test]
    fn what_arrived_while_paused_is_not_replayed_on_resume() {
        let mut quiet = Quiet::default();
        quiet.pause_until_resumed();
        assert!(quiet.outgoing(at(0), notes(2)).is_empty());
        quiet.resume();
        assert!(quiet.outgoing(at(1), Vec::new()).is_empty());
    }

    #[test]
    fn an_unpaused_burst_is_folded_into_one() {
        let mut quiet = Quiet::default();
        assert_eq!(quiet.outgoing(at(0), notes(3)).len(), 3);
        let folded = quiet.outgoing(at(0), notes(4));
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].kind, NotifyKind::Batch);
    }

    #[test]
    fn the_view_serialises_as_the_popover_reads_it() {
        let mut quiet = Quiet::default();
        quiet.pause_for_an_hour(at(0));
        assert_eq!(
            serde_json::to_value(quiet.view(at(0))).unwrap(),
            serde_json::json!({ "until": "2027-01-15T09:00:00Z" })
        );
        quiet.pause_until_resumed();
        assert_eq!(
            serde_json::to_value(quiet.view(at(0))).unwrap(),
            serde_json::json!({ "until": null })
        );
    }
}
