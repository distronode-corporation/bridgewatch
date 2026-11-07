//! The full job view: every job, parent and child, with its stage and timing.
//!
//! The popover grew from "the jobs the verdict names" to "every job, grouped by
//! stage, with elapsed time". Everything it draws comes from `JobView`, so the
//! fields it needs are pinned here against recorded fixtures rather than
//! against hand-made structs.

mod support;

use bridgewatch_core::config::JobsMode;
use bridgewatch_core::config::edit::{Edit, EditValue};
use bridgewatch_core::model::Job;
use bridgewatch_core::verdict::{JobView, PipelineView, Snapshot};

async fn snapshot_for(fixture: &str, overrides: &[Edit]) -> Snapshot {
    let config = support::config_with(overrides);
    let (mut poller, _) = support::fixture_poller(&config, &support::fixtures_dir().join(fixture));
    poller.tick().await.snapshot
}

fn main_row(snapshot: &Snapshot) -> &PipelineView {
    snapshot
        .watches
        .iter()
        .find(|w| w.id == "main-push")
        .and_then(|w| w.rows.first())
        .expect("the main-push watch has a row")
}

fn find<'a>(jobs: &'a [JobView], name: &str) -> &'a JobView {
    jobs.iter()
        .find(|j| j.name == name)
        .unwrap_or_else(|| panic!("no job {name:?}"))
}

/// GitLab's own `duration` is decoded and carried through untouched, alongside
/// both timestamps. For a running job it is the elapsed time at response time,
/// which is what the popover's local tick starts from.
#[test]
fn the_wire_duration_and_timestamps_decode() {
    let job: Job = serde_json::from_str(
        r#"{"id":7,"name":"deploy:origins","status":"running","stage":"deploy",
            "started_at":"2026-09-17T14:40:34.699Z","finished_at":null,
            "duration":12.5,"web_url":"https://gitlab.com/x/-/jobs/7"}"#,
    )
    .unwrap();
    assert_eq!(job.duration, Some(12.5));
    assert_eq!(job.started_at.as_deref(), Some("2026-09-17T14:40:34.699Z"));
    assert_eq!(job.finished_at, None);

    let bare: Job = serde_json::from_str(r#"{"id":8,"name":"x","status":"manual"}"#).unwrap();
    assert_eq!(bare.duration, None, "absent is None, never 0");
}

/// A recorded fixture carries no `duration` (the allow-list predates it), so
/// the view derives it from the two timestamps. `deploy:origins` in ca41ab28
/// ran 14:40:34.699 → 14:41:07.879, which is 33.18 s.
#[tokio::test]
async fn child_jobs_carry_stage_timestamps_and_duration() {
    let snapshot = snapshot_for("ca41ab28-deployed-with-failure", &[]).await;
    let row = main_row(&snapshot);
    let website = row
        .bridges
        .iter()
        .find(|b| b.name == "trigger:website")
        .expect("trigger:website");

    let origins = find(&website.jobs, "deploy:origins");
    assert_eq!(origins.stage.as_deref(), Some("deploy"));
    assert_eq!(
        origins.started_at.as_deref(),
        Some("2026-09-17T14:40:34.699Z")
    );
    assert_eq!(
        origins.finished_at.as_deref(),
        Some("2026-09-17T14:41:07.879Z")
    );
    let d = origins.duration.expect("derived from the timestamps");
    assert!((d - 33.18).abs() < 0.001, "{d}");

    // A job that never ran has no start, no finish and no duration: absent,
    // not zero, so the UI prints nothing rather than "0s".
    let shard = find(&website.jobs, "verify:web_coverage_full 1/4");
    assert_eq!(shard.started_at, None);
    assert_eq!(shard.duration, None);
}

/// The parent's own jobs get the same treatment.
#[tokio::test]
async fn parent_jobs_carry_stage_timestamps_and_duration() {
    let snapshot = snapshot_for("ca41ab28-deployed-with-failure", &[]).await;
    let row = main_row(&snapshot);
    let job = find(&row.parent_jobs, "verify:secret_detection_coverage");
    assert_eq!(job.stage.as_deref(), Some("scan-verify"));
    let d = job.duration.unwrap();
    assert!((d - 9.39).abs() < 0.001, "{d}");
}

/// The view model carries EVERY job, not only the ones the verdict names: the
/// child had 33 jobs, and the example config ignores exactly one of them
/// (`kics-iac-sast`), so 32 reach the popover. The parent's 7 all do.
#[tokio::test]
async fn the_view_model_carries_every_job() {
    let snapshot = snapshot_for("ca41ab28-deployed-with-failure", &[]).await;
    let row = main_row(&snapshot);
    assert_eq!(row.parent_jobs.len(), 7);
    let website = row
        .bridges
        .iter()
        .find(|b| b.name == "trigger:website")
        .unwrap();
    assert_eq!(website.jobs.len(), 32);
    assert!(website.jobs.iter().all(|j| j.name != "kics-iac-sast"));
    // Far more than the verdict names, which is the point of the full view.
    assert!(website.jobs.len() > website.verdict_jobs.len() + 20);
}

/// A duration GitLab sent wins over the derived one.
#[test]
fn a_wire_duration_wins_over_the_derived_one() {
    let job: Job = serde_json::from_str(
        r#"{"id":7,"name":"a","status":"success",
            "started_at":"2026-09-17T14:40:00Z","finished_at":"2026-09-17T14:41:00Z",
            "duration":58.25}"#,
    )
    .unwrap();
    assert_eq!(bridgewatch_core::verdict::job_duration(&job), Some(58.25));

    let derived = Job {
        duration: None,
        ..job.clone()
    };
    assert_eq!(
        bridgewatch_core::verdict::job_duration(&derived),
        Some(60.0)
    );

    // Unparseable or reversed timestamps derive nothing rather than a nonsense
    // negative number.
    let reversed = Job {
        duration: None,
        started_at: Some("2026-09-17T14:41:00Z".into()),
        finished_at: Some("2026-09-17T14:40:00Z".into()),
        ..job.clone()
    };
    assert_eq!(bridgewatch_core::verdict::job_duration(&reversed), None);
    let garbage = Job {
        duration: None,
        started_at: Some("yesterday".into()),
        ..job
    };
    assert_eq!(bridgewatch_core::verdict::job_duration(&garbage), None);
}

// ---------------------------------------------------------------------------
// ui.jobs / watches.show.jobs
// ---------------------------------------------------------------------------

/// With nothing configured every watch shows every job: "all" is the 0.1.0
/// default.
#[tokio::test]
async fn every_watch_carries_its_effective_jobs_mode_defaulting_to_all() {
    let snapshot = snapshot_for("ca41ab28-deployed-with-failure", &[]).await;
    assert!(!snapshot.watches.is_empty());
    for w in &snapshot.watches {
        assert_eq!(w.jobs, JobsMode::All, "{}", w.id);
    }
}

/// `ui.jobs` sets the default for every watch, and `watches.show.jobs` overrides
/// it for one.
#[tokio::test]
async fn a_per_watch_override_beats_the_global_mode() {
    let snapshot = snapshot_for(
        "ca41ab28-deployed-with-failure",
        &[
            Edit::Set {
                path: "ui.jobs".into(),
                value: EditValue::String("failures".into()),
            },
            Edit::SetWatch {
                id: "hourly".into(),
                path: "show.jobs".into(),
                value: EditValue::String("all".into()),
            },
        ],
    )
    .await;
    let mode = |id: &str| {
        snapshot
            .watches
            .iter()
            .find(|w| w.id == id)
            .unwrap_or_else(|| panic!("{id}"))
            .jobs
    };
    assert_eq!(mode("main-push"), JobsMode::Failures);
    assert_eq!(mode("preflights"), JobsMode::Failures);
    assert_eq!(mode("hourly"), JobsMode::All);
}

/// The mode is presentation only: `failures` must not drop a single job from
/// the view model, or switching back to `all` would show nothing until the next
/// fetch.
#[tokio::test]
async fn failures_mode_still_carries_every_job() {
    let snapshot = snapshot_for(
        "ca41ab28-deployed-with-failure",
        &[Edit::Set {
            path: "ui.jobs".into(),
            value: EditValue::String("failures".into()),
        }],
    )
    .await;
    let row = main_row(&snapshot);
    assert_eq!(row.parent_jobs.len(), 7);
}

/// The wire spelling is the config spelling.
#[test]
fn the_jobs_mode_serialises_as_the_config_word() {
    let w = |m: JobsMode| serde_json::to_string(&m).unwrap();
    assert_eq!(w(JobsMode::All), "\"all\"");
    assert_eq!(w(JobsMode::Failures), "\"failures\"");
}

// ---------------------------------------------------------------------------
// The first failing line
// ---------------------------------------------------------------------------

/// GitLab's `failure_reason` is already in the jobs list bridgewatch fetches;
/// a failed job carries it in words, and nothing else carries anything.
#[tokio::test]
async fn a_failed_gitlab_job_carries_its_failure_reason_in_words() {
    let snapshot = snapshot_for("ca41ab28-deployed-with-failure", &[]);
    let snapshot = snapshot.await;
    let row = main_row(&snapshot);
    let jobs: Vec<&JobView> = row
        .parent_jobs
        .iter()
        .chain(row.bridges.iter().flat_map(|b| b.jobs.iter()))
        .collect();
    let android = jobs
        .iter()
        .find(|j| j.name == "verify:android")
        .expect("the failed android job is listed");
    assert_eq!(android.failure.as_deref(), Some("script failure"));
    let others: Vec<&str> = jobs
        .iter()
        .filter(|j| j.status != "failed")
        .filter(|j| j.failure.is_some())
        .map(|j| j.name.as_str())
        .collect();
    assert!(others.is_empty(), "only a failed job says why: {others:?}");
}

/// The line is one line, at the file and line GitHub pointed at, and never a
/// wall: the message is cut to its first line and to `FAILURE_LINE_MAX`.
#[test]
fn a_failing_line_is_one_short_line_at_its_location() {
    use bridgewatch_core::model::{Annotation, FAILURE_LINE_MAX};
    let at = |path: Option<&str>, line: Option<u64>, message: &str| {
        Annotation {
            path: path.map(str::to_string),
            start_line: line,
            message: message.to_string(),
        }
        .line()
    };
    assert_eq!(
        at(Some("src/a.rs"), Some(7), "\n  boom  \nsecond line").as_deref(),
        Some("src/a.rs:7: boom")
    );
    assert_eq!(
        at(Some("src/a.rs"), None, "boom").as_deref(),
        Some("src/a.rs: boom")
    );
    assert_eq!(
        at(
            Some(".github"),
            Some(1),
            "Process completed with exit code 1."
        )
        .as_deref(),
        Some("Process completed with exit code 1."),
        "GitHub's placeholder path points at nothing worth naming"
    );
    assert_eq!(at(None, Some(3), "boom").as_deref(), Some("boom"));
    assert_eq!(
        at(Some("a"), Some(1), " \n\t"),
        None,
        "nothing said, no line"
    );

    let long = at(Some("a"), Some(1), &"x".repeat(500)).unwrap();
    let message = long.strip_prefix("a:1: ").unwrap();
    assert_eq!(message.chars().count(), FAILURE_LINE_MAX);
    assert!(message.ends_with('\u{2026}'), "{message}");

    let job: Job = serde_json::from_str(
        r#"{"id":1,"name":"a","status":"failed","failure_reason":"stuck_or_timeout_failure"}"#,
    )
    .unwrap();
    assert_eq!(
        job.failure_line().as_deref(),
        Some("stuck or timeout failure")
    );
    let passed = Job {
        status: bridgewatch_core::status::Status::Success,
        ..job.clone()
    };
    assert_eq!(
        passed.failure_line(),
        None,
        "a reason on a job that passed is not shown"
    );
    let annotated = Job {
        annotation: Some(Annotation {
            path: Some("src/a.rs".into()),
            start_line: Some(3),
            message: "boom".into(),
        }),
        ..job
    };
    assert_eq!(
        annotated.failure_line().as_deref(),
        Some("src/a.rs:3: boom"),
        "an annotation says more than a reason code"
    );
}
