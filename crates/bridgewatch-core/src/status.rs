//! The status vocabulary shared by pipelines, jobs and bridges, plus the
//! classification of a job into the class the verdict engine reasons about.
//!
//! GitLab adds statuses over time. [`Status`] therefore never fails to decode:
//! anything unrecognised lands in [`Status::Unknown`] carrying the raw string.

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
