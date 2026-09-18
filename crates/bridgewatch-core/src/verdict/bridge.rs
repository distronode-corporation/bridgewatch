//! The bridge verdict: what a `trigger:*` job plus its child pipeline mean.
//!
//! This is the whole reason bridgewatch exists. A monitor that stops at the
//! parent pipeline cannot tell you that the website deployed while the android
//! bridge failed, because at the parent both are just jobs.

use serde::{Deserialize, Serialize};

use crate::config::WatchRules;
use crate::model::{Bridge, Job};
use crate::status::{JobClass, Status};

use super::job::{classify, classify_parts};

/// What a bridge resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum BridgeVerdict {
    /// The trigger job exists but no child pipeline was ever created. Usually a
    /// child `.gitlab-ci.yml` that failed to parse, or a `needs:` on a job the
    /// child lane does not have. Nothing downstream ran, and the parent may
    /// still read green.
    Dead,
    /// The child is in flight.
    Running,
    /// Nothing is running and at least one manual job is waiting for a human.
    AwaitingGate {
        /// The manual jobs that are waiting.
        jobs: Vec<String>,
    },
    /// At least one blocking failure.
    Failed {
        /// The blocking failures. Empty when the bridge itself failed but the
        /// child's jobs were not walked, or hold no failure of their own.
        jobs: Vec<String>,
    },
    /// Succeeded, with tolerated failures worth showing.
    PassedWithWarnings {
        /// The allow-failure jobs that failed.
        jobs: Vec<String>,
    },
    /// Succeeded cleanly.
    Passed,
    /// Cancelled.
    Canceled,
    /// Skipped by its rules.
    Skipped,
    /// A status bridgewatch does not recognise, preserved verbatim.
    Unknown {
        /// The raw status.
        status: String,
    },
}

impl BridgeVerdict {
    /// The single word used in the view model, in `expected.json` and in Rhai.
    pub fn word(&self) -> &'static str {
        match self {
            BridgeVerdict::Dead => "dead",
            BridgeVerdict::Running => "running",
            BridgeVerdict::AwaitingGate { .. } => "awaiting_gate",
            BridgeVerdict::Failed { .. } => "failed",
            BridgeVerdict::PassedWithWarnings { .. } => "passed_with_warnings",
            BridgeVerdict::Passed => "passed",
            BridgeVerdict::Canceled => "canceled",
            BridgeVerdict::Skipped => "skipped",
            BridgeVerdict::Unknown { .. } => "unknown",
        }
    }

    /// The job names this verdict names, if any.
    pub fn jobs(&self) -> &[String] {
        match self {
            BridgeVerdict::AwaitingGate { jobs }
            | BridgeVerdict::Failed { jobs }
            | BridgeVerdict::PassedWithWarnings { jobs } => jobs,
            _ => &[],
        }
    }

    /// True for the two verdicts that mean something is wrong.
    pub fn is_failure(&self) -> bool {
        matches!(self, BridgeVerdict::Failed { .. } | BridgeVerdict::Dead)
    }

    /// True while work is expected. A bridge parked at a gate is **not** live:
    /// it will sit there until somebody presses a button, and holding the fast
    /// poll interval open for that is how a monitor burns a rate limit.
    pub fn is_live(&self) -> bool {
        matches!(self, BridgeVerdict::Running)
    }

    /// True when a human is being waited on.
    pub fn is_awaiting_gate(&self) -> bool {
        matches!(self, BridgeVerdict::AwaitingGate { .. })
    }
}

/// The class the trigger job itself has, after `[watches.jobs]` and its own
/// `allow_failure`.
///
/// A bridge is a job. It carries `allow_failure`, it has a name the override
/// table can match, and until 2026-09-17 neither was read: `Bridge.allow_failure`
/// had no reader anywhere in the crate and `"trigger:android" = "ignore"` was a
/// line that did nothing.
pub fn own_class(bridge: &Bridge, rules: &WatchRules) -> JobClass {
    classify_parts(&bridge.name, &bridge.status, bridge.allow_failure, rules)
}

/// Resolve a bridge.
///
/// `child_jobs` is `None` when the bridge was not walked into, either because
/// `dive.bridges` does not match it or because `dive.only_when` held. In that
/// case the verdict comes from the trigger job's own status, which is all the
/// information there is.
///
/// `nested` holds the verdicts of the child's *own* bridges, which exist only
/// when `dive.depth` took the walk past this level. A child whose grandchild
/// bridge is dead cannot honestly read `passed`.
pub fn verdict(
    bridge: &Bridge,
    child_jobs: Option<&[Job]>,
    nested: &[(String, BridgeVerdict)],
    rules: &WatchRules,
) -> BridgeVerdict {
    reconcile(
        raw_verdict(bridge, child_jobs, nested, rules),
        own_class(bridge, rules),
        &bridge.name,
    )
}

/// Re-read a verdict through the trigger job's own class.
///
/// The child's jobs say what happened; the override table and `allow_failure`
/// say what it is worth. They are separate steps because the job names a
/// failure has to keep naming it even when the failure is tolerated — a
/// warning with no job names in it is not worth showing.
fn reconcile(raw: BridgeVerdict, own: JobClass, name: &str) -> BridgeVerdict {
    match own {
        // `ignore` is handled by the caller, which drops the bridge entirely;
        // reaching here with it would mean a bridge that is half-hidden.
        JobClass::Ignored => raw,
        JobClass::WarningFailure if raw.is_failure() => {
            let mut jobs = raw.jobs().to_vec();
            if jobs.is_empty() {
                jobs.push(name.to_string());
            }
            BridgeVerdict::PassedWithWarnings { jobs }
        }
        JobClass::BlockingFailure if matches!(raw, BridgeVerdict::PassedWithWarnings { .. }) => {
            BridgeVerdict::Failed {
                jobs: raw.jobs().to_vec(),
            }
        }
        // `gate` on a trigger job that was cancelled or skipped says "this lane
        // is expected to sit unrun", which is a park and not missing work.
        JobClass::Gate if matches!(raw, BridgeVerdict::Canceled | BridgeVerdict::Skipped) => {
            BridgeVerdict::AwaitingGate {
                jobs: vec![name.to_string()],
            }
        }
        _ => raw,
    }
}

/// The verdict from the child alone, before the trigger job's own class is
/// applied.
fn raw_verdict(
    bridge: &Bridge,
    child_jobs: Option<&[Job]>,
    nested: &[(String, BridgeVerdict)],
    rules: &WatchRules,
) -> BridgeVerdict {
    // A bridge with no downstream pipeline is dead whatever its own status says,
    // and its own status is frequently `success`.
    let Some(downstream) = &bridge.downstream_pipeline else {
        return BridgeVerdict::Dead;
    };

    let Some(jobs) = child_jobs else {
        return from_status_only(&bridge.status, downstream);
    };

    let classified: Vec<(JobClass, &Job)> = jobs
        .iter()
        .map(|j| (classify(j, rules), j))
        .filter(|(c, _)| *c != JobClass::Ignored)
        .collect();

    let mut blocking: Vec<String> = classified
        .iter()
        .filter(|(c, _)| c.is_blocking_failure())
        .map(|(_, j)| j.name.clone())
        .collect();
    // A grandchild that failed or was never created is this child's failure. It
    // is only ever non-empty when `dive.depth > 1`, which is the whole reason
    // walking deeper is worth a request. A dead one has no job to name, so it
    // is named by its own trigger job, exactly as the parent level does.
    for (nested_name, nested_verdict) in nested {
        if nested_verdict.is_failure() {
            let named = nested_verdict.jobs();
            if named.is_empty() {
                blocking.push(nested_name.clone());
            } else {
                blocking.extend(named.iter().cloned());
            }
        }
    }
    let warnings: Vec<String> = classified
        .iter()
        .filter(|(c, _)| c.is_warning_failure())
        .map(|(_, j)| j.name.clone())
        .collect();
    let gates: Vec<String> = classified
        .iter()
        .filter(|(c, _)| c.is_gate())
        .map(|(_, j)| j.name.clone())
        .collect();
    let any_live =
        classified.iter().any(|(c, _)| c.is_live()) || nested.iter().any(|(_, v)| v.is_live());

    // The table in the README, in order. Cancellation is checked first because a
    // cancelled child leaves half-run jobs that would otherwise read as gaps.
    if bridge.status.is_not_built() && blocking.is_empty() {
        return match bridge.status {
            Status::Skipped => BridgeVerdict::Skipped,
            _ => BridgeVerdict::Canceled,
        };
    }
    if bridge.status.is_live() {
        if any_live {
            return BridgeVerdict::Running;
        }
        if !blocking.is_empty() {
            return BridgeVerdict::Failed { jobs: blocking };
        }
        if !gates.is_empty() {
            return BridgeVerdict::AwaitingGate { jobs: gates };
        }
        return BridgeVerdict::Running;
    }
    if !blocking.is_empty() {
        return BridgeVerdict::Failed { jobs: blocking };
    }
    if bridge.status.is_failed() {
        // The trigger job failed but nothing in the child is a blocking failure:
        // the child was cancelled out from under it, or the failure is in a
        // grandchild this dive did not reach.
        return BridgeVerdict::Failed { jobs: Vec::new() };
    }
    if bridge.status.is_success() {
        if !warnings.is_empty() {
            return BridgeVerdict::PassedWithWarnings { jobs: warnings };
        }
        return BridgeVerdict::Passed;
    }
    if bridge.status.is_gate() {
        let mut jobs = gates;
        if jobs.is_empty() {
            jobs.push(bridge.name.clone());
        }
        return BridgeVerdict::AwaitingGate { jobs };
    }
    from_status_only(&bridge.status, downstream)
}

/// Derive a verdict from the trigger job's status alone, for a bridge that was
/// not dived into.
fn from_status_only(
    status: &Status,
    downstream: &crate::model::DownstreamPipeline,
) -> BridgeVerdict {
    // With `strategy: depend` the trigger job mirrors the child. Without it the
    // trigger goes `success` the instant the child is created, so the child's
    // own status is the better signal when the two disagree.
    // A settled child under a still-live trigger job is the more useful answer:
    // a `pending` trigger whose child is `manual` is parked, not working.
    let disagrees = (status.is_live() && !downstream.status.is_live())
        || (status.is_success() && !downstream.status.is_success());
    let effective = if disagrees {
        &downstream.status
    } else {
        status
    };
    match effective {
        s if s.is_live() => BridgeVerdict::Running,
        Status::Success => BridgeVerdict::Passed,
        Status::Failed => BridgeVerdict::Failed { jobs: Vec::new() },
        Status::Canceled => BridgeVerdict::Canceled,
        Status::Skipped => BridgeVerdict::Skipped,
        Status::Manual => BridgeVerdict::AwaitingGate { jobs: Vec::new() },
        Status::Unknown(raw) => BridgeVerdict::Unknown {
            status: raw.clone(),
        },
        _ => BridgeVerdict::Unknown {
            status: effective.to_string(),
        },
    }
}
