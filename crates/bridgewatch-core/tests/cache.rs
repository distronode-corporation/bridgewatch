//! The pipeline cache: what it is safe not to ask for again.

use bridgewatch_core::model::{Pipeline, PipelineDetail};
use bridgewatch_core::poll::PipelineCache;
use bridgewatch_core::status::Status;

fn row(id: u64, status: &str, updated: Option<&str>) -> Pipeline {
    Pipeline {
        id,
        iid: None,
        project_id: Some(1),
        sha: "0123456789abcdef".into(),
        ref_name: "main".into(),
        status: Status::from(status.to_string()),
        source: Some("push".into()),
        web_url: None,
        created_at: None,
        updated_at: updated.map(str::to_string),
        started_at: None,
        finished_at: None,
    }
}

#[test]
fn an_uncached_pipeline_always_needs_fetching() {
    let cache = PipelineCache::new();
    assert!(cache.needs_refresh(&row(1, "success", Some("t1"))));
    assert!(cache.get(1).is_none());
}

/// The claim the idle interval rests on: a settled pipeline is re-fetched only
/// when its list row's `updated_at` moves.
#[test]
fn a_settled_pipeline_is_not_refetched_until_updated_at_moves() {
    let mut cache = PipelineCache::new();
    let first = row(1, "success", Some("2026-09-17T06:59:00.643Z"));
    cache.insert(&first, PipelineDetail::bare(first.clone()));

    assert!(!cache.needs_refresh(&first), "nothing moved");
    assert!(
        !cache.needs_refresh(&row(1, "success", Some("2026-09-17T06:59:00.643Z"))),
        "an identical row from a later list is still the same revision"
    );
    assert!(
        cache.needs_refresh(&row(1, "success", Some("2026-09-17T07:10:00.000Z"))),
        "updated_at moved, so a job was retried and the detail is stale"
    );
    assert!(
        cache.needs_refresh(&row(1, "failed", Some("2026-09-17T06:59:00.643Z"))),
        "the status moved even though the timestamp did not"
    );
}

/// A live pipeline is refetched every tick whatever its timestamp says, because
/// `updated_at` does not move for every job transition.
#[test]
fn a_live_pipeline_is_always_refetched() {
    let mut cache = PipelineCache::new();
    let running = row(2, "running", Some("t1"));
    cache.insert(&running, PipelineDetail::bare(running.clone()));
    assert!(cache.needs_refresh(&running));

    for status in [
        "pending",
        "created",
        "waiting_for_resource",
        "preparing",
        "canceling",
    ] {
        let r = row(2, status, Some("t1"));
        cache.insert(&r, PipelineDetail::bare(r.clone()));
        assert!(cache.needs_refresh(&r), "{status} is live");
    }
}

/// A pipeline parked at a gate is settled, and must not hold the fast interval
/// open. `manual` on a job is a gate; a pipeline the API calls `manual` is too.
#[test]
fn a_gate_counts_as_settled() {
    let mut cache = PipelineCache::new();
    let parked = row(3, "manual", Some("t1"));
    cache.insert(&parked, PipelineDetail::bare(parked.clone()));
    assert!(
        !cache.needs_refresh(&parked),
        "a gate will still be a gate in twenty seconds"
    );
    assert!(!Status::Manual.is_live());
    assert!(Status::Manual.is_settled());
}

/// A row that drops off the list is dropped from the cache, so a busy project
/// cannot grow it without bound.
#[test]
fn retain_drops_pipelines_that_left_the_list() {
    let mut cache = PipelineCache::new();
    for id in 1..=5 {
        let r = row(id, "success", Some("t"));
        cache.insert(&r, PipelineDetail::bare(r.clone()));
    }
    assert_eq!(cache.len(), 5);

    cache.retain(&[4u64, 5].into_iter().collect());
    assert_eq!(cache.len(), 2);
    assert!(cache.get(1).is_none());
    assert!(cache.get(5).is_some());

    cache.clear();
    assert!(cache.is_empty());
}

/// A missing `updated_at` is a revision like any other, and must not make every
/// tick look like a change.
#[test]
fn a_null_updated_at_is_a_stable_revision() {
    let mut cache = PipelineCache::new();
    let r = row(6, "success", None);
    cache.insert(&r, PipelineDetail::bare(r.clone()));
    assert!(!cache.needs_refresh(&row(6, "success", None)));
    assert!(cache.needs_refresh(&row(6, "success", Some("t1"))));
}

/// A detail one of whose requests failed is re-fetched even though the pipeline
/// is settled and has not moved.
///
/// ⛔ This is the difference between a transient failure and a permanent one.
/// The saving the cache exists for — a settled pipeline is not asked for again —
/// becomes permanent silence when what is stored is missing a child: the
/// revision never moves again, so nothing is ever re-planned, and a bridge keeps
/// reporting the `success` a `trigger:*` job has from the moment its child is
/// created. One dropped connection would hide a red child pipeline until the
/// process restarted.
#[test]
fn an_incomplete_detail_is_always_refetched() {
    let mut cache = PipelineCache::new();
    let settled = row(1, "success", Some("t1"));

    cache.insert_partial(&settled, PipelineDetail::bare(settled.clone()), true);
    assert!(
        cache.needs_refresh(&settled),
        "a child request failed, so what is cached is not the whole picture"
    );
    assert!(cache.get(1).is_some(), "but it is still worth showing");
    assert!(cache.get(1).unwrap().incomplete);

    // The successful re-fetch clears it, and the saving resumes.
    cache.insert(&settled, PipelineDetail::bare(settled.clone()));
    assert!(!cache.needs_refresh(&settled));
    assert!(!cache.get(1).unwrap().incomplete);
}
