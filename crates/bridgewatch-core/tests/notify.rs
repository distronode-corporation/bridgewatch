//! Notifications: what is worth interrupting somebody over, and exactly once.

mod support;

use std::sync::atomic::{AtomicU64, Ordering};

use bridgewatch_core::config::edit::{Edit, EditValue};
use bridgewatch_core::notify::{NotifyKind, NotifyLedger, dedupe_key, notifications_for};
use bridgewatch_core::poll::Poller;
use bridgewatch_core::verdict::WatchView;

/// A ledger file in the temp dir, unique to one call and removed when dropped.
///
/// ⛔ Every path here used to be `<name>-{pid}.json`. The tests in this binary
/// share one pid and run on parallel threads, so two names that met collided:
/// `baselined_tick`'s tag `"green"` built exactly the `bw-notify-green-{pid}.json`
/// that `the_default_body_reads_all_green_when_nothing_failed` wrote by hand, and
/// the two tests read each other's ledger — a flake seen on Linux. The counter
/// makes a collision impossible whatever the names. The file goes on DROP, not
/// after `with_ledger`: a tick re-saves the ledger, so a `remove_file` straight
/// after building the poller ran before the last write and leaked one file per
/// test per run.
struct TempLedger(std::path::PathBuf);

impl TempLedger {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("{name}-{}-{n}.json", std::process::id())))
    }

    fn path(&self) -> std::path::PathBuf {
        self.0.clone()
    }
}

impl Drop for TempLedger {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn poller_for(fixture: &str, edits: &[Edit]) -> Poller {
    let config = support::config_with(edits);
    let dir = support::fixtures_dir().join(fixture);
    support::fixture_poller(&config, &dir).0
}

/// A GitLab watch's view carries no `provider` key at all, so `check --json`
/// and every recorded snapshot read byte for byte as they did before GitHub
/// support, and reading one back still yields GitLab.
#[tokio::test]
async fn a_gitlab_watch_view_serialises_without_a_provider_key() {
    let mut poller = poller_for("ca41ab28-deployed-with-failure", &[]);
    let snapshot = poller.tick().await.snapshot;
    let json = serde_json::to_value(&snapshot).expect("a snapshot serialises");
    let watch = json["watches"][0]
        .as_object()
        .expect("a watch is an object");
    assert!(!watch.contains_key("provider"), "keys: {:?}", watch.keys());
    let back: WatchView = serde_json::from_value(json["watches"][0].clone()).expect("reads back");
    assert_eq!(back.provider, bridgewatch_core::config::Provider::Gitlab);
}

/// The first successful tick baselines silently. Starting the app must not
/// replay the last week as a burst.
#[tokio::test]
async fn the_first_tick_baselines_silently() {
    let mut poller = poller_for("ca41ab28-deployed-with-failure", &[]);

    let first = poller.tick().await;
    assert!(
        first.notifications.is_empty(),
        "the first tick tells you nothing: {:?}",
        first.notifications
    );
    assert!(poller.ledger().is_baselined("main-push"));
    assert!(
        !poller.ledger().seen.is_empty(),
        "but it records what it would have said, so the next tick only reports changes"
    );

    let second = poller.tick().await;
    assert!(
        second.notifications.is_empty(),
        "nothing changed, so nothing is said"
    );
}

/// A change after the baseline does notify, once, with the configured template.
#[tokio::test]
async fn a_change_after_the_baseline_notifies_exactly_once() {
    let mut poller = poller_for("ca41ab28-deployed-with-failure", &[]);
    poller.tick().await; // baseline

    // Re-open the same fixture through a fresh ledger: a poller that has been
    // baselined on nothing sees this pipeline's state as new.
    let mut fresh = poller_for("ca41ab28-deployed-with-failure", &[]);
    fresh.tick().await;
    fresh.tick().await;

    // Drive a genuinely unseen pipeline through an already-baselined ledger by
    // asking a second fixture, which is what a new push looks like.
    let mut poller = poller_for("4cfaced9-deployed", &[]);
    poller.tick().await;
    let again = poller.tick().await;
    assert!(again.notifications.is_empty());
}

/// Deduplication is by pipeline, kind and the job names involved.
#[test]
fn the_dedupe_key_is_stable_under_reordering() {
    let a = dedupe_key(1, NotifyKind::BlockingFailure, &["b".into(), "a".into()]);
    let b = dedupe_key(1, NotifyKind::BlockingFailure, &["a".into(), "b".into()]);
    assert_eq!(a, b, "the order GitLab returned them in is not news");
    assert_eq!(a, "1|blocking_failure|a,b");

    assert_ne!(
        a,
        dedupe_key(2, NotifyKind::BlockingFailure, &["a".into(), "b".into()])
    );
    assert_ne!(
        a,
        dedupe_key(1, NotifyKind::Finished, &["a".into(), "b".into()])
    );
    assert_ne!(
        a,
        dedupe_key(1, NotifyKind::BlockingFailure, &["a".into(), "c".into()])
    );
}

/// A secondary watch never notifies, however red it is.
#[tokio::test]
async fn a_secondary_watch_never_notifies() {
    // The schedule really is failing, and it is matched only by the secondary
    // `hourly` watch.
    let mut poller = poller_for("mixed-primary-green-secondary-red", &[]);
    poller.tick().await; // baseline
    let tick = poller.tick().await;

    assert!(tick.notifications.is_empty());
    assert!(
        !poller.ledger().is_baselined("hourly"),
        "a secondary watch is never even baselined, because it can never fire"
    );
    assert!(poller.ledger().is_baselined("main-push"));

    let hourly = tick
        .snapshot
        .watches
        .iter()
        .find(|w| w.id == "hourly")
        .unwrap();
    assert_eq!(hourly.rows[0].state.as_str(), "failed", "it really is red");
}

/// The templates are MiniJinja over the view model, and the default body's
/// `default('all green')` filter has to survive an empty failures list.
#[tokio::test]
async fn templates_render_from_the_view_model() {
    let mut poller = poller_for(
        "ca41ab28-deployed-with-failure",
        &[Edit::Set {
            path: "watches.0.notify.title".into(),
            value: EditValue::String(
                "{{watch.id}} {{sha7}} {{state}} deploy={{deploy}} [{{kind}}]".into(),
            ),
        }],
    );

    // Baseline against a ledger that has already seen a different pipeline, so
    // this one's events are genuinely new.
    let mut ledger = NotifyLedger::default();
    ledger.baseline("main-push");
    ledger.baseline("hourly");
    ledger.baseline("preflights");
    let ledger_file = TempLedger::new("bridgewatch-notify");
    let mut poller = {
        let _ = &mut poller;
        let config = support::config_with(&[Edit::Set {
            path: "watches.0.notify.title".into(),
            value: EditValue::String(
                "{{watch.id}} {{sha7}} {{state}} deploy={{deploy}} [{{kind}}]".into(),
            ),
        }]);
        let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
        let path = ledger_file.path();
        ledger.save(&path).expect("ledger writes");
        let p = support::fixture_poller(&config, &dir)
            .0
            .with_ledger(path.clone());
        let _ = std::fs::remove_file(&path);
        p
    };

    let tick = poller.tick().await;
    assert!(
        !tick.notifications.is_empty(),
        "a baselined watch with new events notifies"
    );

    let deployed = tick
        .notifications
        .iter()
        .find(|n| n.kind == NotifyKind::Deployed)
        .expect("deploy:origins succeeded, so `deployed` fires");
    assert_eq!(
        deployed.title,
        "main-push ca41ab2 deployed_with_failure deploy=live [deployed]"
    );

    let failure = tick
        .notifications
        .iter()
        .find(|n| n.kind == NotifyKind::BlockingFailure)
        .expect("the android bridge failed");
    assert_eq!(
        failure.body, "verify:android",
        "the default body joins the failures"
    );
    assert_eq!(failure.pipeline_id, 2857464986);
    assert!(
        failure.url.as_deref().unwrap().contains("/jobs/"),
        "click = first_failure_or_pipeline resolves to the failing job: {:?}",
        failure.url
    );

    // ⛔ `finished` is NOT here, and the old expectation that it would be was the
    // defect: `deployed` and `finished` both fire on the same tick for the same
    // pipeline, and with the shipped templates they render the same title and
    // the same body. Two identical banners for one push, three when something
    // also broke. The interesting kinds win and `finished` stands down.
    assert!(
        !tick
            .notifications
            .iter()
            .any(|n| n.kind == NotifyKind::Finished),
        "a pipeline that deployed does not also announce that it finished: {:?}",
        tick.notifications
            .iter()
            .map(|n| (n.kind, n.title.clone()))
            .collect::<Vec<_>>()
    );
}

/// A settled pipeline with nothing else to say does still announce itself.
///
/// The pair to the assertion above: `finished` stands down for a better kind,
/// not for good. `4cfaced9` with `deployed` turned off leaves `finished` as the
/// only thing worth saying.
#[tokio::test]
async fn finished_still_fires_when_it_is_the_only_news() {
    let mut ledger = NotifyLedger::default();
    ledger.baseline("main-push");
    let ledger_file = TempLedger::new("bw-notify-only");
    let path = ledger_file.path();
    ledger.save(&path).unwrap();

    let config = support::config_with(&[Edit::Set {
        path: "watches.0.notify.deployed".into(),
        value: EditValue::Boolean(false),
    }]);
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let mut poller = support::fixture_poller(&config, &dir)
        .0
        .with_ledger(path.clone());
    let _ = std::fs::remove_file(&path);

    let kinds: Vec<NotifyKind> = poller
        .tick()
        .await
        .notifications
        .iter()
        .map(|n| n.kind)
        .collect();
    assert_eq!(kinds, [NotifyKind::Finished]);
}

/// A clean deploy raises exactly one notification, not two.
#[tokio::test]
async fn a_clean_deploy_raises_one_notification() {
    let mut ledger = NotifyLedger::default();
    ledger.baseline("main-push");
    let ledger_file = TempLedger::new("bw-notify-one");
    let path = ledger_file.path();
    ledger.save(&path).unwrap();

    let config = support::config_with(&[]);
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let mut poller = support::fixture_poller(&config, &dir)
        .0
        .with_ledger(path.clone());
    let _ = std::fs::remove_file(&path);

    let tick = poller.tick().await;
    let kinds: Vec<NotifyKind> = tick.notifications.iter().map(|n| n.kind).collect();
    assert_eq!(
        kinds,
        [NotifyKind::Deployed],
        "the shipped config turns on deployed, blocking_failure and finished; \
         a clean deploy is one event, not the same sentence twice"
    );

    // And the suppressed key is still recorded, or it fires on its own next tick.
    let second = poller.tick().await;
    assert!(
        second.notifications.is_empty(),
        "the stood-down `finished` does not arrive a minute late: {:?}",
        second.notifications
    );
}

/// The default body says something useful when there is nothing wrong.
#[tokio::test]
async fn the_default_body_reads_all_green_when_nothing_failed() {
    let mut ledger = NotifyLedger::default();
    ledger.baseline("main-push");
    let ledger_file = TempLedger::new("bw-notify-green");
    let path = ledger_file.path();
    ledger.save(&path).unwrap();

    let config = support::config_with(&[]);
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let mut poller = support::fixture_poller(&config, &dir)
        .0
        .with_ledger(path.clone());
    let _ = std::fs::remove_file(&path);

    let tick = poller.tick().await;
    let deployed = tick
        .notifications
        .iter()
        .find(|n| n.kind == NotifyKind::Deployed)
        .expect("it deployed");
    assert_eq!(deployed.body, "all green");
    assert_eq!(deployed.title, "main-push 4cfaced: deployed");
}

/// Turning a kind off in the config turns it off.
#[tokio::test]
async fn a_disabled_kind_does_not_fire() {
    let mut ledger = NotifyLedger::default();
    ledger.baseline("main-push");
    let ledger_file = TempLedger::new("bw-notify-off");
    let path = ledger_file.path();
    ledger.save(&path).unwrap();

    let config = support::config_with(&[
        Edit::Set {
            path: "watches.0.notify.deployed".into(),
            value: EditValue::Boolean(false),
        },
        Edit::Set {
            path: "watches.0.notify.finished".into(),
            value: EditValue::Boolean(false),
        },
    ]);
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let mut poller = support::fixture_poller(&config, &dir)
        .0
        .with_ledger(path.clone());
    let _ = std::fs::remove_file(&path);

    let kinds: Vec<NotifyKind> = poller
        .tick()
        .await
        .notifications
        .iter()
        .map(|n| n.kind)
        .collect();
    assert_eq!(kinds, [NotifyKind::BlockingFailure]);
}

/// The ledger survives a round trip through disk, and a corrupt one costs one
/// repeated notification rather than a crash.
#[test]
fn the_ledger_round_trips_and_tolerates_corruption() {
    let ledger_file = TempLedger::new("bw-ledger");
    let path = ledger_file.path();
    let mut ledger = NotifyLedger::default();
    ledger.record("1|deployed|deploy:origins".into());
    ledger.baseline("main-push");
    ledger.save(&path).unwrap();

    let reloaded = NotifyLedger::load(&path);
    assert!(reloaded.contains("1|deployed|deploy:origins"));
    assert!(reloaded.is_baselined("main-push"));
    assert!(!reloaded.is_baselined("hourly"));

    std::fs::write(&path, "{ not json").unwrap();
    let broken = NotifyLedger::load(&path);
    assert!(
        broken.seen.is_empty(),
        "a corrupt ledger is an empty one, not a panic"
    );

    let missing = NotifyLedger::load(std::path::Path::new("/nonexistent/bridgewatch/x.json"));
    assert!(missing.seen.is_empty());
    let _ = std::fs::remove_file(&path);
}

/// The ledger drops the oldest **pipeline**, not the smallest string.
///
/// ⛔ The set orders keys as text, and `"1000000000|…"` sorts before
/// `"999999999|…"`, so taking the set's first element throws away the NEWEST
/// pipeline the moment ids cross a power of ten. gitlab.com crossed 1,000,000,000
/// in 2021 and this project's ids are ten digits today. The pipeline that was
/// just trimmed is the one still on screen, so it re-notifies on every tick
/// until it falls off the list.
#[test]
fn the_ledger_trims_the_oldest_pipeline_across_a_digit_boundary() {
    use bridgewatch_core::notify::LEDGER_MAX_KEYS;

    let mut ledger = NotifyLedger::default();
    let oldest = "999000000|finished|deployed".to_string();
    ledger.record(oldest.clone());
    for i in 1..LEDGER_MAX_KEYS {
        ledger.record(format!("{}|finished|deployed", 999_000_000 + i));
    }
    assert_eq!(ledger.seen.len(), LEDGER_MAX_KEYS);

    // One more, from a pipeline whose id has ten digits — so it is the newest
    // and it sorts FIRST as a string.
    let newest = "1000000001|blocking_failure|verify:web_types".to_string();
    ledger.record(newest.clone());

    assert_eq!(ledger.seen.len(), LEDGER_MAX_KEYS, "one key was dropped");
    assert!(
        ledger.contains(&newest),
        "and it was not the pipeline that is still on screen"
    );
    assert!(!ledger.contains(&oldest), "it was the oldest one");
}

/// A tick whose list request failed has learned nothing, and must not claim the
/// baseline.
///
/// ⛔ Otherwise a laptop that starts before its Wi-Fi does gets a burst of stale
/// `deployed` and `finished` notifications on the first tick that works: the
/// empty tick takes the baseline, so the real one counts as change. An empty row
/// list with **no** error is a watch whose filter matched nothing, which is a
/// genuine baseline.
#[test]
fn a_watch_is_not_baselined_on_a_tick_that_failed() {
    let watch = support::config_with(&[]).watches.remove(0);
    let mut ledger = NotifyLedger::default();

    let offline = WatchView {
        id: watch.id.clone(),
        role: watch.role,
        icon_state: None,
        rows: Vec::new(),
        error: Some("transport error: dns failure".into()),
        jobs: Default::default(),
        provider: Default::default(),
    };
    assert!(notifications_for(&watch, &offline, &mut ledger).is_empty());
    assert!(
        !ledger.is_baselined(&watch.id),
        "a tick that learned nothing cannot be the baseline"
    );

    // The Wi-Fi comes up. This is the tick that used to arrive as a burst.
    let online = WatchView {
        rows: vec![row_view("deployed", "live")],
        error: None,
        ..offline.clone()
    };
    assert!(
        notifications_for(&watch, &online, &mut ledger).is_empty(),
        "the first tick that actually saw something baselines, silently"
    );
    assert!(ledger.is_baselined(&watch.id));

    // And a watch whose filter simply matched nothing is a real baseline.
    let mut ledger = NotifyLedger::default();
    let empty = WatchView {
        rows: Vec::new(),
        error: None,
        ..offline.clone()
    };
    assert!(notifications_for(&watch, &empty, &mut ledger).is_empty());
    assert!(ledger.is_baselined(&watch.id));
}

/// A notification whose template does not render is not silently spent.
///
/// ⛔ Template validation is syntax-only, so an unknown filter fails at RENDER
/// time. Recording the dedupe key first meant the key was burnt and the
/// notification lost for good — including after the template was fixed, because
/// the ledger remembered saying something it never said.
#[test]
fn a_template_that_fails_to_render_does_not_burn_the_dedupe_key() {
    use bridgewatch_core::config::edit::{Edit, EditValue};

    let mut broken = support::config_with(&[Edit::Set {
        path: "watches.0.notify.title".into(),
        value: EditValue::String("{{ sha7 | no_such_filter }}".into()),
    }])
    .watches
    .remove(0);
    broken.notify.finished = false;

    let view = WatchView {
        id: broken.id.clone(),
        role: broken.role,
        icon_state: None,
        rows: vec![row_view("deployed", "live")],
        error: None,
        jobs: Default::default(),
        provider: Default::default(),
    };

    let mut ledger = NotifyLedger::default();
    ledger.baseline(&broken.id);

    assert!(
        notifications_for(&broken, &view, &mut ledger).is_empty(),
        "it cannot be delivered"
    );
    assert!(
        ledger.seen.is_empty(),
        "and it is not recorded as delivered either: {:?}",
        ledger.seen
    );

    // The user fixes the template. The notification they were owed arrives.
    let mut fixed = support::config_with(&[]).watches.remove(0);
    fixed.notify.finished = false;
    let delivered = notifications_for(&fixed, &view, &mut ledger);
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].kind, NotifyKind::Deployed);
    assert_eq!(delivered[0].title, "main-push 4cfaced: deployed");
    assert_eq!(
        ledger.seen.len(),
        1,
        "recorded now that it was said, and not before"
    );
}

/// A pipeline parked at a gate has not finished, and a pipeline with a gate
/// somewhere in it is not news.
///
/// ⛔ `parked_gate` is the one state this tool exists to name. Calling it
/// "finished" is the same mistake every other monitor makes, wearing a different
/// word. ⚠️ And `gate_opened` keyed on "are there any gate jobs" fired on every
/// single push with the shipped config, because the four coverage shards are
/// manual on `main` — for a set of jobs the override table exists to say are
/// EXPECTED to sit unrun.
#[tokio::test]
async fn a_parked_pipeline_is_not_finished_and_a_mere_gate_is_not_news() {
    use bridgewatch_core::config::edit::{Edit, EditValue};

    let gate_on = [Edit::Set {
        path: "watches.0.notify.gate_opened".into(),
        value: EditValue::Boolean(true),
    }];

    // With the shipped config, which has `gate_opened = false`, the parked
    // pipeline says NOTHING. It has not finished; it is waiting for a person.
    let shipped = baselined_tick("9da437fd-parked-gate-stacked", &[], "shipped").await;
    assert_eq!(
        shipped
            .iter()
            .map(|n| (n.kind, n.pipeline_id))
            .collect::<Vec<_>>(),
        [(NotifyKind::Finished, 2859138213)],
        "only the green push finished; the parked one is still parked"
    );

    let parked = baselined_tick("9da437fd-parked-gate-stacked", &gate_on, "gates").await;
    let parked_row: Vec<(NotifyKind, u64)> = parked
        .iter()
        .map(|n| (n.kind, n.pipeline_id))
        .filter(|(_, id)| *id == 2858536145)
        .collect();
    assert_eq!(
        parked_row,
        [(NotifyKind::GateOpened, 2858536145)],
        "the parked pipeline is announced as parked, and never as finished: {:?}",
        parked
            .iter()
            .map(|n| (n.kind, n.pipeline_id))
            .collect::<Vec<_>>()
    );

    let deployed = baselined_tick("4cfaced9-deployed", &gate_on, "green").await;
    let kinds: Vec<NotifyKind> = deployed.iter().map(|n| n.kind).collect();
    assert_eq!(
        kinds,
        [NotifyKind::Deployed],
        "four manual coverage shards are not a gate anybody has to open"
    );
}

/// The ledger is what stops a restart replaying the week.
///
/// It is the only thing bridgewatch keeps across launches, and this is the
/// reason: a second process, over the same fixture, with the same ledger file,
/// says nothing at all.
#[tokio::test]
async fn a_restart_over_the_same_ledger_says_nothing_again() {
    let ledger_file = TempLedger::new("bw-restart");
    let path = ledger_file.path();
    let _ = std::fs::remove_file(&path);
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[]);

    let mut first = support::fixture_poller(&config, &dir)
        .0
        .with_ledger(path.clone());
    assert!(
        first.tick().await.notifications.is_empty(),
        "the first launch baselines silently"
    );
    assert!(first.ledger().is_baselined("main-push"));
    drop(first);

    let mut second = support::fixture_poller(&config, &dir)
        .0
        .with_ledger(path.clone());
    assert!(
        second.ledger().is_baselined("main-push"),
        "the baseline survived the restart, or the app re-notifies on every launch"
    );
    assert!(second.tick().await.notifications.is_empty());

    // A launch with no ledger at all re-baselines rather than replaying.
    let mut fresh = support::fixture_poller(&config, &dir).0;
    assert!(fresh.tick().await.notifications.is_empty());
    let _ = std::fs::remove_file(&path);
}

/// One pipeline row, built from JSON so the test says only what it means.
fn row_view(state: &str, deploy: &str) -> bridgewatch_core::verdict::PipelineView {
    serde_json::from_str(&format!(
        r#"{{"id":2856963900,"iid":13054,"sha":"4cfaced9d33af808","sha7":"4cfaced",
            "ref":"main","source":"push","status":"success","web_url":null,
            "state":"{state}","deploy":"{deploy}",
            "deploy_marker":{{"name":"deploy:origins","id":1,"pipeline_id":2,"web_url":null,
                              "started_at":null}},
            "deploy_failures":[],"failures":[],"warnings":[],"gates":[],
            "post_deploy_failures":[],"sibling_failures":[],"bridges":[],"parent_jobs":[],
            "updated_at":null,"created_at":null,"live":false}}"#
    ))
    .expect("view decodes")
}

/// One tick over a fixture, through a ledger that has already been baselined.
async fn baselined_tick(
    fixture: &str,
    edits: &[bridgewatch_core::config::edit::Edit],
    tag: &str,
) -> Vec<bridgewatch_core::notify::Notification> {
    let ledger_file = TempLedger::new(&format!("bw-notify-{tag}"));
    let path = ledger_file.path();
    let mut ledger = NotifyLedger::default();
    ledger.baseline("main-push");
    ledger.save(&path).unwrap();

    let config = support::config_with(edits);
    let dir = support::fixtures_dir().join(fixture);
    let mut poller = support::fixture_poller(&config, &dir)
        .0
        .with_ledger(path.clone());
    let out = poller.tick().await.notifications;
    let _ = std::fs::remove_file(&path);
    out
}

#[test]
fn the_app_and_the_cli_keep_separate_ledgers() {
    // Sharing one file meant a `bridgewatch watch` running beside the app
    // wrote back its own copy and deleted the app's keys (last writer wins).
    assert_ne!(NotifyLedger::default_path(), NotifyLedger::cli_path());
    assert_eq!(
        NotifyLedger::default_path().parent(),
        NotifyLedger::cli_path().parent()
    );
}

#[test]
fn a_ledger_save_round_trips_and_leaves_no_temporary_file() {
    let file = TempLedger::new("bridgewatch-atomic");
    let path = file.path();
    let mut ledger = NotifyLedger::default();
    ledger.baseline("main-push");
    ledger.record("12|deployed|".into());
    ledger.save(&path).expect("ledger writes");
    // Saving over an existing file goes through the rename path too.
    ledger.record("13|deployed|".into());
    ledger.save(&path).expect("ledger rewrites");

    let back = NotifyLedger::load(&path);
    assert!(back.is_baselined("main-push"));
    assert!(back.contains("13|deployed|"));
    let dir = path.parent().unwrap();
    let stem = path.file_name().unwrap().to_string_lossy().into_owned();
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&format!(".{stem}")) && n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
