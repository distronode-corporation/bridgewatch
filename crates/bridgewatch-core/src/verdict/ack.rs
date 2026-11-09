//! Acknowledging a failure locally: the tray icon drops a level, and nothing is
//! sent anywhere.
//!
//! A failure someone already knows about (a flaky job, a fix already pushed)
//! keeps the tray red for as long as the pipeline stays the newest one, and a
//! red icon that is always red stops saying anything. Acknowledging it draws
//! the tray one level lower for that watch: `failed` as
//! `deployed_with_failure`, and `deployed_with_failure` as `deployed`. The
//! popover still says what the pipeline did; only the tray is quieter.
//!
//! ⛔ An acknowledgement is for ONE failure: the watch's newest pipeline, in
//! the state it was in. It is forgotten as soon as that stops being true,
//! that is on the next new failure (a newer pipeline), on the same pipeline
//! getting worse, or on the failure going away, so it can never hide a failure
//! nobody has seen. It lives in memory only: not in the config, not in the
//! notification ledger, and not on the server.

use std::collections::BTreeMap;

use super::{IconState, Snapshot, WatchView};

/// The state the tray draws for an acknowledged failure, or `None` for a
/// state that cannot be acknowledged.
pub fn demoted(state: IconState) -> Option<IconState> {
    match state {
        IconState::Failed => Some(IconState::DeployedWithFailure),
        IconState::DeployedWithFailure => Some(IconState::Deployed),
        _ => None,
    }
}

/// What the tray shows once acknowledgements are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayState {
    /// The state whose glyph is drawn.
    pub drawn: IconState,
    /// The snapshot's own state, when an acknowledgement changed what is
    /// drawn; `None` when the tray shows the snapshot's state as it is.
    pub acknowledged_from: Option<IconState>,
}

/// One acknowledged failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Acked {
    pipeline: u64,
    state: IconState,
}

/// The acknowledged failures, by watch id.
#[derive(Debug, Clone, Default)]
pub struct Acknowledgements {
    by_watch: BTreeMap<String, Acked>,
}

/// The failure a watch shows now: its newest pipeline and its state, when that
/// state can be acknowledged. A secondary watch never touches the tray, so it
/// has nothing to acknowledge.
fn current_failure(watch: &WatchView) -> Option<Acked> {
    let state = watch.icon_state?;
    demoted(state)?;
    let pipeline = watch.rows.iter().map(|r| r.id).max()?;
    Some(Acked { pipeline, state })
}

impl Acknowledgements {
    /// Acknowledge the failure `watch` shows in `snapshot`. Returns false, and
    /// changes nothing, when there is no such watch or it shows no failure.
    pub fn acknowledge(&mut self, snapshot: &Snapshot, watch: &str) -> bool {
        let Some(failure) = snapshot
            .watches
            .iter()
            .find(|w| w.id == watch)
            .and_then(current_failure)
        else {
            return false;
        };
        self.by_watch.insert(watch.to_string(), failure);
        true
    }

    /// Take an acknowledgement back. Returns whether there was one.
    pub fn clear(&mut self, watch: &str) -> bool {
        self.by_watch.remove(watch).is_some()
    }

    /// Whether `watch` has an acknowledgement in force.
    pub fn is_acknowledged(&self, watch: &str) -> bool {
        self.by_watch.contains_key(watch)
    }

    /// Fit the acknowledgements to a new snapshot and say what the tray draws.
    ///
    /// Forgets every acknowledgement whose failure is no longer the watch's
    /// current one: a newer pipeline, the same pipeline in a worse state, a
    /// failure that went away, or a watch that is gone. One whose pipeline got
    /// BETTER but still failed (`failed` to `deployed_with_failure`) stays,
    /// at the new state: nothing new has failed. Writes the watches still
    /// acknowledged into `snapshot.acknowledged`.
    pub fn apply(&mut self, snapshot: &mut Snapshot) -> TrayState {
        self.by_watch.retain(|id, acked| {
            let Some(now) = snapshot
                .watches
                .iter()
                .find(|w| &w.id == id)
                .and_then(current_failure)
            else {
                return false;
            };
            // `severity` is lowest for the loudest state.
            if now.pipeline != acked.pipeline || now.state.severity() < acked.state.severity() {
                return false;
            }
            *acked = now;
            true
        });
        snapshot.acknowledged = self.by_watch.keys().cloned().collect();

        let drawn = snapshot
            .watches
            .iter()
            .filter(|w| w.role.is_primary())
            .filter_map(|w| {
                let state = w.icon_state?;
                Some(match self.by_watch.contains_key(&w.id) {
                    true => demoted(state).unwrap_or(state),
                    false => state,
                })
            })
            .min_by_key(IconState::severity)
            .unwrap_or(IconState::Unknown);
        TrayState {
            drawn,
            acknowledged_from: (drawn != snapshot.icon_state).then_some(snapshot.icon_state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Role;

    fn watch(id: &str, role: Role, newest: u64, state: Option<IconState>) -> WatchView {
        let row: super::super::PipelineView = serde_json::from_value(serde_json::json!({
            "id": newest, "iid": null, "sha": "0123456789abcdef0123456789abcdef01234567",
            "sha7": "0123456", "ref": "main", "source": "push", "status": "failed",
            "web_url": null, "state": state.unwrap_or(IconState::Unknown), "deploy": "none",
            "deploy_marker": null, "deploy_failures": [], "failures": [], "warnings": [],
            "gates": [], "post_deploy_failures": [], "sibling_failures": [], "bridges": [],
            "parent_jobs": [], "updated_at": null, "created_at": null, "live": false
        }))
        .expect("a pipeline view");
        WatchView {
            id: id.into(),
            role,
            icon_state: state,
            rows: vec![row],
            error: None,
            jobs: Default::default(),
            provider: Default::default(),
            actions: false,
        }
    }

    fn snapshot(watches: Vec<WatchView>) -> Snapshot {
        let mut s = Snapshot::empty();
        s.icon_state = Snapshot::icon_from_watches(&watches);
        s.watches = watches;
        s
    }

    #[test]
    fn an_acknowledged_failure_drops_the_tray_one_level() {
        let mut acks = Acknowledgements::default();
        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        assert!(acks.acknowledge(&s, "web"));
        let tray = acks.apply(&mut s);
        assert_eq!(tray.drawn, IconState::DeployedWithFailure);
        assert_eq!(tray.acknowledged_from, Some(IconState::Failed));
        assert_eq!(s.acknowledged, vec!["web".to_string()]);
        // The snapshot's own verdict is untouched: only the tray is quieter.
        assert_eq!(s.icon_state, IconState::Failed);

        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::DeployedWithFailure),
        )]);
        let mut acks = Acknowledgements::default();
        assert!(acks.acknowledge(&s, "web"));
        assert_eq!(acks.apply(&mut s).drawn, IconState::Deployed);
    }

    #[test]
    fn another_watchs_failure_still_shows_at_full_level() {
        let mut acks = Acknowledgements::default();
        let mut s = snapshot(vec![
            watch("web", Role::Primary, 10, Some(IconState::Failed)),
            watch("api", Role::Primary, 20, Some(IconState::Failed)),
        ]);
        assert!(acks.acknowledge(&s, "web"));
        let tray = acks.apply(&mut s);
        assert_eq!(tray.drawn, IconState::Failed);
        assert_eq!(tray.acknowledged_from, None);
    }

    #[test]
    fn only_a_failure_of_a_primary_watch_can_be_acknowledged() {
        let mut acks = Acknowledgements::default();
        let s = snapshot(vec![
            watch("green", Role::Primary, 10, Some(IconState::Deployed)),
            watch("running", Role::Primary, 11, Some(IconState::Running)),
            watch("hourly", Role::Secondary, 12, None),
        ]);
        for id in ["green", "running", "hourly", "missing"] {
            assert!(!acks.acknowledge(&s, id), "{id}");
        }
        assert!(!acks.is_acknowledged("green"));
    }

    #[test]
    fn the_next_new_failure_clears_it() {
        let mut acks = Acknowledgements::default();
        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        acks.acknowledge(&s, "web");
        acks.apply(&mut s);
        // A newer pipeline fails: full red again, and the acknowledgement is gone.
        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            11,
            Some(IconState::Failed),
        )]);
        let tray = acks.apply(&mut s);
        assert_eq!(tray.drawn, IconState::Failed);
        assert!(!acks.is_acknowledged("web"));
        assert!(s.acknowledged.is_empty());
    }

    #[test]
    fn the_same_pipeline_getting_worse_clears_it_and_getting_better_keeps_it() {
        let mut acks = Acknowledgements::default();
        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::DeployedWithFailure),
        )]);
        acks.acknowledge(&s, "web");
        acks.apply(&mut s);
        let mut worse = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        assert_eq!(acks.apply(&mut worse).drawn, IconState::Failed);
        assert!(!acks.is_acknowledged("web"));

        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        acks.acknowledge(&s, "web");
        acks.apply(&mut s);
        let mut better = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::DeployedWithFailure),
        )]);
        assert_eq!(acks.apply(&mut better).drawn, IconState::Deployed);
        assert!(acks.is_acknowledged("web"));
    }

    #[test]
    fn a_failure_that_went_away_or_a_watch_that_is_gone_forgets_it() {
        let mut acks = Acknowledgements::default();
        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        acks.acknowledge(&s, "web");
        acks.apply(&mut s);
        // A retried job passed: the same pipeline is green now.
        let mut green = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Deployed),
        )]);
        acks.apply(&mut green);
        assert!(!acks.is_acknowledged("web"));
        // ...and when it fails again, it is not acknowledged.
        let mut red = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        assert_eq!(acks.apply(&mut red).drawn, IconState::Failed);

        acks.acknowledge(&red, "web");
        let mut gone = snapshot(vec![]);
        acks.apply(&mut gone);
        assert!(!acks.is_acknowledged("web"));
    }

    #[test]
    fn clear_takes_it_back() {
        let mut acks = Acknowledgements::default();
        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        acks.acknowledge(&s, "web");
        assert!(acks.clear("web"));
        assert!(!acks.clear("web"));
        assert_eq!(acks.apply(&mut s).drawn, IconState::Failed);
    }

    #[test]
    fn the_acknowledged_list_is_left_out_of_json_when_empty() {
        let mut s = snapshot(vec![watch(
            "web",
            Role::Primary,
            10,
            Some(IconState::Failed),
        )]);
        let json = serde_json::to_value(&s).unwrap();
        assert!(json.get("acknowledged").is_none(), "{json}");
        let mut acks = Acknowledgements::default();
        acks.acknowledge(&s, "web");
        acks.apply(&mut s);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["acknowledged"], serde_json::json!(["web"]));
    }
}
