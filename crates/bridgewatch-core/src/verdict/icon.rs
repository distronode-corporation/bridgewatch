//! The icon-state rules: ordered rules over one pipeline's facts.
//!
//! First rule wins. The order is the point — a fleet that deployed and then
//! broke is a different thing from one that never deployed, and a monitor that
//! collapses them to "red" is the monitor bridgewatch was written to replace.

use serde::{Deserialize, Serialize};

use crate::config::FailurePolicy;

use super::deploy::DeployOutcome;

/// What the tray icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconState {
    /// No data, or an error with nothing cached to fall back on.
    Unknown,
    /// Something that should have worked did not.
    Failed,
    /// It is out, and something else is broken.
    DeployedWithFailure,
    /// It is out.
    Deployed,
    /// Work in flight.
    Running,
    /// Cancelled or skipped.
    Canceled,
    /// Nothing is running, nothing is broken, and a human has not pressed a
    /// button. The state every other monitor calls "running" forever.
    ParkedGate,
    /// Everything green, but this pipeline does not deploy.
    SucceededNoDeploy,
}

impl IconState {
    /// The lowercase name used in the view model, in `expected.json`, in Rhai
    /// scripts and as the icon file's stem.
    pub fn as_str(&self) -> &'static str {
        match self {
            IconState::Unknown => "unknown",
            IconState::Failed => "failed",
            IconState::DeployedWithFailure => "deployed_with_failure",
            IconState::Deployed => "deployed",
            IconState::Running => "running",
            IconState::Canceled => "canceled",
            IconState::ParkedGate => "parked_gate",
            IconState::SucceededNoDeploy => "succeeded_no_deploy",
        }
    }

    /// Parse a state name, for the Rhai hook's return value and for tests.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "unknown" => IconState::Unknown,
            "failed" => IconState::Failed,
            "deployed_with_failure" => IconState::DeployedWithFailure,
            "deployed" => IconState::Deployed,
            "running" => IconState::Running,
            "canceled" | "cancelled" => IconState::Canceled,
            "parked_gate" => IconState::ParkedGate,
            "succeeded_no_deploy" => IconState::SucceededNoDeploy,
            _ => return None,
        })
    }

    /// How loudly this state should shout, lowest first.
    ///
    /// Used to pick one tray icon when several primary watches disagree: the
    /// worst news wins, because a tray icon that hides a failure behind a
    /// success is worse than no tray icon.
    pub fn severity(&self) -> u8 {
        match self {
            IconState::Failed => 0,
            IconState::DeployedWithFailure => 1,
            IconState::Running => 2,
            IconState::Canceled => 3,
            IconState::ParkedGate => 4,
            IconState::Unknown => 5,
            IconState::Deployed => 6,
            IconState::SucceededNoDeploy => 7,
        }
    }

    /// True for the states worth interrupting somebody over.
    pub fn is_bad(&self) -> bool {
        matches!(self, IconState::Failed | IconState::DeployedWithFailure)
    }
}

impl std::fmt::Display for IconState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything the rules need about one pipeline, gathered once.
#[derive(Debug, Clone)]
pub struct PipelineFacts<'a> {
    /// The deploy outcome from the watch's markers.
    pub deploy: &'a DeployOutcome,
    /// The parent pipeline's own jobs hold a blocking failure.
    pub parent_blocking: bool,
    /// Any bridge is `failed` or `dead`.
    pub bridge_failed_or_dead: bool,
    /// A failed or dead bridge that is **not** the one carrying the deploy
    /// marker.
    pub sibling_failure: bool,
    /// A blocking failure that started after the marker succeeded.
    pub post_deploy_failure: bool,
    /// A job or a bridge is actually in flight. A gate is not.
    pub anything_live: bool,
    /// A bridge is waiting on a human.
    pub awaiting_gate: bool,
    /// The parent pipeline is `canceled` or `skipped`.
    pub parent_not_built: bool,
    /// Any blocking failure anywhere that was looked.
    pub any_blocking_failure: bool,
    /// A blocking failure in the marker's own pipeline that did not start after
    /// the marker. Carried separately because it is neither a sibling bridge's
    /// failure nor a post-deploy one, and collapsing it into either would hide
    /// it: see [`super::deploy::marker_scope_failures`].
    pub marker_scope_failure: bool,
    /// Neither `/jobs` nor `/bridges` could be read for this pipeline and
    /// nothing was cached, so every list the rules consult is empty for want of
    /// data rather than for want of failures.
    pub detail_unavailable: bool,
    /// What a sibling failure does.
    pub sibling_policy: FailurePolicy,
    /// What a post-deploy failure does.
    pub post_deploy_policy: FailurePolicy,
}

/// Apply the rules, in order.
pub fn state(f: &PipelineFacts<'_>) -> IconState {
    // 0. Nothing was read. ⛔ This has to come before everything else: an empty
    // `jobs` list is indistinguishable from a pipeline with no failures, so a
    // 429 or a 15 s timeout on the second request of a tick used to turn a red
    // pipeline into the outline check, exit 0, with the error visible only in
    // the watch's `error` string. `live` still comes from the list row, which
    // did arrive.
    if f.detail_unavailable {
        return IconState::Unknown;
    }

    // 1. It is broken. A parent blocking failure counts here only when it is
    // not already accounted for as a post-deploy one; otherwise rule 1 would
    // pre-empt rule 6 and `post_deploy_failure = "downgrade"` could never apply
    // to a job in the parent (README rule 6 says it does).
    if matches!(f.deploy, DeployOutcome::Failed { .. } | DeployOutcome::Dead)
        || f.parent_blocking
        || (f.deploy.is_absent() && f.bridge_failed_or_dead)
    {
        return IconState::Failed;
    }

    // 1b. A marker in a status this build does not understand. Saying "unknown"
    // out loud is the point of the outcome existing.
    if matches!(f.deploy, DeployOutcome::Unknown) {
        return IconState::Unknown;
    }

    // 2 and 3. It is out, with or without collateral damage.
    if f.deploy.is_live() {
        let sibling = f.sibling_failure && f.sibling_policy != FailurePolicy::Ignore;
        // A failure inside the marker's own pipeline takes the post-deploy
        // policy too. It is not literally post-deploy, but the shape is the
        // same — it shipped, and something around it did not — and leaving it
        // with no policy at all is what made a pre-marker failure read as a
        // clean `deployed`.
        let post = (f.post_deploy_failure || f.marker_scope_failure)
            && f.post_deploy_policy != FailurePolicy::Ignore;
        if (sibling && f.sibling_policy == FailurePolicy::Fail)
            || (post && f.post_deploy_policy == FailurePolicy::Fail)
        {
            return IconState::Failed;
        }
        if sibling || post {
            return IconState::DeployedWithFailure;
        }
        return IconState::Deployed;
    }

    // 4. It is on its way.
    if matches!(f.deploy, DeployOutcome::InProgress { .. })
        || (f.deploy.is_absent() && f.anything_live)
    {
        return IconState::Running;
    }

    // 5. Somebody stopped it.
    if f.parent_not_built || matches!(f.deploy, DeployOutcome::Canceled) {
        return IconState::Canceled;
    }

    // 6. It is waiting for a human. Not running, not broken, not done.
    if f.deploy.is_absent() && f.awaiting_gate && !f.anything_live && !f.any_blocking_failure {
        return IconState::ParkedGate;
    }

    // 7. Green, but this is not a lane that deploys.
    if f.deploy.is_absent() && !f.anything_live && !f.any_blocking_failure {
        return IconState::SucceededNoDeploy;
    }

    IconState::Unknown
}
