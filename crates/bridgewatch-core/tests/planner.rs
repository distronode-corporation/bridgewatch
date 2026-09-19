//! What a tick costs, in requests.
//!
//! The claims here are the ones that decide whether bridgewatch can be left
//! running all day on a shared rate limit, so they are counted rather than
//! assumed.

mod support;

use std::collections::HashSet;

use bridgewatch_core::config::WatchRules;
use bridgewatch_core::model::{Bridge, DownstreamPipeline, Pipeline};
use bridgewatch_core::poll::PipelineCache;
use bridgewatch_core::poll::planner;
use bridgewatch_core::status::Status;

fn requests_by_kind(paths: &[String]) -> (usize, usize, usize, usize) {
    let lists = paths.iter().filter(|p| p.ends_with_list()).count();
    let jobs = paths.iter().filter(|p| p.contains("/jobs?")).count();
    let bridges = paths.iter().filter(|p| p.contains("/bridges?")).count();
    (lists, jobs, bridges, paths.len())
}

trait EndsWithList {
    fn ends_with_list(&self) -> bool;
}
impl EndsWithList for String {
    fn ends_with_list(&self) -> bool {
        self.contains("/pipelines?")
    }
}

/// A first tick over a settled pipeline: one list per watch, two per pipeline
/// that needs refreshing, one per dived child.
#[tokio::test]
async fn a_cold_tick_costs_list_plus_two_plus_one_per_child() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[]);
    let (mut poller, transport) = support::fixture_poller(&config, &dir);

    poller.tick().await;
    let paths = transport.seen();
    let (lists, jobs, bridges, total) = requests_by_kind(&paths);

    assert_eq!(lists, 3, "one list per watch, and there are three watches");
    assert_eq!(bridges, 1, "only the one pipeline the push watch matched");
    assert_eq!(jobs, 5, "the parent plus its four children");
    assert_eq!(
        total, 9,
        "3 lists + 1 parent jobs + 1 bridges + 4 children: {paths:#?}"
    );
}

/// A second tick over the same settled pipeline costs nothing but the lists.
/// This is the claim the idle interval rests on.
#[tokio::test]
async fn an_idle_tick_costs_one_list_per_watch_and_nothing_else() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[]);
    let (mut poller, transport) = support::fixture_poller(&config, &dir);

    poller.tick().await;
    transport.clear_seen();
    let tick = poller.tick().await;

    let paths = transport.seen();
    assert_eq!(paths.len(), 3, "three lists and nothing else: {paths:#?}");
    assert!(paths.iter().all(|p| p.contains("/pipelines?")));
    assert_eq!(
        tick.snapshot.icon_state.as_str(),
        "deployed_with_failure",
        "and the verdict is unchanged, served from cache"
    );
}

/// A still-running pipeline is refetched every tick, but only the bridges that
/// are live or that moved cost a child request.
#[tokio::test]
async fn a_busy_tick_refetches_the_live_pipeline_and_only_the_live_child() {
    let dir = support::fixtures_dir().join("9da437fd-parked-gate-stacked");
    let config = support::config_with(&[]);
    let (mut poller, transport) = support::fixture_poller(&config, &dir);

    poller.tick().await;
    transport.clear_seen();
    poller.tick().await;

    let paths = transport.seen();
    let (lists, jobs, bridges, _) = requests_by_kind(&paths);
    assert_eq!(lists, 3);
    assert_eq!(
        bridges, 1,
        "only the running pipeline, not the settled one above it"
    );
    assert_eq!(
        jobs, 2,
        "the running parent, plus the one bridge that is still pending; \
         trigger:k8s_drift succeeded and did not move, so it is not refetched: {paths:#?}"
    );
}

/// `dive.bridges = ""` turns a watch into one request per tick.
#[tokio::test]
async fn a_watch_that_dives_into_nothing_never_fetches_a_child() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[bridgewatch_core::config::edit::Edit::Set {
        path: "watches.0.dive.bridges".into(),
        value: bridgewatch_core::config::edit::EditValue::String(String::new()),
    }]);
    let (mut poller, transport) = support::fixture_poller(&config, &dir);

    let tick = poller.tick().await;
    let paths = transport.seen();
    let (_, jobs, _, _) = requests_by_kind(&paths);
    assert_eq!(jobs, 1, "the parent's own jobs and no child: {paths:#?}");

    let row = &tick.snapshot.watches[0].rows[0];
    assert!(row.bridges.iter().all(|b| !b.dived));
    assert_eq!(
        row.bridges
            .iter()
            .find(|b| b.name == "trigger:android")
            .unwrap()
            .verdict,
        "failed",
        "an undived bridge still reports its own status"
    );
    assert_eq!(
        row.deploy, "absent",
        "and a marker inside a child that was never opened cannot be found, \
         which is the documented cost of not diving"
    );
}

/// `dive.only_when` keeps a noisy secondary watch cheap until something breaks.
#[tokio::test]
async fn only_when_failed_opens_just_the_failed_bridge() {
    let dir = support::fixtures_dir().join("schedule-hourly-failed");
    let config = support::config_with(&[]);
    let (mut poller, transport) = support::fixture_poller(&config, &dir);

    let tick = poller.tick().await;
    let paths = transport.seen();
    let (_, jobs, _, _) = requests_by_kind(&paths);
    assert_eq!(
        jobs, 2,
        "the parent, plus only the one failed bridge out of five: {paths:#?}"
    );

    let row = &tick
        .snapshot
        .watches
        .iter()
        .find(|w| w.id == "hourly")
        .unwrap()
        .rows[0];
    let dived: Vec<&str> = row
        .bridges
        .iter()
        .filter(|b| b.dived)
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(dived, ["trigger:k8s_drift"]);
}

// ---------------------------------------------------------------------------
// The planner in isolation
// ---------------------------------------------------------------------------

fn row(id: u64, status: &str, updated: &str) -> Pipeline {
    Pipeline {
        id,
        iid: None,
        project_id: Some(1),
        sha: "abcdef1234".into(),
        ref_name: "main".into(),
        status: Status::from(status.to_string()),
        source: Some("push".into()),
        web_url: None,
        created_at: None,
        updated_at: Some(updated.into()),
        started_at: None,
        finished_at: None,
    }
}

#[test]
fn the_plan_counts_two_requests_per_stale_pipeline() {
    let cache = PipelineCache::new();
    let plan = planner::plan(&[row(1, "success", "t1"), row(2, "running", "t2")], &cache);

    assert_eq!(plan.pipelines.len(), 2);
    assert!(plan.cached.is_empty());
    assert_eq!(
        plan.request_count(),
        1 + 2 * 2,
        "one list plus two per pipeline"
    );
    assert!(!plan.is_idle());
}

fn bridge(name: &str, status: &str, child: Option<(u64, &str)>) -> Bridge {
    Bridge {
        id: 10,
        name: name.into(),
        status: Status::from(status.to_string()),
        stage: None,
        allow_failure: false,
        started_at: None,
        web_url: None,
        downstream_pipeline: child.map(|(id, s)| DownstreamPipeline {
            id,
            project_id: Some(1),
            sha: None,
            ref_name: None,
            status: Status::from(s.to_string()),
            web_url: None,
        }),
    }
}

#[test]
fn a_child_is_fetched_when_it_is_live_or_when_it_moved() {
    let loaded = bridgewatch_core::config::parse_str(
        r#"
        [accounts.gl]
        token = { env = "T" }
        [[watches]]
        id = "x"
        account = "gl"
        project = 1
        "#,
        std::path::Path::new("t.toml"),
    )
    .unwrap();
    let rules = WatchRules::compile(&loaded.config.watches[0]).unwrap();

    let settled = vec![bridge("trigger:a", "success", Some((99, "success")))];
    let live = vec![bridge("trigger:a", "running", Some((99, "running")))];
    let held: HashSet<u64> = [99].into_iter().collect();
    let nothing: HashSet<u64> = HashSet::new();

    assert_eq!(
        planner::child_fetches(&settled, None, &nothing, &rules),
        [(99, Some(1))],
        "with nothing cached, every dived child is fetched once"
    );
    assert!(
        planner::child_fetches(&settled, Some(&settled), &held, &rules).is_empty(),
        "a settled child that did not move is not fetched again"
    );
    assert_eq!(
        planner::child_fetches(&live, Some(&live), &held, &rules).len(),
        1,
        "a live child is always refetched"
    );
    assert_eq!(
        planner::child_fetches(&settled, Some(&live), &held, &rules).len(),
        1,
        "a child that just settled is fetched one last time"
    );

    let dead = vec![bridge("trigger:a", "success", None)];
    assert!(
        planner::child_fetches(&dead, None, &nothing, &rules).is_empty(),
        "a dead bridge has nothing to fetch"
    );
}

/// A child whose jobs are not held is fetched even when its bridge is settled
/// and has not moved. That is the only thing standing between a dropped child
/// request and a bridge that reads its own `success` for the life of the
/// process.
#[test]
fn a_child_with_no_jobs_held_is_always_refetched() {
    let loaded = bridgewatch_core::config::parse_str(
        r#"
        [accounts.gl]
        token = { env = "T" }
        [[watches]]
        id = "x"
        account = "gl"
        project = 1
        "#,
        std::path::Path::new("t.toml"),
    )
    .unwrap();
    let rules = WatchRules::compile(&loaded.config.watches[0]).unwrap();

    let settled = vec![bridge("trigger:a", "success", Some((99, "success")))];
    let held: HashSet<u64> = [99].into_iter().collect();

    assert!(
        planner::child_fetches(&settled, Some(&settled), &held, &rules).is_empty(),
        "held, settled and unmoved: nothing to do"
    );
    assert_eq!(
        planner::child_fetches(&settled, Some(&settled), &HashSet::new(), &rules),
        [(99, Some(1))],
        "the same bridge with no jobs held is asked for again"
    );
}

// ---------------------------------------------------------------------------
// What a failed request costs the NEXT tick
// ---------------------------------------------------------------------------

/// A child request that failed is asked for again, and the failure is reported.
///
/// ⛔ The old arm logged the error at `debug`, stored the detail without that
/// child, and recorded the bridge's status as seen. The next tick then found a
/// settled row whose revision had not moved, planned nothing, and never asked
/// again — so one dropped connection hid a whole child pipeline for the life of
/// the process, with `errors: []` and a bridge reading the `success` a
/// `trigger:*` job without `strategy: depend` gets the moment its child is
/// created. Here that child carries the deploy marker, so the tick before the
/// retry cannot see the deploy at all.
#[tokio::test]
async fn a_child_request_that_failed_is_retried_and_reported() {
    use bridgewatch_core::config::edit::{Edit, EditValue};

    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    // A one-second schedule, so the backoff this error opens has expired by the
    // time the second tick comes round. A watch that is backing off sits ticks
    // out by design; see `should_defer`.
    let config = support::config_with(&[
        Edit::Set {
            path: "watches.0.poll.idle_secs".into(),
            value: EditValue::Integer(1),
        },
        Edit::Set {
            path: "watches.0.poll.live_secs".into(),
            value: EditValue::Integer(1),
        },
        Edit::Set {
            path: "accounts.gitlab.rate_limit_backoff.max_secs".into(),
            value: EditValue::Integer(1),
        },
    ]);

    let transport = std::sync::Arc::new(support::ScriptedTransport::load(&dir));
    transport.fail(
        "/pipelines/2856963963/jobs",
        1,
        bridgewatch_core::client::ClientError::Transport("timed out".into()),
    );
    let mut poller = support::poller_with_transport(&config, transport.clone());

    let first = poller.tick().await;
    assert!(
        !first.snapshot.errors.is_empty(),
        "the child that did not arrive is reported, not swallowed"
    );
    assert_eq!(
        first.snapshot.watches[0].rows[0].deploy, "absent",
        "and the marker inside it genuinely cannot be seen this tick"
    );

    transport.clear_seen();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = poller.tick().await;

    assert_eq!(
        transport.count("/pipelines/2856963963/jobs"),
        1,
        "the child is asked for again: {:#?}",
        transport.seen()
    );
    assert!(
        second.snapshot.errors.is_empty(),
        "and this time it arrives: {:?}",
        second.snapshot.errors
    );
    assert_eq!(second.snapshot.watches[0].rows[0].deploy, "live");
    assert_eq!(second.snapshot.icon_state.as_str(), "deployed");
}

/// A list request that fails keeps the frame the cache already holds.
///
/// Dropping the rows would put a primary watch on `unknown` for a doubled
/// interval over one dropped connection, while every detail behind it is still
/// in the cache. The honest report is the last verdict plus the error.
#[tokio::test]
async fn a_failed_list_keeps_the_last_frame_and_says_why() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[]);
    let transport = std::sync::Arc::new(support::ScriptedTransport::load(&dir));
    let mut poller = support::poller_with_transport(&config, transport.clone());

    let first = poller.tick().await;
    assert_eq!(first.snapshot.icon_state.as_str(), "deployed_with_failure");

    transport.fail(
        "/pipelines?",
        1,
        bridgewatch_core::client::ClientError::RateLimited {
            retry_after: Some(60),
            reset: None,
        },
    );
    let second = poller.tick().await;

    let watch = &second.snapshot.watches[0];
    assert_eq!(watch.id, "main-push");
    assert_eq!(
        watch.rows.len(),
        1,
        "the rows survive a list request that did not arrive"
    );
    assert_eq!(second.snapshot.icon_state.as_str(), "deployed_with_failure");
    assert!(
        watch.error.as_deref().unwrap_or_default().contains("rate"),
        "with the reason attached: {:?}",
        watch.error
    );
    assert!(!second.snapshot.errors.is_empty());
}

/// Walking a level deeper costs one `/bridges` for the child and one `/jobs`
/// for the grandchild. That is the price `dive.depth` is opt-in for.
#[tokio::test]
async fn a_depth_two_dive_costs_two_more_requests() {
    use bridgewatch_core::config::edit::{Edit, EditValue};

    let dir = support::fixtures_dir().join("synth-dive-depth-two");

    let (mut shallow, shallow_t) = support::fixture_poller(&support::config_with(&[]), &dir);
    shallow.tick().await;
    let shallow_paths = shallow_t.seen();

    let deep_config = support::config_with(&[Edit::Set {
        path: "watches.0.dive.depth".into(),
        value: EditValue::Integer(2),
    }]);
    let (mut deep, deep_t) = support::fixture_poller(&deep_config, &dir);
    deep.tick().await;
    let deep_paths = deep_t.seen();

    assert_eq!(
        deep_paths.len(),
        shallow_paths.len() + 2,
        "depth 1: {shallow_paths:#?}\ndepth 2: {deep_paths:#?}"
    );
    assert!(
        deep_paths
            .iter()
            .any(|p| p.contains("/pipelines/2900000002/bridges")),
        "the child's own trigger jobs"
    );
    assert!(
        deep_paths
            .iter()
            .any(|p| p.contains("/projects/91000001/pipelines/2900000003/jobs")),
        "and the grandchild's jobs, asked for in the project the DOWNSTREAM pipeline \
         names rather than the one the watch is configured with — a multi-project \
         trigger's child is somewhere else, and asking the watch's project 404s: \
         {deep_paths:#?}"
    );
    assert!(
        !shallow_paths
            .iter()
            .any(|p| p.contains("/pipelines/2900000003/")),
        "neither of which depth 1 asks for"
    );
}

// ---------------------------------------------------------------------------
// The list query, and what it can and cannot push to the API
// ---------------------------------------------------------------------------

fn watch_with(toml: &str) -> bridgewatch_core::config::Watch {
    let loaded = bridgewatch_core::config::parse_str(
        &format!(
            r#"
            [accounts.gl]
            token = {{ env = "T" }}
            [[watches]]
            id = "x"
            account = "gl"
            project = 1
            {toml}
            "#
        ),
        std::path::Path::new("t.toml"),
    )
    .expect("parses");
    loaded.config.watches.into_iter().next().unwrap()
}

/// An exact ref is pushed to the API. A source can be pushed only when there is
/// exactly one of them, so the page has to be big enough to survive the filter
/// running here instead.
///
/// ⚠️ README:495's "exact refs have no page limit" is true only for the single
/// case in the middle. `sources = []` filters nothing, so the page can be small;
/// two or more sources cannot be expressed in GitLab's one-valued `source`
/// parameter, so the rows have to be over-fetched and filtered locally.
#[test]
fn a_multi_source_watch_over_fetches_because_the_api_takes_one_source() {
    use bridgewatch_core::poll::list_query;

    let one = watch_with(
        r#"ref = "main"
            sources = ["push"]
            show = { max_rows = 2, settled = 1 }"#,
    );
    let rules = WatchRules::compile(&one).unwrap();
    let q = list_query(&one, &rules);
    assert_eq!(q.ref_name.as_deref(), Some("main"));
    assert_eq!(q.source.as_deref(), Some("push"));
    assert_eq!(q.per_page, 10, "clamped up from 2 x 4, and no more");

    let none = watch_with(
        r#"ref = "main"
            sources = []
            show = { max_rows = 2, settled = 1 }"#,
    );
    let rules = WatchRules::compile(&none).unwrap();
    let q = list_query(&none, &rules);
    assert_eq!(q.source, None);
    assert_eq!(q.per_page, 10, "an empty list filters nothing");

    let many = watch_with(
        r#"ref = "main"
            sources = ["push", "web"]
            show = { max_rows = 2, settled = 1 }"#,
    );
    let rules = WatchRules::compile(&many).unwrap();
    let q = list_query(&many, &rules);
    assert_eq!(
        q.source, None,
        "GitLab's `source` takes one value, so neither can be pushed"
    );
    assert_eq!(
        q.per_page, 30,
        "so the page is widened, or a project of hourly schedules returns a \
         page that is entirely discarded and a watch that shows nothing"
    );

    let globbed = watch_with(
        r#"ref = "pf/*"
            show = { max_rows = 2, settled = 1 }"#,
    );
    let rules = WatchRules::compile(&globbed).unwrap();
    let q = list_query(&globbed, &rules);
    assert_eq!(q.ref_name, None, "a glob cannot be pushed either");
    assert_eq!(q.order_by, "updated_at");
    assert_eq!(q.per_page, 30);
}

/// A primary watch always shows its newest match, whatever the trim says.
///
/// `show.settled = 0` means "only show me what is running", which on a quiet
/// day is nothing — and the icon is computed from the rows, so the tray read
/// `unknown` over a perfectly green estate. A secondary watch has no icon and
/// is left to show exactly what it was asked for.
#[test]
fn a_primary_watch_keeps_its_newest_row_when_the_trim_would_empty_it() {
    use bridgewatch_core::poll::select_rows;

    let rows = [row(7, "success", "t7"), row(6, "success", "t6")];

    let primary = watch_with(
        r#"ref = "main"
            role = "primary"
            show = { max_rows = 5, settled = 0 }"#,
    );
    let rules = WatchRules::compile(&primary).unwrap();
    let kept = select_rows(&rows, &primary, &rules);
    assert_eq!(
        kept.iter().map(|r| r.id).collect::<Vec<_>>(),
        [7],
        "the newest match, so the icon has something to be computed from"
    );

    let zero_rows = watch_with(
        r#"ref = "main"
            role = "primary"
            show = { max_rows = 0, settled = 1 }"#,
    );
    let rules = WatchRules::compile(&zero_rows).unwrap();
    assert_eq!(
        select_rows(&rows, &zero_rows, &rules)
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [7],
        "`max_rows = 0` is accepted by the schema and means the same thing"
    );

    let secondary = watch_with(
        r#"ref = "main"
            role = "secondary"
            show = { max_rows = 5, settled = 0 }"#,
    );
    let rules = WatchRules::compile(&secondary).unwrap();
    assert!(
        select_rows(&rows, &secondary, &rules).is_empty(),
        "a secondary watch paints no icon, so nothing has to be kept for one"
    );
}

/// ⛔ The live-cadence budget, pinned. `poll.live_secs` went from 20 to 5 for
/// 0.1.0, which quadruples what a busy estate costs, so the arithmetic in
/// `planner::live_tick_requests` is held to what the transport really saw.
///
/// ca41ab28, cold: three watches, the push pipeline with four dived children.
/// 3 lists + 1 `/jobs` + 1 `/bridges` + 4 child `/jobs` = 9, which is also the
/// worst case for a live tick of this estate (a live pipeline is re-fetched
/// exactly like an uncached one). At the default 5 s that is 108 requests a
/// minute, against the 2,000 a minute GitLab.com allows authenticated API
/// traffic per user.
#[tokio::test]
async fn the_live_tick_budget_matches_what_is_really_sent() {
    use bridgewatch_core::config::PollConfig;
    use bridgewatch_core::poll::planner::{
        GITLAB_COM_API_PER_MINUTE, GITLAB_COM_FREE_PROPOSED_PER_MINUTE, live_tick_requests,
        per_minute,
    };

    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[]);
    let (mut poller, transport) = support::fixture_poller(&config, &dir);
    poller.tick().await;
    let sent = transport.seen().len();

    // Three watches; only main-push matched a pipeline, and it dives four.
    let predicted = live_tick_requests(&[&[4], &[], &[]]);
    assert_eq!(predicted, 9);
    assert_eq!(sent, predicted, "{:#?}", transport.seen());

    let live_secs = PollConfig::default().live_secs;
    assert_eq!(live_secs, 5);
    assert_eq!(per_minute(live_secs, predicted), 108);
    assert!(per_minute(live_secs, predicted) * 10 < GITLAB_COM_API_PER_MINUTE);
    // ⚠️ And stated rather than hidden: under GitLab.com's PROPOSED Free-plan
    // limit (100 a minute, not in effect yet) this estate at 5 s is over
    // budget while anything runs. If that limit lands, this assertion flips
    // and the default needs revisiting.
    assert!(per_minute(live_secs, predicted) > GITLAB_COM_FREE_PROPOSED_PER_MINUTE);

    // A second, busy tick of the parked-gate fixture: the arithmetic holds for
    // a partly cached estate too (1 running parent, 1 pending child).
    let dir = support::fixtures_dir().join("9da437fd-parked-gate-stacked");
    let (mut poller, transport) = support::fixture_poller(&config, &dir);
    poller.tick().await;
    transport.clear_seen();
    poller.tick().await;
    assert_eq!(
        transport.seen().len(),
        live_tick_requests(&[&[1], &[], &[]])
    );
}

/// `per_minute` rounds up: a 7 s interval is 9 ticks a minute, not 8.
#[test]
fn per_minute_rounds_up() {
    use bridgewatch_core::poll::planner::per_minute;
    assert_eq!(per_minute(60, 3), 3);
    assert_eq!(per_minute(7, 1), 9);
    assert_eq!(per_minute(0, 1), 60, "a zero interval is treated as 1 s");
}
