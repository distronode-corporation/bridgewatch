//! The status vocabulary shared by pipelines, jobs and bridges, plus the
//! classification of a job into the class the verdict engine reasons about.
//!
//! GitLab adds statuses over time. [`Status`] therefore never fails to decode:
//! anything unrecognised lands in [`Status::Unknown`] carrying the raw string.
//!
//! The names are GitLab's because that is the vocabulary the view model, the
//! Rhai hook and every `expected.json` publish. A second provider folds its own
//! into them rather than adding a parallel enum: see [`Status::from_github`],
//! which maps GitHub's `status` plus `conclusion` pair onto these.

use serde::{Deserialize, Serialize};

/// A GitLab CI status, as reported for a pipeline, a job or a bridge.
///
/// Decoding is total: an unrecognised value becomes [`Status::Unknown`] rather
/// than an error, so a new GitLab status can never break a poll.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Status {
    /// The pipeline or job has been created but has not been queued.
    Created,
    /// Waiting for a resource group to free up.
    WaitingForResource,
    /// The runner is preparing the environment.
    Preparing,
    /// Queued, waiting for a runner.
    Pending,
    /// Executing.
    Running,
    /// Finished successfully.
    Success,
    /// Finished unsuccessfully.
    Failed,
    /// Cancellation has been requested but has not completed.
    Canceling,
    /// Cancelled before completing.
    Canceled,
    /// Never ran because its rules or a prior failure skipped it.
    Skipped,
    /// Awaiting a human: a manual job or a manual downstream pipeline.
    Manual,
    /// Scheduled to start later (a delayed job).
    Scheduled,
    /// Waiting for an external callback to report back.
    WaitingForCallback,
    /// A status this build of bridgewatch does not know, preserved verbatim.
    Unknown(String),
}

impl Status {
    /// The wire representation, exactly as GitLab spells it.
    pub fn as_str(&self) -> &str {
        match self {
            Status::Created => "created",
            Status::WaitingForResource => "waiting_for_resource",
            Status::Preparing => "preparing",
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Success => "success",
            Status::Failed => "failed",
            Status::Canceling => "canceling",
            Status::Canceled => "canceled",
            Status::Skipped => "skipped",
            Status::Manual => "manual",
            Status::Scheduled => "scheduled",
            Status::WaitingForCallback => "waiting_for_callback",
            Status::Unknown(s) => s,
        }
    }

    /// True for the statuses that mean "work is still expected to happen":
    /// `running`, `pending`, `created`, `waiting_for_resource`, `preparing`,
    /// `waiting_for_callback`, `scheduled` and `canceling`.
    ///
    /// `manual` is deliberately **not** live: a job parked at a gate will sit
    /// there forever and must not hold the fast poll interval open.
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            Status::Running
                | Status::Pending
                | Status::Created
                | Status::WaitingForResource
                | Status::Preparing
                | Status::WaitingForCallback
                | Status::Scheduled
                | Status::Canceling
        )
    }

    /// True for `canceled` and `skipped`: the work was never done, and that is
    /// not the same thing as having failed.
    pub fn is_not_built(&self) -> bool {
        matches!(self, Status::Canceled | Status::Skipped)
    }

    /// True for `manual`: a job or downstream pipeline awaiting a human.
    pub fn is_gate(&self) -> bool {
        matches!(self, Status::Manual)
    }

    /// True for `success`.
    pub fn is_success(&self) -> bool {
        matches!(self, Status::Success)
    }

    /// True for `failed`.
    pub fn is_failed(&self) -> bool {
        matches!(self, Status::Failed)
    }

    /// True when no further progress is expected without human action.
    ///
    /// A gate counts as settled: the poller must not spin at the fast interval
    /// because somebody has not pressed a button.
    pub fn is_settled(&self) -> bool {
        !self.is_live()
    }

    /// GitHub Actions splits one status into two fields, and neither is
    /// enumerated on a workflow run.
    ///
    /// `status` is the phase (`queued`, `in_progress`, `completed`, and the ones
    /// the documentation only half admits to: `requested`, `pending`, `waiting`),
    /// and `conclusion` is the outcome, which is `null` until the phase is
    /// `completed`. The pair is folded onto the same [`Status`] the rest of the
    /// engine reads, so every verdict rule, every icon rule and the whole view
    /// model work on a GitHub run without knowing it is one.
    ///
    /// ⛔ **`action_required` is a GATE, not a failure, and getting this wrong
    /// reds the tray on one run in eight.** A fork pull request from a first-time
    /// contributor produces a run that is `completed` with `conclusion:
    /// action_required` and **zero jobs**, waiting for a maintainer to press
    /// Approve. Reading "conclusion is not success, so it failed" turns every such
    /// run red; it is [`Status::Manual`], which `is_settled()` already treats as
    /// "no further progress without a human" and which the icon rules draw as
    /// `parked_gate`.
    ///
    /// ⛔ **`waiting` is the mirror image.** A run held by a deployment protection
    /// rule has `status: waiting` with a `null` conclusion, which on the conclusion
    /// alone is indistinguishable from a run that is executing. Mapping it anywhere
    /// [`Status::is_live`] answers true would hold the fast poll interval open all
    /// weekend for a run nobody will touch until Monday, which is the exact mistake
    /// `is_live`'s own documentation says this tool exists to avoid.
    ///
    /// ⚠️ **The run's pair is an OPEN vocabulary.** In GitHub's OpenAPI description
    /// `workflow-run.status` and `workflow-run.conclusion` are bare nullable
    /// strings with no `enum` at all (the 14-value list everyone quotes belongs to
    /// the *query parameter*, and spans both fields), which is how `startup_failure`
    /// and `stale` reach a run at all: they are enumerated only on a check suite,
    /// and the run mirrors its suite through an unconstrained string. Anything
    /// unrecognised therefore becomes [`Status::Unknown`] carrying the raw value,
    /// which the icon rules draw as `unknown` rather than as a wrong colour.
    ///
    /// `neutral` is the one judgement call: it means "ran, did not pass, does not
    /// count", which nothing in the API says is TOLERATED. `Unknown("neutral")` is
    /// the honest answer, and `[watches.jobs]` is how a user says what they want it
    /// to mean.
    pub fn from_github(status: &str, conclusion: Option<&str>) -> Status {
        // The phase decides first: a conclusion only exists once `completed`, and
        // a non-completed run carrying one would be the API contradicting itself.
        match status {
            "queued" => return Status::Pending,
            "in_progress" => return Status::Running,
            // Both are pre-queue phases GitHub reports for a run that has been
            // accepted but not yet scheduled.
            "requested" | "pending" => return Status::Created,
            "waiting" => return Status::Manual,
            "completed" => {}
            other => return Status::Unknown(other.to_string()),
        }
        match conclusion {
            Some("success") => Status::Success,
            // `startup_failure` is an invalid or unparseable workflow file, and
            // `timed_out` is the job limit: both are work that should have happened
            // and did not.
            Some("failure") | Some("startup_failure") | Some("timed_out") => Status::Failed,
            Some("cancelled") => Status::Canceled,
            // `stale` is GitHub's word for a run whose result was discarded before
            // it could count, which is what `skipped` already means here.
            Some("skipped") | Some("stale") => Status::Skipped,
            Some("action_required") => Status::Manual,
            Some(other) => Status::Unknown(other.to_string()),
            // Completed with no conclusion at all. Rare, and not something to
            // guess at: `Unknown` draws the question mark and says so.
            None => Status::Unknown("completed".to_string()),
        }
    }
}

impl From<String> for Status {
    fn from(s: String) -> Self {
        match s.as_str() {
            "created" => Status::Created,
            "waiting_for_resource" => Status::WaitingForResource,
            "preparing" => Status::Preparing,
            "pending" => Status::Pending,
            "running" => Status::Running,
            "success" => Status::Success,
            "failed" => Status::Failed,
            "canceling" | "cancelling" => Status::Canceling,
            "canceled" | "cancelled" => Status::Canceled,
            "skipped" => Status::Skipped,
            "manual" => Status::Manual,
            "scheduled" => Status::Scheduled,
            "waiting_for_callback" => Status::WaitingForCallback,
            _ => Status::Unknown(s),
        }
    }
}

impl From<Status> for String {
    fn from(s: Status) -> String {
        match s {
            Status::Unknown(raw) => raw,
            other => other.as_str().to_string(),
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a job means to the verdict engine, after `[watches.jobs]` overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobClass {
    /// `success`.
    Passed,
    /// Still running, queued or otherwise in flight.
    Live,
    /// `failed` with `allow_failure: false`, or promoted there by an override.
    BlockingFailure,
    /// `failed` with `allow_failure: true`, or demoted there by an override.
    WarningFailure,
    /// `manual`: awaiting a human.
    Gate,
    /// `canceled` or `skipped`.
    NotBuilt,
    /// A status this build does not recognise.
    Unknown,
    /// Hidden from every verdict by an `ignore` override.
    Ignored,
}

impl JobClass {
    /// True for the one class that can red a pipeline.
    pub fn is_blocking_failure(&self) -> bool {
        matches!(self, JobClass::BlockingFailure)
    }

    /// True for a failure that is explicitly tolerated.
    pub fn is_warning_failure(&self) -> bool {
        matches!(self, JobClass::WarningFailure)
    }

    /// True while the job is in flight.
    pub fn is_live(&self) -> bool {
        matches!(self, JobClass::Live)
    }

    /// True when the job is parked awaiting a human.
    pub fn is_gate(&self) -> bool {
        matches!(self, JobClass::Gate)
    }

    /// The lowercase name used in the view model and in Rhai scripts.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobClass::Passed => "passed",
            JobClass::Live => "live",
            JobClass::BlockingFailure => "blocking_failure",
            JobClass::WarningFailure => "warning_failure",
            JobClass::Gate => "gate",
            JobClass::NotBuilt => "not_built",
            JobClass::Unknown => "unknown",
            JobClass::Ignored => "ignored",
        }
    }
}

impl std::fmt::Display for JobClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
