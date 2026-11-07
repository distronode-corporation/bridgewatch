//! The noise controls: one "failing" per pipeline, "fixed again", and folding
//! a burst into one notification.

mod support;

use bridgewatch_core::config::Watch;
use bridgewatch_core::notify::{
    BATCH_OVER, Notification, NotifyKind, NotifyLedger, batch, notifications_for,
};
use bridgewatch_core::verdict::{PipelineView, WatchView};

fn watch() -> Watch {
    support::config_with(&[]).watches.remove(0)
}

/// One row, built from JSON so a test says only what it means.
fn row(id: u64, state: &str, deploy: &str, live: bool, failures: &[&str]) -> PipelineView {
    let failures = serde_json::to_string(failures).unwrap();
    serde_json::from_str(&format!(
        r#"{{"id":{id},"iid":1,"sha":"4cfaced9d33af808","sha7":"4cfaced",
            "ref":"main","source":"push","status":"success",
            "web_url":"https://gitlab.example/p/-/pipelines/{id}",
            "state":"{state}","deploy":"{deploy}",
            "deploy_marker":{{"name":"deploy:origins","id":1,"pipeline_id":2,"web_url":null,
                              "started_at":null}},
            "deploy_failures":[],"failures":{failures},"warnings":[],"gates":[],
            "post_deploy_failures":[],"sibling_failures":[],"bridges":[],"parent_jobs":[],
            "updated_at":null,"created_at":null,"live":{live}}}"#
    ))
    .expect("view decodes")
}

fn view(watch: &Watch, rows: Vec<PipelineView>) -> WatchView {
    WatchView {
        id: watch.id.clone(),
        role: watch.role,
        icon_state: None,
        rows,
        error: None,
        jobs: Default::default(),
        provider: Default::default(),
        actions: false,
    }
}

fn baselined(watch: &Watch) -> NotifyLedger {
    let mut ledger = NotifyLedger::default();
    ledger.baseline(&watch.id);
    ledger
}

fn kinds(out: &[Notification]) -> Vec<&'static str> {
    out.iter().map(|n| n.kind.as_str()).collect()
}

// ---------------------------------------------------------------------------
// "Still failing" is said once per pipeline
// ---------------------------------------------------------------------------

/// ⛔ A fan-out of jobs going red one after another in ONE pipeline used to
/// announce that pipeline again for each new first-three set of names.
#[test]
fn a_pipeline_reported_failing_is_not_reported_again_as_more_jobs_fail() {
    let w = watch();
    let mut ledger = baselined(&w);

    let first = notifications_for(
        &w,
        &view(&w, vec![row(10, "failed", "absent", true, &["shard-1"])]),
        &mut ledger,
    );
    assert_eq!(kinds(&first), ["blocking_failure"]);

    let again = notifications_for(
        &w,
        &view(
            &w,
            vec![row(10, "failed", "absent", true, &["shard-1", "shard-2"])],
        ),
        &mut ledger,
    );
    assert!(again.is_empty(), "still failing, same pipeline: {again:?}");
}

/// ⛔ And when that pipeline settles red, "finished: failed" is the same news
/// a second time.
#[test]
fn a_pipeline_reported_failing_is_not_announced_again_when_it_settles() {
    let w = watch();
    let mut ledger = baselined(&w);

    let first = notifications_for(
        &w,
        &view(&w, vec![row(11, "failed", "absent", true, &["lint"])]),
        &mut ledger,
    );
    assert_eq!(kinds(&first), ["blocking_failure"]);

    let settled = notifications_for(
        &w,
        &view(&w, vec![row(11, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );
    assert!(settled.is_empty(), "{:?}", kinds(&settled));
}

/// A ledger from before the key changed holds `<pid>|blocking_failure|<names>`.
/// Upgrading must not read that as "never said".
#[test]
fn a_ledger_from_before_the_failure_key_changed_is_not_announced_again() {
    let w = watch();
    let mut ledger = baselined(&w);
    ledger.record("12|blocking_failure|lint".into());

    let out = notifications_for(
        &w,
        &view(
            &w,
            vec![row(12, "failed", "absent", true, &["lint", "test"])],
        ),
        &mut ledger,
    );
    assert!(out.is_empty(), "{out:?}");

    // The prefix is the whole id: pipeline 1 is not pipeline 12.
    let other = notifications_for(
        &w,
        &view(&w, vec![row(1, "failed", "absent", true, &["lint"])]),
        &mut ledger,
    );
    assert_eq!(kinds(&other), ["blocking_failure"]);
}

/// A NEW pipeline failing is news, however red the watch already is.
#[test]
fn a_new_pipeline_failing_is_still_news() {
    let w = watch();
    let mut ledger = baselined(&w);
    notifications_for(
        &w,
        &view(&w, vec![row(20, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );
    let out = notifications_for(
        &w,
        &view(
            &w,
            vec![
                row(21, "failed", "absent", true, &["lint"]),
                row(20, "failed", "absent", false, &["lint"]),
            ],
        ),
        &mut ledger,
    );
    assert_eq!(kinds(&out), ["blocking_failure"]);
    assert_eq!(out[0].pipeline_id, 21);
}

// ---------------------------------------------------------------------------
// "Fixed again"
// ---------------------------------------------------------------------------

/// A failing watch whose next settled pipeline passes says "fixed again",
/// ONCE, in place of the deploy and finish banners for that push.
#[test]
fn a_failing_watch_that_passes_says_fixed_again_once() {
    let w = watch();
    let mut ledger = baselined(&w);
    notifications_for(
        &w,
        &view(&w, vec![row(30, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );
    assert!(ledger.is_failing(&w.id));

    let rows = vec![
        row(31, "deployed", "live", false, &[]),
        row(30, "failed", "absent", false, &["lint"]),
    ];
    let out = notifications_for(&w, &view(&w, rows.clone()), &mut ledger);
    assert_eq!(kinds(&out), ["recovered"], "{out:#?}");
    assert_eq!(out[0].pipeline_id, 31);
    assert_eq!(out[0].title, "main-push 4cfaced: deployed, fixed again");
    assert!(!ledger.is_failing(&w.id));

    let again = notifications_for(&w, &view(&w, rows), &mut ledger);
    assert!(
        again.is_empty(),
        "the stood-down deploy and finish do not arrive a tick late: {again:?}"
    );
}

/// ⛔ A deploy reached while post-deploy jobs still run used to be announced
/// at once, so "fixed again" arrived as a SECOND banner when the row settled
/// (Seer, #58). On a failing watch the deploy waits for the settle.
#[test]
fn a_deploy_on_a_failing_watch_waits_for_the_settle_and_is_replaced() {
    let w = watch();
    let mut ledger = baselined(&w);
    notifications_for(
        &w,
        &view(&w, vec![row(40, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );

    let red = row(40, "failed", "absent", false, &["lint"]);
    let live = notifications_for(
        &w,
        &view(
            &w,
            vec![row(41, "deployed", "live", true, &[]), red.clone()],
        ),
        &mut ledger,
    );
    assert!(live.is_empty(), "held while live: {:?}", kinds(&live));

    let settled = vec![row(41, "deployed", "live", false, &[]), red.clone()];
    let out = notifications_for(&w, &view(&w, settled.clone()), &mut ledger);
    assert_eq!(kinds(&out), ["recovered"], "one banner for the push");
    assert!(notifications_for(&w, &view(&w, settled), &mut ledger).is_empty());

    // Settling red instead, the held deploy is still said, beside the failure.
    let mut ledger = baselined(&w);
    notifications_for(&w, &view(&w, vec![red.clone()]), &mut ledger);
    let live = notifications_for(
        &w,
        &view(
            &w,
            vec![row(42, "deployed", "live", true, &[]), red.clone()],
        ),
        &mut ledger,
    );
    assert!(live.is_empty(), "{:?}", kinds(&live));
    let out = notifications_for(
        &w,
        &view(
            &w,
            vec![
                row(42, "deployed_with_failure", "live", false, &["fuzz"]),
                red,
            ],
        ),
        &mut ledger,
    );
    assert_eq!(kinds(&out), ["deployed", "blocking_failure"], "{out:#?}");
}

/// A green watch's deploy is not held: it is said while post-deploy jobs run.
#[test]
fn a_deploy_on_a_green_watch_is_said_at_once() {
    let w = watch();
    let mut ledger = baselined(&w);
    let out = notifications_for(
        &w,
        &view(&w, vec![row(50, "deployed", "live", true, &[])]),
        &mut ledger,
    );
    assert_eq!(kinds(&out), ["deployed"]);
}

/// Turned off, a recovering push says what any green push says.
#[test]
fn recovered_off_says_the_ordinary_thing() {
    let mut w = watch();
    w.notify.recovered = false;
    let mut ledger = baselined(&w);
    notifications_for(
        &w,
        &view(&w, vec![row(40, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );
    let out = notifications_for(
        &w,
        &view(
            &w,
            vec![
                row(41, "deployed", "live", false, &[]),
                row(40, "failed", "absent", false, &["lint"]),
            ],
        ),
        &mut ledger,
    );
    assert_eq!(kinds(&out), ["deployed"]);
}

/// The baseline learns that a watch is failing without saying so, and the
/// first green push after a launch still says "fixed again".
#[test]
fn the_baseline_learns_a_failing_watch_silently() {
    let w = watch();
    let mut ledger = NotifyLedger::default();
    let first = notifications_for(
        &w,
        &view(&w, vec![row(50, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );
    assert!(first.is_empty());
    assert!(ledger.is_failing(&w.id));

    let out = notifications_for(
        &w,
        &view(
            &w,
            vec![
                row(51, "succeeded_no_deploy", "absent", false, &[]),
                row(50, "failed", "absent", false, &["lint"]),
            ],
        ),
        &mut ledger,
    );
    assert_eq!(kinds(&out), ["recovered"]);
    assert_eq!(
        out[0].title,
        "main-push 4cfaced: succeeded_no_deploy, fixed again"
    );
}

/// A cancelled or still-running push neither breaks nor fixes the watch.
#[test]
fn a_cancelled_or_running_push_neither_breaks_nor_fixes() {
    let w = watch();
    let mut ledger = baselined(&w);
    notifications_for(
        &w,
        &view(&w, vec![row(60, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );
    let out = notifications_for(
        &w,
        &view(
            &w,
            vec![
                row(62, "running", "in_progress", true, &[]),
                row(61, "canceled", "canceled", false, &[]),
                row(60, "failed", "absent", false, &["lint"]),
            ],
        ),
        &mut ledger,
    );
    assert!(!kinds(&out).contains(&"recovered"), "{:?}", kinds(&out));
    assert!(ledger.is_failing(&w.id));
}

/// A green watch that stays green never says "fixed again".
#[test]
fn a_green_watch_is_never_fixed_again() {
    let w = watch();
    let mut ledger = baselined(&w);
    notifications_for(
        &w,
        &view(&w, vec![row(70, "deployed", "live", false, &[])]),
        &mut ledger,
    );
    let out = notifications_for(
        &w,
        &view(
            &w,
            vec![
                row(71, "deployed", "live", false, &[]),
                row(70, "deployed", "live", false, &[]),
            ],
        ),
        &mut ledger,
    );
    assert_eq!(kinds(&out), ["deployed"]);
}

/// The failing set survives a restart, so a fix after one still says so.
#[test]
fn the_failing_set_round_trips_through_the_ledger_file() {
    let path = std::env::temp_dir().join(format!(
        "bw-noise-failing-{}-{}.json",
        std::process::id(),
        line!()
    ));
    let w = watch();
    let mut ledger = baselined(&w);
    notifications_for(
        &w,
        &view(&w, vec![row(80, "failed", "absent", false, &["lint"])]),
        &mut ledger,
    );
    ledger.save(&path).unwrap();
    let back = NotifyLedger::load(&path);
    let _ = std::fs::remove_file(&path);
    assert!(back.is_failing(&w.id));

    // And a ledger written before the field existed reads as nothing failing.
    let old: NotifyLedger =
        serde_json::from_str(r#"{"seen":[],"baselined":["main-push"]}"#).unwrap();
    assert!(!old.is_failing("main-push"));
}

// ---------------------------------------------------------------------------
// A burst is folded into one
// ---------------------------------------------------------------------------

fn note(watch: &str, n: u64, url: Option<&str>) -> Notification {
    Notification {
        watch: watch.into(),
        kind: NotifyKind::Deployed,
        pipeline_id: n,
        title: format!("{watch} #{n}: deployed"),
        body: "all green".into(),
        url: url.map(str::to_string),
        key: format!("{n}|deployed|"),
    }
}

#[test]
fn up_to_three_notifications_are_delivered_as_they_are() {
    assert_eq!(BATCH_OVER, 3);
    let three: Vec<_> = (1..=3).map(|n| note("web", n, None)).collect();
    assert_eq!(batch(three.clone()), three);
    assert!(batch(Vec::new()).is_empty());
}

#[test]
fn more_than_three_in_one_poll_are_folded_into_one() {
    let five: Vec<_> = (1..=5)
        .map(|n| {
            note(
                if n % 2 == 0 { "api" } else { "web" },
                n,
                Some("https://x/1"),
            )
        })
        .collect();
    let out = batch(five);
    assert_eq!(out.len(), 1, "{out:#?}");
    let n = &out[0];
    assert_eq!(n.kind.as_str(), "batch");
    assert_eq!(n.title, "bridgewatch: 5 updates");
    assert_eq!(
        n.body,
        "web #1: deployed\napi #2: deployed\nweb #3: deployed\nand 2 more"
    );
    assert_eq!(
        n.url.as_deref(),
        Some("https://x/1"),
        "every one pointed there"
    );
    assert_eq!(n.watch, "", "more than one watch");
}

#[test]
fn a_folded_burst_links_nowhere_when_its_parts_disagree() {
    let four = vec![
        note("web", 1, Some("https://x/1")),
        note("web", 2, Some("https://x/2")),
        note("web", 3, None),
        note("web", 4, Some("https://x/1")),
    ];
    let out = batch(four);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].url, None);
    assert_eq!(out[0].watch, "web");
}
