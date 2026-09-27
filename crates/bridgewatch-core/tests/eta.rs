//! Deploy ETA: how long a deploy usually takes, learned from the watch's own
//! history, and shown only while it is still worth showing.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bridgewatch_core::config::edit::{Edit, EditValue};
use bridgewatch_core::eta::{
    Eta, EtaHistory, MAX_SAMPLES, deploy_sample, estimate, median, seconds_between,
};
use bridgewatch_core::poll::Poller;
use bridgewatch_core::verdict::PipelineView;
use chrono::{DateTime, Duration, Utc};

/// A path in the temp dir, unique to one call and removed when dropped: a file,
/// or a directory with everything in it.
///
/// Unique by a counter as well as the pid, for the reason `tests/notify.rs`'s
/// `TempLedger` gives: the tests in one binary share a pid and run in parallel.
struct TempPath(PathBuf);

impl TempPath {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("{name}-{}-{n}", std::process::id())))
    }

    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = std::fs::remove_file(&self.0);
    }
}

/// When the synthetic pipeline below was created.
const CREATED: &str = "2026-01-05T10:00:00Z";

fn created() -> DateTime<Utc> {
    CREATED.parse().expect("a valid time")
}

/// A one-pipeline GitLab fixture, written to a temp dir: a running push on
/// `main` whose only job is `job` in `status`. Synthetic, like every
/// `synth-*` fixture: nothing in it was recorded.
fn running_fixture(job: &str, status: &str) -> TempPath {
    let dir = TempPath::new("bw-eta-fixture");
    let root = dir.path();
    std::fs::create_dir_all(&root).expect("temp dir");
    let write = |name: &str, value: serde_json::Value| {
        std::fs::write(root.join(name), value.to_string()).expect("fixture file");
    };
    write(
        "fixture.json",
        serde_json::json!({ "primary": 900, "project": 82468124, "source": "synthetic: eta tests" }),
    );
    write(
        "list.json",
        serde_json::json!([{
            "id": 900, "iid": 9, "project_id": 82468124,
            "sha": "0123456789abcdef0123456789abcdef01234567", "ref": "main",
            "status": "running", "source": "push",
            "created_at": CREATED, "updated_at": CREATED
        }]),
    );
    write(
        "jobs.json",
        serde_json::json!([{
            "id": 9001, "name": job, "status": status, "stage": "deploy",
            "allow_failure": false, "started_at": CREATED
        }]),
    );
    write("bridges.json", serde_json::json!([]));
    dir
}

/// A history file holding `secs` for the example's primary watch, one sample
/// per pipeline id from 1.
fn seeded_history(secs: &[u64]) -> TempPath {
    let file = TempPath::new("bw-eta-history");
    let mut history = EtaHistory::default();
    for (i, s) in secs.iter().enumerate() {
        assert!(history.record("main-push", i as u64 + 1, *s));
    }
    history.save_merged(&file.path()).expect("history saves");
    file
}

fn poller_over(dir: &Path, edits: &[Edit]) -> Poller {
    support::fixture_poller(&support::config_with(edits), dir).0
}

fn main_row(snapshot: &bridgewatch_core::Snapshot) -> &PipelineView {
    snapshot
        .watches
        .iter()
        .find(|w| w.id == "main-push")
        .and_then(|w| w.rows.first())
        .expect("main-push has a row")
}

fn set_main(path: &str, value: EditValue) -> Edit {
    Edit::SetWatch {
        id: "main-push".into(),
        path: path.into(),
        value,
    }
}

// --- The pure half -------------------------------------------------------

#[test]
fn the_median_of_an_odd_count_is_the_middle_sample_whatever_the_order() {
    assert_eq!(median(&[900, 300, 600]), Some(600));
    assert_eq!(median(&[5]), Some(5));
    assert_eq!(median(&[]), None);
}

#[test]
fn the_median_of_an_even_count_is_the_mean_of_the_middle_two_rounded_down() {
    assert_eq!(median(&[100, 400, 200, 300]), Some(250));
    assert_eq!(median(&[1, 2]), Some(1));
    // Widened for the sum: two samples at the top of the range do not wrap.
    assert_eq!(median(&[u64::MAX, u64::MAX]), Some(u64::MAX));
}

#[test]
fn fewer_than_three_samples_give_no_estimate_and_three_do() {
    assert_eq!(estimate(&[], 10), None);
    assert_eq!(estimate(&[600, 660], 10), None);
    assert_eq!(
        estimate(&[600, 660, 720], 10),
        Some(Eta {
            typical_secs: 660,
            elapsed_secs: 10
        })
    );
}

#[test]
fn an_estimate_is_hidden_once_elapsed_passes_twice_the_median_and_not_a_second_before() {
    let samples = [600, 660, 720];
    assert_eq!(
        estimate(&samples, 1320),
        Some(Eta {
            typical_secs: 660,
            elapsed_secs: 1320
        }),
        "exactly twice the median is still an estimate"
    );
    assert_eq!(estimate(&samples, 1321), None);
}

#[test]
fn clock_skew_that_makes_elapsed_negative_reads_as_zero_rather_than_hiding_the_estimate() {
    assert_eq!(
        estimate(&[600, 660, 720], -4),
        Some(Eta {
            typical_secs: 660,
            elapsed_secs: 0
        })
    );
}

#[test]
fn a_sample_is_the_time_from_the_push_to_the_marker_finishing() {
    assert_eq!(
        deploy_sample(
            Some("2026-09-17T06:44:27.662Z"),
            Some("2026-09-17T06:57:47.209Z")
        ),
        Some(799)
    );
    // Offsets are honoured, not compared as strings.
    assert_eq!(
        seconds_between("2026-01-01T10:00:00+01:00", "2026-01-01T09:05:00Z"),
        Some(300)
    );
}

#[test]
fn a_missing_or_unreadable_time_is_no_sample_rather_than_a_guess() {
    assert_eq!(deploy_sample(Some(CREATED), None), None);
    assert_eq!(deploy_sample(None, Some(CREATED)), None);
    assert_eq!(deploy_sample(Some("yesterday"), Some(CREATED)), None);
}

#[test]
fn a_zero_or_negative_duration_is_dropped_as_a_bad_sample() {
    assert_eq!(deploy_sample(Some(CREATED), Some(CREATED)), None);
    assert_eq!(
        deploy_sample(Some("2026-01-05T10:00:10Z"), Some(CREATED)),
        None
    );
}

// --- The history ---------------------------------------------------------

#[test]
fn one_pipeline_is_one_sample_however_often_it_is_recorded() {
    let mut history = EtaHistory::default();
    assert!(history.record("w", 7, 600));
    assert!(!history.record("w", 7, 900), "the second is a no-op");
    assert!(!history.record("w", 8, 0), "zero is not a sample");
    assert_eq!(history.samples("w"), [600]);
    assert!(history.samples("other").is_empty());
}

#[test]
fn a_watch_keeps_the_newest_twenty_pipelines_and_an_older_one_is_trimmed_on_arrival() {
    let mut history = EtaHistory::default();
    for id in 1..=25u64 {
        history.record("w", id, id * 10);
    }
    assert_eq!(history.samples("w").len(), MAX_SAMPLES);
    assert!(!history.contains("w", 5));
    assert!(history.contains("w", 6) && history.contains("w", 25));
    // Pipeline 3 is still on somebody's screen: recording it again must not
    // push out a newer one, or it would come back on every tick.
    assert!(!history.record("w", 3, 30));
    assert!(history.contains("w", 6));
}

#[test]
fn a_missing_file_is_an_empty_history() {
    let file = TempPath::new("bw-eta-missing");
    assert_eq!(EtaHistory::load(&file.path()), EtaHistory::default());
}

#[test]
fn a_corrupt_file_is_an_empty_history_and_the_next_save_replaces_it() {
    let file = TempPath::new("bw-eta-corrupt");
    std::fs::write(
        file.path(),
        "{\"watches\": {\"w\": [{\"pipeline_id\": 1, \"secs\": -5}]}",
    )
    .expect("write");
    let mut history = EtaHistory::load(&file.path());
    assert_eq!(history, EtaHistory::default());

    history.record("w", 2, 60);
    history.save_merged(&file.path()).expect("saves over it");
    assert_eq!(EtaHistory::load(&file.path()).samples("w"), [60]);
}

#[test]
fn saving_merges_what_another_process_wrote_and_leaves_no_temp_file_behind() {
    let dir = TempPath::new("bw-eta-merge");
    std::fs::create_dir_all(dir.path()).expect("dir");
    let file = dir.path().join("eta.json");

    // The app learned pipeline 1 ...
    let mut app = EtaHistory::default();
    app.record("w", 1, 600);
    app.save_merged(&file).expect("app saves");
    // ... and a `check` that loaded before it learned pipeline 2.
    let mut cli = EtaHistory::default();
    cli.record("w", 2, 700);
    cli.save_merged(&file).expect("cli saves");

    assert_eq!(EtaHistory::load(&file).samples("w"), [600, 700]);
    assert_eq!(
        cli.samples("w"),
        [600, 700],
        "the saver learns the union too"
    );
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["eta.json"]);
}

// --- Through the poller --------------------------------------------------

#[tokio::test]
async fn a_deployed_pipeline_is_recorded_once_from_its_push_to_its_marker_finishing() {
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let mut poller = poller_over(&dir, &[]);
    poller.tick().await;
    // `created_at` 06:44:27.662, `deploy:origins` finished 06:57:47.209.
    assert_eq!(poller.eta_history().samples("main-push"), [799]);
    poller.tick().await;
    assert_eq!(poller.eta_history().samples("main-push"), [799]);
    assert!(
        poller.eta_history().samples("hourly").is_empty(),
        "a watch with no deployed row learns nothing"
    );
}

#[tokio::test]
async fn a_secondary_watch_with_deploy_markers_learns_too() {
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let mut poller = poller_over(
        &dir,
        &[set_main("role", EditValue::String("secondary".into()))],
    );
    poller.tick().await;
    assert_eq!(poller.eta_history().samples("main-push"), [799]);
}

#[tokio::test]
async fn a_watch_with_eta_off_neither_records_nor_shows_an_estimate() {
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let mut poller = poller_over(&dir, &[set_main("eta", EditValue::Boolean(false))]);
    poller.tick().await;
    assert!(poller.eta_history().samples("main-push").is_empty());

    let fixture = running_fixture("deploy:origins", "running");
    let history = seeded_history(&[600, 660, 720]);
    let mut poller = poller_over(
        &fixture.path(),
        &[set_main("eta", EditValue::Boolean(false))],
    )
    .with_eta_history(history.path())
    .with_clock(|| created() + Duration::seconds(390));
    let tick = poller.tick().await;
    assert_eq!(main_row(&tick.snapshot).eta, None);
}

#[tokio::test]
async fn a_row_whose_marker_is_running_shows_the_median_and_how_far_in_it_is() {
    let fixture = running_fixture("deploy:origins", "running");
    let history = seeded_history(&[600, 660, 720]);
    let mut poller = poller_over(&fixture.path(), &[])
        .with_eta_history(history.path())
        .with_clock(|| created() + Duration::seconds(390));
    let tick = poller.tick().await;
    let row = main_row(&tick.snapshot);
    assert_eq!(row.deploy, "in_progress");
    assert_eq!(
        row.eta,
        Some(Eta {
            typical_secs: 660,
            elapsed_secs: 390
        })
    );
    let json = serde_json::to_value(row).expect("serialises");
    assert_eq!(
        json["eta"],
        serde_json::json!({ "typical_secs": 660, "elapsed_secs": 390 })
    );
}

#[tokio::test]
async fn a_live_row_whose_marker_has_not_appeared_yet_shows_the_estimate_too() {
    let fixture = running_fixture("build", "running");
    let history = seeded_history(&[600, 660, 720]);
    let mut poller = poller_over(&fixture.path(), &[])
        .with_eta_history(history.path())
        .with_clock(|| created() + Duration::seconds(30));
    let tick = poller.tick().await;
    let row = main_row(&tick.snapshot);
    assert_eq!(row.deploy, "absent");
    assert!(row.live);
    assert_eq!(
        row.eta,
        Some(Eta {
            typical_secs: 660,
            elapsed_secs: 30
        })
    );
}

#[tokio::test]
async fn a_live_row_on_a_watch_without_markers_has_nothing_to_estimate() {
    let fixture = running_fixture("build", "running");
    let history = seeded_history(&[600, 660, 720]);
    let mut poller = poller_over(
        &fixture.path(),
        &[set_main("deploy_markers", EditValue::Array(Vec::new()))],
    )
    .with_eta_history(history.path())
    .with_clock(|| created() + Duration::seconds(30));
    let tick = poller.tick().await;
    assert_eq!(main_row(&tick.snapshot).eta, None);
}

#[tokio::test]
async fn a_deploy_past_twice_its_usual_time_shows_no_estimate() {
    let fixture = running_fixture("deploy:origins", "running");
    let history = seeded_history(&[600, 660, 720]);
    let mut poller = poller_over(&fixture.path(), &[])
        .with_eta_history(history.path())
        .with_clock(|| created() + Duration::seconds(1321));
    let tick = poller.tick().await;
    assert_eq!(main_row(&tick.snapshot).eta, None);
}

#[tokio::test]
async fn two_samples_are_not_enough_to_show_anything() {
    let fixture = running_fixture("deploy:origins", "running");
    let history = seeded_history(&[600, 660]);
    let mut poller = poller_over(&fixture.path(), &[])
        .with_eta_history(history.path())
        .with_clock(|| created() + Duration::seconds(30));
    let tick = poller.tick().await;
    assert_eq!(main_row(&tick.snapshot).eta, None);
}

#[tokio::test]
async fn a_settled_row_carries_no_estimate_and_serialises_without_the_key() {
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let history = seeded_history(&[600, 660, 720]);
    let mut poller = poller_over(&dir, &[]).with_eta_history(history.path());
    let tick = poller.tick().await;
    let row = main_row(&tick.snapshot);
    assert_eq!(row.deploy, "live");
    assert_eq!(row.eta, None);
    let json = serde_json::to_value(row).expect("serialises");
    assert!(
        !json.as_object().expect("an object").contains_key("eta"),
        "a settled row's JSON is what it was before the estimate existed"
    );
}

#[tokio::test]
async fn a_new_sample_is_written_to_the_file_and_a_tick_without_one_writes_nothing() {
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let file = TempPath::new("bw-eta-persist");
    let mut poller = poller_over(&dir, &[]).with_eta_history(file.path());
    poller.tick().await;
    assert_eq!(EtaHistory::load(&file.path()).samples("main-push"), [799]);

    // Nothing new to learn: the file is left alone, so deleting it proves a
    // second tick did not write.
    std::fs::remove_file(file.path()).expect("remove");
    poller.tick().await;
    assert!(!file.path().exists());

    let fixture = running_fixture("deploy:origins", "running");
    let untouched = TempPath::new("bw-eta-untouched");
    let mut poller = poller_over(&fixture.path(), &[]).with_eta_history(untouched.path());
    poller.tick().await;
    assert!(
        !untouched.path().exists(),
        "a tick that learned nothing creates no file"
    );
}
