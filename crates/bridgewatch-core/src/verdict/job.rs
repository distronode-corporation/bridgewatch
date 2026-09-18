//! Turning a raw job into the class the verdict engine reasons about.

use crate::config::{JobOverride, WatchRules};
use crate::model::Job;
use crate::status::{JobClass, Status};

/// Classify a job, applying the watch's `[watches.jobs]` overrides.
///
/// An override wins over the raw status, but only where that is meaningful:
/// `warning`, `blocking` and `gate` re-read a job that is already in a failed or
/// manual state, and `ignore` removes the job from every verdict whatever its
/// state. Promoting a *successful* job to `blocking` would be nonsense, so the
/// overrides deliberately do not reach it.
pub fn classify(job: &Job, rules: &WatchRules) -> JobClass {
    classify_parts(&job.name, &job.status, job.allow_failure, rules)
}

/// The same classification, for anything that has a name, a status and an
/// `allow_failure` flag but is not a [`Job`].
///
/// A bridge is such a thing: GitLab returns trigger jobs from a separate
/// endpoint with the same shape, and until 2026-09-17 nothing ran the override
/// table over one, so `"trigger:android" = "ignore"` did nothing at all.
pub fn classify_parts(
    name: &str,
    status: &Status,
    allow_failure: bool,
    rules: &WatchRules,
) -> JobClass {
    let override_ = rules.job_override(name);

    if override_ == Some(JobOverride::Ignore) {
        return JobClass::Ignored;
    }

    let base = base_class(status, allow_failure);

    match (override_, base) {
        // A failure can be re-read in either direction.
        (Some(JobOverride::Warning), JobClass::BlockingFailure) => JobClass::WarningFailure,
        (Some(JobOverride::Blocking), JobClass::WarningFailure) => JobClass::BlockingFailure,
        // `gate` marks a manual job as a deliberate park rather than as work
        // that never happened, which is the difference between a pipeline that
        // is waiting and one that is broken.
        //
        // ⛔ It deliberately does **not** reach a job that RAN AND FAILED. The
        // shipped example applies `gate` to `re:^verify:web_coverage_full`,
        // whose shards are manual on `main` and automatic on the weekly
        // schedule; an arm that also swallowed `BlockingFailure` meant a shard
        // that genuinely ran and failed was listed under `gates`, could never
        // red the icon, and on a project without bridges left the tray on the
        // outline check with exit 0.
        (Some(JobOverride::Gate), JobClass::Gate) => JobClass::Gate,
        (Some(JobOverride::Gate), JobClass::NotBuilt) => JobClass::Gate,
        _ => base,
    }
}

/// The class a job has before any override.
pub fn base_class(status: &Status, allow_failure: bool) -> JobClass {
    if status.is_live() {
        return JobClass::Live;
    }
    match status {
        Status::Success => JobClass::Passed,
        Status::Failed => {
            if allow_failure {
                JobClass::WarningFailure
            } else {
                JobClass::BlockingFailure
            }
        }
        Status::Manual => JobClass::Gate,
        Status::Canceled | Status::Skipped => JobClass::NotBuilt,
        Status::Unknown(_) => JobClass::Unknown,
        // Every live status is handled above.
        _ => JobClass::Unknown,
    }
}
