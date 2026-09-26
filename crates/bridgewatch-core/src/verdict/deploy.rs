//! Deriving "did this deploy?" from configurable marker jobs.
//!
//! GitLab has no notion of a deploy. A pipeline is green or it is not, and on a
//! parent/child estate "green" frequently means "the bridges were created".
//! bridgewatch therefore asks the user which job names *mean* deployed, and
//! looks for them across the parent and every child it walked into.

use serde::{Deserialize, Serialize};

use crate::config::WatchRules;
use crate::model::Job;
use crate::status::JobClass;

use super::job::classify;

/// Where a marker job was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerJob {
    /// Job name.
    pub name: String,
    /// Job id.
    pub id: u64,
    /// The pipeline the job belongs to: the parent, or a child's id.
    pub pipeline_id: u64,
    /// Link to the job.
    pub web_url: Option<String>,
    /// When it started, for the post-deploy comparison.
    pub started_at: Option<String>,
}

/// What the deploy markers say happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeployOutcome {
    /// No marker job exists anywhere that was looked. Either this pipeline does
    /// not deploy, or the lane that would have deployed never ran.
    Absent,
    /// A marker is running, or is queued behind a live pipeline, or a lane that
    /// could carry one has not been read yet.
    InProgress {
        /// The marker being waited on, or the trigger job whose unread child
        /// could hold one.
        marker: String,
    },
    /// A marker succeeded. This is the only outcome that means "it is out".
    Live {
        /// The marker that succeeded.
        marker: Box<MarkerJob>,
    },
    /// A blocking failure in the marker's own pipeline, before any marker
    /// succeeded.
    Failed {
        /// The failing jobs in that pipeline.
        jobs: Vec<String>,
    },
    /// The pipeline that would have carried the marker was never created.
    Dead,
    /// Every marker was cancelled or skipped.
    Canceled,
    /// A marker exists in a state this build of bridgewatch cannot read as any
    /// of the above — a GitLab status added since it was compiled, say.
    ///
    /// This used to collapse to [`DeployOutcome::Absent`], whose comment said
    /// "say so rather than guessing" and then guessed: absent plus a settled
    /// green pipeline is rule 7, the outline check. "I do not know" has to be
    /// its own answer or it is indistinguishable from "it is fine".
    Unknown,
}

impl DeployOutcome {
    /// The single word used in the view model, in `expected.json` and in Rhai.
    pub fn word(&self) -> &'static str {
        match self {
            DeployOutcome::Absent => "absent",
            DeployOutcome::InProgress { .. } => "in_progress",
            DeployOutcome::Live { .. } => "live",
            DeployOutcome::Failed { .. } => "failed",
            DeployOutcome::Dead => "dead",
            DeployOutcome::Canceled => "canceled",
            DeployOutcome::Unknown => "unknown",
        }
    }

    /// The marker job, when one succeeded.
    pub fn marker(&self) -> Option<&MarkerJob> {
        match self {
            DeployOutcome::Live { marker } => Some(marker),
            _ => None,
        }
    }

    /// True when a marker succeeded.
    pub fn is_live(&self) -> bool {
        matches!(self, DeployOutcome::Live { .. })
    }

    /// True when no marker job was found at all.
    pub fn is_absent(&self) -> bool {
        matches!(self, DeployOutcome::Absent)
    }
}

/// One pipeline's jobs, tagged with which pipeline they came from.
pub struct JobScope<'a> {
    /// The pipeline id these jobs belong to.
    pub pipeline_id: u64,
    /// Whether that pipeline is itself still live.
    pub pipeline_live: bool,
    /// The jobs.
    pub jobs: &'a [Job],
}

/// Work out the deploy outcome over every scope that was fetched.
///
/// `any_dead_bridge` distinguishes "this pipeline does not deploy" from "the
/// lane that deploys was never created", which look identical from the job list
/// alone and mean opposite things.
///
/// `unread_live_carrier` names a bridge that is still live, that the dive rules
/// would walk into, and whose child's jobs have not been read: its child has
/// not been created yet, or the request for it failed. A marker in there is one
/// nobody has seen, so it holds a success elsewhere back exactly as a marker
/// that is seen and still queued does.
pub fn outcome(
    scopes: &[JobScope<'_>],
    rules: &WatchRules,
    any_dead_bridge: bool,
    unread_live_carrier: Option<&str>,
) -> DeployOutcome {
    if rules.deploy_markers.is_empty() {
        return DeployOutcome::Absent;
    }

    // Gather every marker job, remembering its scope.
    let mut found: Vec<(MarkerJob, JobClass, usize)> = Vec::new();
    for (scope_index, scope) in scopes.iter().enumerate() {
        for job in scope.jobs {
            if !rules.is_deploy_marker(&job.name) {
                continue;
            }
            let class = classify(job, rules);
            if class == JobClass::Ignored {
                continue;
            }
            found.push((
                MarkerJob {
                    name: job.name.clone(),
                    id: job.id,
                    pipeline_id: scope.pipeline_id,
                    web_url: job.web_url.clone(),
                    started_at: job.started_at.clone(),
                },
                class,
                scope_index,
            ));
        }
    }

    if found.is_empty() {
        return if any_dead_bridge {
            DeployOutcome::Dead
        } else {
            DeployOutcome::Absent
        };
    }

    // A marker parked at a gate has not deployed and is not going to without a
    // human, so it does not make the outcome "in progress". Treating an
    // all-gate set as absent is what lets the icon read `parked_gate` rather
    // than spinning on a deploy that nobody has authorised.
    if found.iter().all(|(_, c, _)| c.is_gate()) {
        return DeployOutcome::Absent;
    }

    // First success wins, in the order the markers are configured: the config's
    // order is the user's statement of which marker means "deployed" when more
    // than one succeeds.
    for pattern in &rules.deploy_markers {
        let mut candidates: Vec<&(MarkerJob, JobClass, usize)> = found
            .iter()
            .filter(|(m, c, _)| *c == JobClass::Passed && pattern.matches(&m.name))
            .collect();
        candidates.sort_by(|a, b| {
            (a.0.started_at.as_deref(), a.0.id).cmp(&(b.0.started_at.as_deref(), b.0.id))
        });
        if let Some((marker, _, _)) = candidates.first() {
            // ⛔ "It is out" only once no configured marker can still move.
            // First-success-wins used to return here unconditionally, so on a
            // real push with `["deploy:origins", "deploy:marketing"]` the
            // marketing marker finished ~30 s in while `deploy:origins` sat
            // `created` behind the website chain, and the tray drew the filled
            // check and fired `deployed` twenty minutes before the fleet had the
            // new code. Which marker is REPORTED is still decided above, by
            // config order and then start time, once nothing is left to wait on.
            // ⛔ Here a live-class marker holds the answer back only while
            // its own pipeline is live. GitLab can leave a job `created` for
            // good inside a pipeline that has SETTLED (a `needs:` on an
            // optional manual job nobody pressed), and without this a finished
            // pipeline with one such marker would never read "deployed".
            if let Some(waiting) = still_moving(&found, scopes, true) {
                return DeployOutcome::InProgress {
                    marker: waiting.to_string(),
                };
            }
            if let Some(bridge) = unread_live_carrier {
                return DeployOutcome::InProgress {
                    marker: bridge.to_string(),
                };
            }
            return DeployOutcome::Live {
                marker: Box::new((*marker).clone()),
            };
        }
    }

    // Nothing succeeded. Is one on its way?
    // Unguarded on purpose: with nothing succeeded, a live-class marker is
    // still the best answer there is, and every arm below it reads failed,
    // cancelled or skipped markers only, so a stray `created` one would fall
    // through to `unknown` instead. That is a separate question from the hold
    // above and is left as it was.
    if let Some(waiting) = still_moving(&found, scopes, false) {
        return DeployOutcome::InProgress {
            marker: waiting.to_string(),
        };
    }

    // A blocking failure in a marker's own pipeline is what stopped it.
    let marker_scopes: Vec<usize> = found.iter().map(|(_, _, i)| *i).collect();
    let mut blocking: Vec<String> = Vec::new();
    for index in &marker_scopes {
        for job in scopes[*index].jobs {
            if classify(job, rules).is_blocking_failure() {
                blocking.push(job.name.clone());
            }
        }
    }
    // ⛔ A marker that failed is one of those even when GitLab tolerated it.
    // `allow_failure: true` on a deploy job means the PIPELINE stays green; it
    // does not mean the deploy happened, and reading it as "nothing to report"
    // is how a tray shows the outline check over a release that never shipped.
    for (marker, class, _) in &found {
        if class.is_warning_failure() {
            blocking.push(marker.name.clone());
        }
    }
    blocking.sort();
    blocking.dedup();
    if !blocking.is_empty() {
        return DeployOutcome::Failed { jobs: blocking };
    }

    if found
        .iter()
        .all(|(_, c, _)| *c == JobClass::NotBuilt || c.is_gate())
    {
        return DeployOutcome::Canceled;
    }

    // A marker that is neither live, successful, failed nor cancelled is one
    // bridgewatch does not understand. Say so rather than guessing "deployed".
    DeployOutcome::Unknown
}

/// The first marker that can still change: one that is live, or one whose
/// class this build cannot read inside a pipeline that is still live.
///
/// ⛔ Only a marker whose own class is unreadable may be called "in progress"
/// because its pipeline is still live. That arm used to accept any class,
/// which meant a marker that had already FAILED read as `in_progress` for as
/// long as the longest unrelated job in its pipeline kept running — so the
/// icon said "running", `failures` was empty, and the `blocking_failure`
/// notification was delayed by a job that had nothing to do with the deploy.
/// README rule 5 says a failed marker outranks everything; this is the line
/// that makes it true.
///
/// A gate is not moving: it will sit there until a human presses it, and
/// holding "deployed" back for it would never let a lane with an optional
/// manual marker read as out.
///
/// `live_scope_only` makes a live-class marker count only when its own
/// pipeline is live too; see the caller holding back a success.
fn still_moving<'a>(
    found: &'a [(MarkerJob, JobClass, usize)],
    scopes: &[JobScope<'_>],
    live_scope_only: bool,
) -> Option<&'a str> {
    found
        .iter()
        .find(|(_, c, i)| c.is_live() && (!live_scope_only || scopes[*i].pipeline_live))
        .or_else(|| {
            found
                .iter()
                .find(|(_, c, i)| *c == JobClass::Unknown && scopes[*i].pipeline_live)
        })
        .map(|(m, _, _)| m.name.as_str())
}

/// Blocking failures that started **after** the winning marker did.
///
/// The comparison is on `started_at`, falling back to job id when it is null —
/// a job that never ran has no start time, and ids are monotonic per instance.
pub fn post_deploy_failures(
    scopes: &[JobScope<'_>],
    rules: &WatchRules,
    marker: &MarkerJob,
) -> Vec<String> {
    let marker_key = (marker.started_at.as_deref(), marker.id);
    let mut out = Vec::new();
    for scope in scopes {
        for job in scope.jobs {
            if job.id == marker.id {
                continue;
            }
            if !classify(job, rules).is_blocking_failure() {
                continue;
            }
            if job.order_key() > marker_key {
                out.push(job.name.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Blocking failures in the marker's **own** pipeline that did *not* start after
/// it — the ones [`post_deploy_failures`] deliberately excludes.
///
/// ⛔ Without this, a lane that broke on its way to a marker that nonetheless
/// succeeded reads as a clean deploy. On this project: `build:ecr_mirror_marketing`
/// fails at 06:46, `deploy:marketing` is skipped, `deploy:origins` succeeds at
/// 06:57 because nothing in its `needs:` chain touched the mirror. GitLab is red,
/// the marketing site did not ship, and the tray showed the filled check — because
/// `sibling_failures` excludes the marker's own bridge and `post_deploy_failures`
/// sorts the failure before the marker.
///
/// A job with a null `started_at` sorts before every job that ran, so anything
/// skipped by the failure lands here too, which is the right side of the line.
pub fn marker_scope_failures(
    scopes: &[JobScope<'_>],
    rules: &WatchRules,
    marker: &MarkerJob,
) -> Vec<String> {
    let marker_key = (marker.started_at.as_deref(), marker.id);
    let mut out = Vec::new();
    for scope in scopes {
        if scope.pipeline_id != marker.pipeline_id {
            continue;
        }
        for job in scope.jobs {
            if job.id == marker.id {
                continue;
            }
            if !classify(job, rules).is_blocking_failure() {
                continue;
            }
            if job.order_key() <= marker_key {
                out.push(job.name.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}
