//! What a notification that cannot be rendered says in the log, and how often.
//!
//! ⛔ Its own binary because it captures logs: see `tests/support/mod.rs` for
//! why every test in a capturing binary has to go through `with_log` first.

mod support;

use bridgewatch_core::config::edit::{Edit, EditValue};
use bridgewatch_core::notify::{NotifyLedger, notifications_for};
use bridgewatch_core::verdict::WatchView;
use support::{level_of, with_log};

fn view_with_row(watch: &bridgewatch_core::config::Watch, id: u64) -> WatchView {
    let row = serde_json::from_str(&format!(
        r#"{{"id":{id},"iid":1,"sha":"4cfaced9d33af808","sha7":"4cfaced",
            "ref":"main","source":"push","status":"success","web_url":null,
            "state":"deployed","deploy":"live",
            "deploy_marker":{{"name":"deploy:origins","id":1,"pipeline_id":2,"web_url":null,
                              "started_at":null}},
            "deploy_failures":[],"failures":[],"warnings":[],"gates":[],
            "post_deploy_failures":[],"sibling_failures":[],"bridges":[],"parent_jobs":[],
            "updated_at":null,"created_at":null,"live":false}}"#
    ))
    .expect("view decodes");
    WatchView {
        id: watch.id.clone(),
        role: watch.role,
        icon_state: None,
        rows: vec![row],
        error: None,
        jobs: Default::default(),
        provider: Default::default(),
        actions: false,
    }
}

/// A template that fails at render time is retried on every tick, by design
/// (the key is not burnt; see `notify.rs`). Warning every time made that one
/// broken filter an unbounded stream of identical lines, 720 an hour while
/// something was live, burying the first one, which is the one that matters.
/// It warns once per event per process, then says the same at `debug`.
#[test]
fn a_template_that_cannot_render_warns_once_per_event_then_debugs() {
    support::start_capture();
    let mut watch = support::config_with(&[Edit::Set {
        path: "watches.0.notify.title".into(),
        value: EditValue::String("{{ sha7 | no_such_filter }}".into()),
    }])
    .watches
    .remove(0);
    watch.notify.finished = false;
    let mut ledger = NotifyLedger::default();
    ledger.baseline(&watch.id);
    let view = view_with_row(&watch, 9_100_000_001);

    let (said, first) = with_log(|| notifications_for(&watch, &view, &mut ledger));
    assert!(said.is_empty());
    assert_eq!(
        level_of(&first, "notification template failed").as_deref(),
        Some("WARN"),
        "{first}"
    );

    for _ in 0..3 {
        let (said, again) = with_log(|| notifications_for(&watch, &view, &mut ledger));
        assert!(said.is_empty());
        assert_eq!(
            level_of(&again, "notification template failed").as_deref(),
            Some("DEBUG"),
            "the same event, again: {again}"
        );
    }

    // A different event is news to the log as well.
    let other = view_with_row(&watch, 9_100_000_002);
    let (_, other_log) = with_log(|| notifications_for(&watch, &other, &mut ledger));
    assert_eq!(
        level_of(&other_log, "notification template failed").as_deref(),
        Some("WARN"),
        "{other_log}"
    );
    assert!(ledger.seen.is_empty(), "and nothing was burnt");
}
