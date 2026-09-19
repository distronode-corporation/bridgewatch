//! The commit group: a commit's several GitHub workflow runs as ONE row whose
//! bridges are the runs, driven through the real poller and the real verdict
//! engine.
//!
//! ⛔ **Every body here is hand-written, holds only the keys the client
//! decodes, and is about `acme-corp/monorepo`.** Nothing in this file has been
//! near the GitHub API. The shapes follow the measured key sets (a run is
//! `{id, name, head_sha, head_branch, status, conclusion, event, html_url,
//! created_at, updated_at, run_started_at}`; a page is wrapped in an object);
//! the values are invented.
//!
//! The verdict engine is not changed for any of this, and that is the claim
//! these tests hold: each one asserts a verdict the unchanged rules reach over
//! synthetic bridges.
//!
//! ⚠️ The transport double routes by PATH rather than replaying in order: a
//! commit group fans its job requests out over several runs, and the order the
//! poller visits them in is not what these tests are about. An unrouted
//! request still panics, so a request-count claim is still a claim.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use bridgewatch_core::client::http::{HttpRequest, HttpResponse, Transport};
use bridgewatch_core::client::{CiClient, ClientError, GitHubClient, ListQuery, RequestRing};
use bridgewatch_core::config::{Account, Config, GroupMode, ProjectRef, Provider};
use bridgewatch_core::model::Pipeline;
use bridgewatch_core::poll::Poller;
use bridgewatch_core::status::Status;
use bridgewatch_core::token::Secret;

const SHA: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00fedcba9";
const OTHER_SHA: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

/// One scripted response.
#[derive(Clone, Debug)]
struct Reply {
    status: u16,
    body: String,
    link: Option<String>,
}

impl Reply {
    fn ok(body: String) -> Self {
        Self {
            status: 200,
            body,
            link: None,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            body: r#"{"message":"scripted"}"#.to_string(),
            link: None,
        }
    }

    fn link(mut self, link: &str) -> Self {
        self.link = Some(link.to_string());
        self
    }
}

/// Answers each path from its own queue; the last reply for a path repeats.
#[derive(Debug, Default)]
struct Routed {
    routes: Mutex<HashMap<String, Vec<Reply>>>,
    seen: Mutex<Vec<String>>,
}

impl Routed {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn on(self: &Arc<Self>, path: &str, reply: Reply) -> Arc<Self> {
        self.routes
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_default()
            .push(reply);
        self.clone()
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn clear_seen(&self) {
        self.seen.lock().unwrap().clear();
    }

    fn count(&self, needle: &str) -> usize {
        self.seen().iter().filter(|p| p.contains(needle)).count()
    }
}

#[async_trait::async_trait]
impl Transport for Routed {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        let path = request.path.clone();
        self.seen.lock().unwrap().push(path.clone());
        let reply = {
            let mut routes = self.routes.lock().unwrap();
            let Some(queue) = routes.get_mut(&path) else {
                panic!("unrouted request: {path}");
            };
            if queue.len() > 1 {
                queue.remove(0)
            } else {
                queue[0].clone()
            }
        };
        Ok(HttpResponse {
            status: reply.status,
            body: reply.body,
            next_page: None,
            ratelimit_remaining: Some(4_998),
            ratelimit_reset: Some(1_789_669_380),
            retry_after: None,
            etag: None,
            link: reply.link,
            oauth_scopes: None,
        })
    }
}

/// A run, with only the keys the client decodes.
struct Run {
    id: u64,
    name: &'static str,
    sha: &'static str,
    event: &'static str,
    branch: &'static str,
    created: &'static str,
    status: &'static str,
    conclusion: &'static str,
}

impl Run {
    fn push(id: u64, name: &'static str, created: &'static str, conclusion: &'static str) -> Self {
        Self {
            id,
            name,
            sha: SHA,
            event: "push",
            branch: "main",
            created,
            status: if conclusion.is_empty() {
                "in_progress"
            } else {
                "completed"
            },
            conclusion,
        }
    }

    fn body(&self) -> String {
        let conclusion = if self.conclusion.is_empty() {
            "null".to_string()
        } else {
            format!("\"{}\"", self.conclusion)
        };
        format!(
            r#"{{"id":{id},"name":"{name}","run_number":7,"head_sha":"{sha}",
                "head_branch":"{branch}","status":"{status}","conclusion":{conclusion},
                "event":"{event}",
                "html_url":"https://github.com/acme-corp/monorepo/actions/runs/{id}",
                "created_at":"{created}","updated_at":"{created}","run_started_at":"{created}"}}"#,
            id = self.id,
            name = self.name,
            sha = self.sha,
            branch = self.branch,
            status = self.status,
            event = self.event,
            created = self.created,
        )
    }
}

fn runs_page(runs: &[Run]) -> String {
    let bodies: Vec<String> = runs.iter().map(Run::body).collect();
    format!(
        r#"{{"total_count":{},"workflow_runs":[{}]}}"#,
        runs.len(),
        bodies.join(",")
    )
}

/// One job, with only the keys the client decodes.
fn job(id: u64, name: &str, conclusion: &str, from: &str, to: &str) -> String {
    let (status, conclusion, to) = if conclusion.is_empty() {
        ("in_progress", "null".to_string(), "null".to_string())
    } else {
        (
            "completed",
            format!("\"{conclusion}\""),
            format!("\"{to}\""),
        )
    };
    format!(
        r#"{{"id":{id},"name":"{name}","status":"{status}","conclusion":{conclusion},
            "started_at":"{from}","completed_at":{to},
            "html_url":"https://github.com/acme-corp/monorepo/actions/runs/1/job/{id}"}}"#
    )
}

fn jobs_page(jobs: &[String]) -> String {
    format!(
        r#"{{"total_count":{},"jobs":[{}]}}"#,
        jobs.len(),
        jobs.join(",")
    )
}

fn jobs_path(run: u64) -> String {
    format!("/repos/acme-corp/monorepo/actions/runs/{run}/jobs?filter=latest&per_page=100")
}

/// The list request a `ref = "main"`, one-source watch with default `show`
/// makes: 5 rows × 4 = a page of 20.
fn list_path(event: &str) -> String {
    format!(
        "/repos/acme-corp/monorepo/actions/runs?branch=main&event={event}&exclude_pull_requests=true&per_page=20&page=1"
    )
}

fn config(extra: &str) -> Config {
    let raw = format!(
        r#"
[accounts.gh]
provider = "github"
token = {{ env = "TOK" }}

[[watches]]
id = "bw-push"
account = "gh"
project = "acme-corp/monorepo"
ref = "main"
{extra}
"#
    );
    bridgewatch_core::config::parse_str(&raw, std::path::Path::new("test.toml"))
        .expect("a GitHub configuration loads")
        .config
}

/// The usual commit-group watch: pushes to main, `publish` means deployed.
fn group_config(extra: &str) -> Config {
    config(&format!(
        "sources = [\"push\"]\ngroup = \"commit\"\ndeploy_markers = [\"publish\"]\n{extra}"
    ))
}

fn poller_for(config: &Config, transport: Arc<Routed>) -> Poller {
    let ring = RequestRing::new(50);
    let mut clients: BTreeMap<String, Arc<dyn CiClient>> = BTreeMap::new();
    for (name, account) in &config.accounts {
        clients.insert(
            name.clone(),
            bridgewatch_core::client::client_for(
                account,
                &Secret::new("ghp_SECRET"),
                transport.clone(),
                ring.clone(),
            )
            .expect("the factory builds a GitHub client"),
        );
    }
    Poller::with_clients(config, clients, ring).expect("poller builds")
}

fn client(transport: Arc<Routed>) -> GitHubClient {
    GitHubClient::new(
        &Account::for_provider(Provider::Github),
        &Secret::new("ghp_SECRET"),
        transport,
        RequestRing::new(10),
    )
}

fn repo() -> ProjectRef {
    ProjectRef::Path("acme-corp/monorepo".into())
}

fn grouped(event: &str, per_page: u32, window: u64) -> ListQuery {
    ListQuery::exact("main", Some(event.to_string()), per_page).in_commit_groups(Some(window))
}

fn ids(rows: &[Pipeline]) -> Vec<u64> {
    rows.iter().map(|r| r.id).collect()
}

/// The push this file keeps coming back to: CI, Deploy and Lint, one push, the
/// same second. Deploy's `publish` succeeded and Lint failed.
fn three_way_push(transport: &Arc<Routed>) {
    transport
        .on(
            &list_path("push"),
            Reply::ok(runs_page(&[
                Run::push(4103, "Lint", "2026-09-18T09:00:01Z", "failure"),
                Run::push(4102, "Deploy", "2026-09-18T09:00:00Z", "success"),
                Run::push(4101, "CI", "2026-09-18T09:00:00Z", "success"),
            ])),
        )
        .on(
            &jobs_path(4101),
            Reply::ok(jobs_page(&[job(
                9101,
                "test",
                "success",
                "2026-09-18T09:00:10Z",
                "2026-09-18T09:02:00Z",
            )])),
        )
        .on(
            &jobs_path(4102),
            Reply::ok(jobs_page(&[
                job(
                    9201,
                    "build",
                    "success",
                    "2026-09-18T09:00:10Z",
                    "2026-09-18T09:01:00Z",
                ),
                job(
                    9202,
                    "publish",
                    "success",
                    "2026-09-18T09:01:00Z",
                    "2026-09-18T09:03:00Z",
                ),
            ])),
        )
        .on(
            &jobs_path(4103),
            Reply::ok(jobs_page(&[job(
                9301,
                "eslint",
                "failure",
                "2026-09-18T09:00:10Z",
                "2026-09-18T09:00:40Z",
            )])),
        );
}

// ---------------------------------------------------------------------------
// The tool's whole thesis, on GitHub
// ---------------------------------------------------------------------------

/// ⛔ A push that fans out into three workflows is ONE row. Deploy's marker
/// succeeded and Lint failed, which is `deployed_with_failure`: the same
/// verdict a GitLab parent reaches when one child deployed and a sibling bridge
/// failed, reached by the same unchanged rules.
#[tokio::test]
async fn a_push_that_fans_out_into_three_workflows_is_one_row_that_deployed_with_a_failure() {
    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(&group_config(""), transport.clone());

    let snapshot = poller.tick().await.snapshot;

    assert_eq!(snapshot.icon_state.as_str(), "deployed_with_failure");
    let rows = &snapshot.watches[0].rows;
    assert_eq!(rows.len(), 1, "three runs, one push, one row: {rows:#?}");
    let row = &rows[0];
    assert_eq!(row.id, 4103, "the group's id is its newest run's");
    assert_eq!(
        row.iid, None,
        "run numbers are per workflow; a group has none"
    );
    assert_eq!(row.sha, SHA);
    assert_eq!(row.ref_name, "main");
    assert_eq!(row.source.as_deref(), Some("push"));
    assert_eq!(row.status, "failed", "the worst of the three, all settled");
    assert_eq!(
        row.web_url.as_deref(),
        Some(format!("https://github.com/acme-corp/monorepo/commit/{SHA}/checks").as_str()),
        "the WEB host (api. off), and the commit's checks page, which is the one \
         page GitHub has for a whole push"
    );
    assert!(
        row.parent_jobs.is_empty(),
        "the group has no jobs of its own"
    );
    let bridges: Vec<(&str, &str, bool)> = row
        .bridges
        .iter()
        .map(|b| (b.name.as_str(), b.verdict.as_str(), b.dived))
        .collect();
    assert_eq!(
        bridges,
        [
            ("CI", "passed", true),
            ("Deploy", "passed", true),
            ("Lint", "failed", true)
        ],
        "one bridge per run, named for its workflow, oldest run first"
    );
    assert_eq!(row.bridges[2].verdict_jobs, ["eslint"]);
    assert_eq!(row.bridges[1].child_id, Some(4102));
    assert_eq!(row.deploy, "live");
    let marker = row.deploy_marker.as_ref().expect("the marker was found");
    assert_eq!(
        (marker.name.as_str(), marker.pipeline_id),
        ("publish", 4102),
        "found inside the Deploy run, a scope reached only through its bridge"
    );
    assert_eq!(row.sibling_failures, ["Lint"]);
    assert_eq!(row.failures, ["eslint"]);
    assert!(!row.live);

    // One list, and one /jobs per run. Nothing for the group itself.
    let mut seen = transport.seen();
    seen.sort();
    let mut want = vec![
        list_path("push"),
        jobs_path(4101),
        jobs_path(4102),
        jobs_path(4103),
    ];
    want.sort();
    assert_eq!(seen, want);

    // What `check --json` prints is this snapshot serialised whole, so the
    // group reaches a script through the same keys a GitLab parent does.
    let json = serde_json::to_value(&snapshot).expect("a snapshot serialises");
    let row = &json["watches"][0]["rows"][0];
    assert_eq!(row["state"], "deployed_with_failure");
    assert_eq!(row["bridges"][2]["verdict"], "failed");
    assert_eq!(row["bridges"][1]["child_id"], 4102);
    assert_eq!(row["parent_jobs"], serde_json::json!([]));
}

/// And `sibling_failure = "fail"` turns the same push red, exactly as it would
/// a GitLab parent's: the policy reads the synthetic bridges like any others.
#[tokio::test]
async fn the_sibling_failure_policy_applies_to_a_failed_workflow() {
    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(
        &group_config("sibling_failure = \"fail\""),
        transport.clone(),
    );

    let snapshot = poller.tick().await.snapshot;

    assert_eq!(snapshot.icon_state.as_str(), "failed");
}

// ---------------------------------------------------------------------------
// The key and the window
// ---------------------------------------------------------------------------

/// ⛔ `head_sha` alone is not a key. A branch head that did not move collects
/// every scheduled run fired against it; a day apart they are two rows.
#[tokio::test]
async fn two_schedule_runs_a_day_apart_on_one_head_sha_are_two_rows() {
    let schedule = |id, created, conclusion| Run {
        id,
        name: "Nightly",
        sha: SHA,
        event: "schedule",
        branch: "main",
        created,
        status: "completed",
        conclusion,
    };
    let transport = Routed::new()
        .on(
            &list_path("schedule"),
            Reply::ok(runs_page(&[
                schedule(5202, "2026-09-18T03:00:00Z", "failure"),
                schedule(5201, "2026-09-17T03:00:00Z", "success"),
            ])),
        )
        .on(&jobs_path(5202), Reply::ok(jobs_page(&[])))
        .on(&jobs_path(5201), Reply::ok(jobs_page(&[])));
    let config = config(
        "sources = [\"schedule\"]\ngroup = \"commit\"\nrole = \"secondary\"\n\
         show = { settled = 2 }",
    );
    let mut poller = poller_for(&config, transport);

    let snapshot = poller.tick().await.snapshot;
    let rows = &snapshot.watches[0].rows;

    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        [5202, 5201],
        "one row per night, not one row for the branch head"
    );
    assert_eq!(rows[0].status, "failed");
    assert_eq!(rows[1].status, "success");
}

/// A run created later than the window, measured from the group's NEWEST run,
/// starts a group of its own; one inside it joins, the edge included.
#[tokio::test]
async fn a_run_arriving_after_the_window_starts_a_new_group() {
    let transport = Routed::new().on(
        "/repos/acme-corp/monorepo/actions/runs?branch=main&event=push&exclude_pull_requests=true&per_page=20&page=1",
        Reply::ok(runs_page(&[
            Run::push(4104, "Docs", "2026-09-18T09:05:00Z", "success"),
            Run::push(4103, "Lint", "2026-09-18T09:01:30Z", "success"),
            Run::push(4102, "Deploy", "2026-09-18T09:00:30Z", "success"),
            Run::push(4101, "CI", "2026-09-18T09:00:00Z", "success"),
        ])),
    );
    let client = client(transport.clone());

    let rows = client
        .list_pipelines(&repo(), &grouped("push", 20, 90))
        .await
        .unwrap();

    // 4103 anchors a group at 09:01:30; 4102 is 60 s older and joins, 4101 is
    // exactly 90 s older and joins too (the window is inclusive). 4104 is
    // three and a half minutes later than all of them: a new row.
    assert_eq!(ids(&rows), [4104, 4103]);
    let (_, bridges) =
        CiClient::listed_detail(&client, &repo(), &rows[1], &grouped("push", 20, 90))
            .await
            .unwrap();
    assert_eq!(
        bridges.iter().map(|b| b.id).collect::<Vec<_>>(),
        [4101, 4102, 4103]
    );
    assert_eq!(
        rows[1].created_at.as_deref(),
        Some("2026-09-18T09:00:00Z"),
        "a group was created when its first run was"
    );
    assert_eq!(
        rows[1].updated_at.as_deref(),
        Some("2026-09-18T09:01:30Z"),
        "and last touched when its latest run was, which is what the cache keys on"
    );

    // With a window of 30 s the same page is three groups: the anchor moves to
    // each group's own newest run, it does not slide along the chain.
    let rows = client
        .list_pipelines(&repo(), &grouped("push", 20, 30))
        .await
        .unwrap();
    assert_eq!(ids(&rows), [4104, 4103, 4102]);
    assert_eq!(transport.seen().len(), 2, "one list each, and nothing else");
}

/// One commit pushed to two branches is two pushes. A glob watch sees both
/// branches in one list, and one branch's failure must not be filed under the
/// other's row.
#[tokio::test]
async fn one_commit_pushed_to_two_branches_is_two_rows() {
    let on = |id, branch| Run {
        id,
        name: "CI",
        sha: SHA,
        event: "push",
        branch,
        created: "2026-09-18T09:00:00Z",
        status: "completed",
        conclusion: "success",
    };
    let transport = Routed::new().on(
        "/repos/acme-corp/monorepo/actions/runs?exclude_pull_requests=true&per_page=30&page=1",
        Reply::ok(runs_page(&[on(4202, "release/2"), on(4201, "main")])),
    );
    let client = client(transport);

    let rows = client
        .list_pipelines(&repo(), &ListQuery::scan(30).in_commit_groups(Some(90)))
        .await
        .unwrap();

    assert_eq!(ids(&rows), [4202, 4201]);
    assert_eq!(rows[0].ref_name, "release/2");
    assert_eq!(rows[1].ref_name, "main");
}

/// ⛔ A page that came back full may have cut its oldest group in half, and a
/// half group is a wrong verdict: the run that did not fit could be the red
/// one. The groups whose window reaches the oldest run on the page are dropped.
#[tokio::test]
async fn a_full_page_drops_the_oldest_group_rather_than_show_half_of_it() {
    let older = |id, name, created| Run {
        id,
        name,
        sha: OTHER_SHA,
        event: "push",
        branch: "main",
        created,
        status: "completed",
        conclusion: "success",
    };
    let page = runs_page(&[
        Run::push(4302, "Lint", "2026-09-18T10:00:00Z", "success"),
        Run::push(4301, "CI", "2026-09-18T10:00:00Z", "success"),
        older(4202, "Lint", "2026-09-18T09:00:00Z"),
        older(4201, "CI", "2026-09-18T09:00:00Z"),
    ]);
    let full = "/repos/acme-corp/monorepo/actions/runs?branch=main&event=push&exclude_pull_requests=true&per_page=4&page=1";
    let roomy = "/repos/acme-corp/monorepo/actions/runs?branch=main&event=push&exclude_pull_requests=true&per_page=5&page=1";
    let transport = Routed::new()
        .on(full, Reply::ok(page.clone()))
        .on(roomy, Reply::ok(page));
    let client = client(transport);

    let rows = client
        .list_pipelines(&repo(), &grouped("push", 4, 90))
        .await
        .unwrap();
    assert_eq!(
        ids(&rows),
        [4302],
        "four of four asked for: the older push may have a third run on page 2"
    );

    let rows = client
        .list_pipelines(&repo(), &grouped("push", 5, 90))
        .await
        .unwrap();
    assert_eq!(
        ids(&rows),
        [4302, 4202],
        "four of five: the page is everything there is, so both groups are whole"
    );
}

// ---------------------------------------------------------------------------
// Live groups, and what the next tick costs
// ---------------------------------------------------------------------------

/// A group with one workflow still running is running, and live, whatever its
/// finished siblings say. The next tick asks again only for the run that is
/// still moving: `planner::child_fetches` is unchanged and holds.
#[tokio::test]
async fn a_group_still_running_is_live_and_only_its_live_run_is_asked_for_again() {
    let transport = Routed::new()
        .on(
            &list_path("push"),
            Reply::ok(runs_page(&[
                Run::push(4102, "Deploy", "2026-09-18T09:00:00Z", ""),
                Run::push(4101, "CI", "2026-09-18T09:00:00Z", "success"),
            ])),
        )
        .on(
            &jobs_path(4101),
            Reply::ok(jobs_page(&[job(
                9101,
                "test",
                "success",
                "2026-09-18T09:00:10Z",
                "2026-09-18T09:02:00Z",
            )])),
        )
        .on(
            &jobs_path(4102),
            Reply::ok(jobs_page(&[job(
                9201,
                "publish",
                "",
                "2026-09-18T09:01:00Z",
                "",
            )])),
        );
    let mut poller = poller_for(&group_config(""), transport.clone());

    let first = poller.tick().await.snapshot;
    let row = &first.watches[0].rows[0];
    assert_eq!(row.status, "running");
    assert!(
        row.live,
        "a workflow is still running, so the fast interval holds"
    );
    assert_eq!(row.state.as_str(), "running");
    assert_eq!(row.deploy, "in_progress");
    assert_eq!(row.bridges[1].verdict, "running");

    transport.clear_seen();
    let second = poller.tick().await.snapshot;
    assert_eq!(
        transport.seen(),
        [list_path("push"), jobs_path(4102)],
        "the list, and the one run still moving; the settled CI run is held"
    );
    assert_eq!(second.watches[0].rows[0].state.as_str(), "running");
}

/// ⛔ Review B2 on the synthetic bridges: a run's jobs that did not arrive are
/// reported, and asked for AGAIN on the next tick even though the group has
/// settled and its revision has not moved. `held` is what does it, unchanged.
#[tokio::test]
async fn a_runs_jobs_that_failed_to_arrive_are_asked_for_again() {
    let transport = Routed::new();
    three_way_push(&transport);
    // The Deploy run's jobs fail once, then arrive.
    let deploy = jobs_path(4102);
    let arrives = transport.routes.lock().unwrap().remove(&deploy).unwrap();
    transport
        .on(&deploy, Reply::status(502))
        .on(&deploy, arrives[0].clone());
    // A one-second schedule and backoff ceiling, so the backoff this error
    // opens has expired by the second tick (a watch that is backing off sits
    // ticks out by design; see `should_defer`). The account's table comes
    // before the watch in the file, so its ceiling is set here instead.
    let mut config = group_config("poll = { live_secs = 1, idle_secs = 1 }");
    config
        .accounts
        .get_mut("gh")
        .expect("the account")
        .rate_limit_backoff
        .max_secs = 1;
    let mut poller = poller_for(&config, transport.clone());

    let first = poller.tick().await.snapshot;
    assert!(
        !first.errors.is_empty(),
        "the run that did not arrive is reported, not swallowed"
    );
    assert_eq!(
        first.watches[0].rows[0].deploy, "absent",
        "and the marker inside it genuinely cannot be seen this tick"
    );

    transport.clear_seen();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = poller.tick().await.snapshot;
    assert_eq!(transport.count(&deploy), 1, "{:#?}", transport.seen());
    assert_eq!(
        transport.count("/jobs"),
        1,
        "and only that one: the runs that did arrive are held"
    );
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(second.icon_state.as_str(), "deployed_with_failure");
}

// ---------------------------------------------------------------------------
// dive over workflow names
// ---------------------------------------------------------------------------

/// `dive.exclude` names WORKFLOWS on a commit group. An excluded run costs no
/// request and is drawn from its own status, so its failure still counts:
/// exclude saves a request, it does not hide a failure. Hiding one is
/// `[watches.jobs]` `ignore`, which drops the bridge, exactly as on GitLab.
#[tokio::test]
async fn dive_exclude_on_a_workflow_name_skips_its_jobs_but_not_its_failure() {
    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(
        &group_config("dive = { exclude = [\"Lint\"] }"),
        transport.clone(),
    );

    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];

    assert_eq!(
        transport.count(&jobs_path(4103)),
        0,
        "{:#?}",
        transport.seen()
    );
    assert_eq!(transport.count("/jobs"), 2);
    let lint = &row.bridges[2];
    assert_eq!((lint.name.as_str(), lint.dived), ("Lint", false));
    assert_eq!(lint.verdict, "failed", "read from the run's own conclusion");
    assert!(lint.verdict_jobs.is_empty(), "no jobs were read to name");
    assert_eq!(row.sibling_failures, ["Lint"]);
    assert_eq!(snapshot.icon_state.as_str(), "deployed_with_failure");

    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(
        &group_config("dive = { exclude = [\"Lint\"] }\n[watches.jobs]\n\"Lint\" = \"ignore\""),
        transport,
    );
    let snapshot = poller.tick().await.snapshot;
    assert_eq!(
        snapshot.icon_state.as_str(),
        "deployed",
        "a workflow the file says to ignore is gone from every verdict"
    );
    assert_eq!(snapshot.watches[0].rows[0].bridges.len(), 2);
}

/// `dive.bridges` is a glob over workflow names too.
#[tokio::test]
async fn dive_bridges_selects_workflows_by_name() {
    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(
        &group_config("dive = { bridges = \"Dep*\" }"),
        transport.clone(),
    );

    let snapshot = poller.tick().await.snapshot;

    assert_eq!(transport.count("/jobs"), 1);
    assert_eq!(transport.count(&jobs_path(4102)), 1);
    let dived: Vec<bool> = snapshot.watches[0].rows[0]
        .bridges
        .iter()
        .map(|b| b.dived)
        .collect();
    assert_eq!(dived, [false, true, false]);
    assert_eq!(snapshot.icon_state.as_str(), "deployed_with_failure");
}

/// `dive.depth` above 1 walks into each run's own bridges, and a run has none:
/// the client answers that without a request, so a deeper dive costs nothing
/// and changes nothing (validation already says the key is ignored here).
#[tokio::test]
async fn a_deeper_dive_costs_no_request_on_a_commit_group() {
    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(&group_config("dive = { depth = 3 }"), transport.clone());

    let snapshot = poller.tick().await.snapshot;

    assert_eq!(transport.seen().len(), 4, "{:#?}", transport.seen());
    assert!(
        snapshot.watches[0].rows[0]
            .bridges
            .iter()
            .all(|b| b.bridges.is_empty())
    );
    assert_eq!(snapshot.icon_state.as_str(), "deployed_with_failure");
}

// ---------------------------------------------------------------------------
// Deploy markers across runs
// ---------------------------------------------------------------------------

/// Post-deploy and marker-scope failures read the runs as scopes, unchanged.
/// A failure in ANOTHER workflow that started after the marker succeeded is a
/// post-deploy failure. ⚠️ The marker here is in the NEWEST run, whose id is
/// also the group's: the parent scope and the run's scope share an id, and the
/// parent's is empty, so nothing is counted twice.
#[tokio::test]
async fn a_failure_after_the_deploy_in_another_workflow_is_a_post_deploy_failure() {
    let transport = Routed::new()
        .on(
            &list_path("push"),
            Reply::ok(runs_page(&[
                Run::push(4102, "Deploy", "2026-09-18T09:00:00Z", "failure"),
                Run::push(4101, "Smoke", "2026-09-18T09:00:00Z", "failure"),
            ])),
        )
        .on(
            &jobs_path(4102),
            Reply::ok(jobs_page(&[
                job(
                    9201,
                    "migrate-check",
                    "failure",
                    "2026-09-18T09:00:10Z",
                    "2026-09-18T09:00:20Z",
                ),
                job(
                    9202,
                    "publish",
                    "success",
                    "2026-09-18T09:01:00Z",
                    "2026-09-18T09:02:00Z",
                ),
            ])),
        )
        .on(
            &jobs_path(4101),
            Reply::ok(jobs_page(&[job(
                9101,
                "smoke-test",
                "failure",
                "2026-09-18T09:05:00Z",
                "2026-09-18T09:06:00Z",
            )])),
        );
    let mut poller = poller_for(&group_config(""), transport);

    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];

    assert_eq!(row.id, 4102);
    assert_eq!(row.deploy, "live");
    assert_eq!(row.deploy_marker.as_ref().unwrap().pipeline_id, 4102);
    assert_eq!(row.post_deploy_failures, ["smoke-test"]);
    assert_eq!(
        row.deploy_failures,
        ["migrate-check"],
        "the pre-marker failure in the marker's own run, counted once"
    );
    assert_eq!(row.state.as_str(), "deployed_with_failure");
}

// ---------------------------------------------------------------------------
// Pagination of a run's jobs
// ---------------------------------------------------------------------------

/// A run inside a group pages its jobs by `Link` exactly as a run on its own
/// does, following the rewritten `/repositories/<id>/` URL verbatim. The marker
/// is on page 2, so a pager that stopped at 100 would report no deploy.
#[tokio::test]
async fn a_runs_jobs_paginate_past_100_inside_a_group() {
    let page_one: Vec<String> = (0..100)
        .map(|i| {
            job(
                10_000 + i,
                &format!("matrix ({i})"),
                "success",
                "2026-09-18T09:00:10Z",
                "2026-09-18T09:00:50Z",
            )
        })
        .collect();
    let page_two = "/repositories/99/actions/runs/4102/jobs?filter=latest&per_page=100&page=2";
    let transport = Routed::new()
        .on(
            &list_path("push"),
            Reply::ok(runs_page(&[
                Run::push(4102, "Deploy", "2026-09-18T09:00:00Z", "success"),
                Run::push(4101, "CI", "2026-09-18T09:00:00Z", "success"),
            ])),
        )
        .on(&jobs_path(4101), Reply::ok(jobs_page(&[])))
        .on(
            &jobs_path(4102),
            Reply::ok(jobs_page(&page_one)).link(&format!(
                "<https://api.github.com{page_two}>; rel=\"next\", \
                 <https://api.github.com{page_two}>; rel=\"last\""
            )),
        )
        .on(
            page_two,
            Reply::ok(jobs_page(&[job(
                10_100,
                "publish",
                "success",
                "2026-09-18T09:01:00Z",
                "2026-09-18T09:02:00Z",
            )])),
        );
    let mut poller = poller_for(&group_config(""), transport.clone());

    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];

    assert_eq!(transport.count(page_two), 1, "{:#?}", transport.seen());
    assert_eq!(row.bridges[1].jobs.len(), 101);
    assert_eq!(row.deploy, "live");
    assert_eq!(snapshot.icon_state.as_str(), "deployed");
}

// ---------------------------------------------------------------------------
// Gates, and the empty parent
// ---------------------------------------------------------------------------

/// ⛔ A fork pull request from a first-time contributor, inside a group: CI is
/// `action_required` with zero jobs, and a labeller that needs no approval
/// passed. The group is parked, not red and not green.
///
/// ⚠️ Hand-built from the documented shape: no live sample of
/// `action_required` exists, because capturing one needs a fork PR.
#[tokio::test]
async fn a_fork_pull_request_awaiting_approval_inside_a_group_is_parked() {
    let pr = |id, name, conclusion| Run {
        id,
        name,
        sha: SHA,
        event: "pull_request",
        branch: "main",
        created: "2026-09-18T09:00:00Z",
        status: "completed",
        conclusion,
    };
    let transport = Routed::new()
        .on(
            &list_path("pull_request"),
            Reply::ok(runs_page(&[
                pr(4402, "CI", "action_required"),
                pr(4401, "Labeler", "success"),
            ])),
        )
        .on(&jobs_path(4402), Reply::ok(jobs_page(&[])))
        .on(
            &jobs_path(4401),
            Reply::ok(jobs_page(&[job(
                9401,
                "label",
                "success",
                "2026-09-18T09:00:10Z",
                "2026-09-18T09:00:20Z",
            )])),
        );
    let config = config("sources = [\"pull_request\"]\ngroup = \"commit\"");
    let mut poller = poller_for(&config, transport);

    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];

    assert_eq!(snapshot.icon_state.as_str(), "parked_gate");
    assert_eq!(row.status, "manual");
    assert_eq!(row.bridges[1].verdict, "awaiting_gate");
    assert_eq!(
        row.gates,
        Vec::<String>::new(),
        "no JOB waits; the run does"
    );
    assert!(row.failures.is_empty());
    assert!(!row.live);
}

/// ⛔ The hazard the scoping report named. A group has NO jobs of its own,
/// which is the shape review B1 was about: an empty job list must not be read
/// as "nothing is known" (it was fetched) nor as "nothing failed" when a
/// workflow did. With `dive.bridges = ""` nothing but the list is read, and:
///
/// * a failed workflow still reds the row, from its bridge, and
/// * an all-green group reads `succeeded_no_deploy`, not `unknown`, which is
///   what it would read if the empty parent were taken as unavailable.
#[tokio::test]
async fn the_groups_empty_job_list_is_read_as_fetched_and_its_failed_workflow_still_counts() {
    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(
        &group_config("dive = { bridges = \"\" }"),
        transport.clone(),
    );

    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];

    assert_eq!(
        transport.seen(),
        [list_path("push")],
        "the list and nothing else"
    );
    assert!(row.parent_jobs.is_empty());
    assert_eq!(
        row.deploy, "absent",
        "no run was dived, so no marker is visible"
    );
    assert_eq!(
        row.state.as_str(),
        "failed",
        "not succeeded_no_deploy: the empty parent plus a failed bridge is a failure"
    );
    assert_eq!(
        row.failures,
        ["Lint"],
        "named by its workflow, no job was read"
    );

    let transport = Routed::new().on(
        &list_path("push"),
        Reply::ok(runs_page(&[
            Run::push(4102, "Docs", "2026-09-18T09:00:00Z", "success"),
            Run::push(4101, "CI", "2026-09-18T09:00:00Z", "skipped"),
        ])),
    );
    let config = config("sources = [\"push\"]\ngroup = \"commit\"\ndive = { bridges = \"\" }");
    let mut poller = poller_for(&config, transport);
    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];
    assert_eq!(
        row.status, "success",
        "one skipped workflow does not make the push 'not built'"
    );
    assert_eq!(snapshot.icon_state.as_str(), "succeeded_no_deploy");
}

// ---------------------------------------------------------------------------
// group = "run" is phase 2, untouched
// ---------------------------------------------------------------------------

/// Without `group` a GitHub watch is exactly what it was: one row per run, the
/// run's own jobs, no bridges, and the same requests.
#[tokio::test]
async fn without_group_every_run_is_its_own_row_as_before() {
    let transport = Routed::new();
    three_way_push(&transport);
    let mut poller = poller_for(
        &config("sources = [\"push\"]\ndeploy_markers = [\"publish\"]\nshow = { settled = 3 }"),
        transport.clone(),
    );

    let snapshot = poller.tick().await.snapshot;
    let rows = &snapshot.watches[0].rows;

    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        [4103, 4102, 4101],
        "one row per run"
    );
    assert!(rows.iter().all(|r| r.bridges.is_empty()));
    assert_eq!(rows[0].parent_jobs[0].name, "eslint");
    assert_eq!(
        rows[0].web_url.as_deref(),
        Some("https://github.com/acme-corp/monorepo/actions/runs/4103")
    );
    assert_eq!(
        transport.seen()[0],
        list_path("push"),
        "the same list request, byte for byte"
    );
    assert_eq!(transport.count("/jobs"), rows.len());
}

/// And the client, asked for a run's bridges and children with no grouped list
/// behind it, answers empty and sends nothing, as it did before groups existed.
#[tokio::test]
async fn without_a_grouped_list_bridges_and_child_jobs_send_nothing() {
    let transport = Routed::new();
    let client = client(transport.clone());

    assert!(
        CiClient::pipeline_bridges(&client, &repo(), 4101)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        CiClient::child_jobs(&client, &repo(), 4101)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(transport.seen().is_empty());
}

// ---------------------------------------------------------------------------
// A row the client did not just list
// ---------------------------------------------------------------------------

/// The memo is a cache of a response, not a source of truth. A group row this
/// client did not list is answered by asking for that commit's runs by
/// `head_sha`, one request, and its runs then count as presented, so their jobs
/// can be fetched. A run nobody presented still costs nothing.
#[tokio::test]
async fn a_group_row_the_client_did_not_list_is_rebuilt_from_its_commit() {
    let by_sha = format!(
        "/repos/acme-corp/monorepo/actions/runs?branch=main&event=push&head_sha={SHA}&exclude_pull_requests=true&per_page=100&page=1"
    );
    let transport = Routed::new()
        .on(
            &by_sha,
            Reply::ok(runs_page(&[
                Run::push(4102, "Deploy", "2026-09-18T09:00:00Z", "success"),
                Run::push(4101, "CI", "2026-09-18T09:00:00Z", "success"),
            ])),
        )
        .on(&jobs_path(4101), Reply::ok(jobs_page(&[])));
    let client = client(transport.clone());
    let row = Pipeline {
        id: 4102,
        iid: None,
        project_id: None,
        sha: SHA.to_string(),
        ref_name: "main".into(),
        status: Status::Success,
        source: Some("push".into()),
        web_url: None,
        created_at: None,
        updated_at: None,
        started_at: None,
        finished_at: None,
    };

    let (jobs, bridges) = CiClient::listed_detail(&client, &repo(), &row, &grouped("push", 20, 90))
        .await
        .unwrap();

    assert!(jobs.is_empty());
    assert_eq!(
        bridges.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["CI", "Deploy"]
    );
    CiClient::child_jobs(&client, &repo(), 4101).await.unwrap();
    assert!(
        CiClient::child_jobs(&client, &repo(), 9999)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(transport.seen(), [by_sha, jobs_path(4101)]);
}

// ---------------------------------------------------------------------------
// The two keys
// ---------------------------------------------------------------------------

fn warnings(raw: &str) -> Vec<(String, String)> {
    bridgewatch_core::config::parse_str(raw, std::path::Path::new("test.toml"))
        .expect("warnings, not errors")
        .warnings
        .into_iter()
        .map(|w| (w.path, w.message))
        .collect()
}

const ACCOUNTS: &str = r#"
[accounts.gl]
token = { env = "TOK" }

[accounts.gh]
provider = "github"
token = { env = "TOK" }
"#;

/// Both keys are GitHub's, and on a GitLab watch they WARN rather than refuse,
/// the way `workflow` does: a watch moved between accounts must still load.
#[test]
fn group_and_fan_out_secs_warn_on_a_gitlab_watch() {
    let said = warnings(&format!(
        r#"{ACCOUNTS}
[[watches]]
id = "on-gitlab"
account = "gl"
project = 1
group = "commit"
fan_out_secs = 60
deploy_markers = ["deploy"]
"#
    ));
    let says = |path: &str, needle: &str| said.iter().any(|(p, m)| p == path && m.contains(needle));
    assert!(says("watches.0.group", "ignored on a GitLab"), "{said:?}");
    assert!(
        says("watches.0.fan_out_secs", "ignored on a GitLab"),
        "{said:?}"
    );
}

/// `fan_out_secs` without `group = "commit"` does nothing, and says so; and a
/// commit group limited to one `workflow` can never hold more than one run.
#[test]
fn a_window_without_a_group_and_a_group_of_one_workflow_warn() {
    let said = warnings(&format!(
        r#"{ACCOUNTS}
[[watches]]
id = "runs"
account = "gh"
project = "acme-corp/monorepo"
fan_out_secs = 60
deploy_markers = ["publish"]

[[watches]]
id = "one-workflow"
account = "gh"
role = "secondary"
project = "acme-corp/monorepo"
group = "commit"
workflow = "ci.yml"
"#
    ));
    let says = |path: &str, needle: &str| said.iter().any(|(p, m)| p == path && m.contains(needle));
    assert!(
        says("watches.0.fan_out_secs", "set group = \"commit\""),
        "{said:?}"
    );
    assert!(
        says("watches.1.workflow", "every row will hold one run"),
        "{said:?}"
    );
}

/// ⛔ On a commit group `dive` selects workflow names, so the warning that says
/// "nothing to dive into" is for one-run-per-row watches only.
#[test]
fn a_commit_group_that_dives_is_not_warned_about() {
    let said = warnings(&format!(
        r#"{ACCOUNTS}
[[watches]]
id = "bw-push"
account = "gh"
project = "acme-corp/monorepo"
sources = ["push"]
group = "commit"
fan_out_secs = 120
dive = {{ bridges = "*", exclude = ["Dependabot*"] }}
deploy_markers = ["publish"]
"#
    ));
    assert!(said.is_empty(), "{said:?}");
}

/// The default: absent is `run`, the window is 90 s only when grouping, and a
/// watch that never mentioned either key is written back without them.
#[test]
fn the_keys_default_to_the_phase_two_behaviour_and_are_never_written_unasked() {
    let loaded = config("sources = [\"push\"]\ndeploy_markers = [\"publish\"]");
    let watch = &loaded.watches[0];
    assert_eq!(watch.group, GroupMode::Run);
    assert_eq!(watch.fan_out_secs, None);
    assert_eq!(watch.commit_group_window(), None);
    let rendered = toml::to_string(watch).expect("a watch serialises");
    assert!(
        !rendered.contains("group") && !rendered.contains("fan_out_secs"),
        "{rendered}"
    );

    let grouped = group_config("");
    assert_eq!(grouped.watches[0].commit_group_window(), Some(90));
    let rendered = toml::to_string(&grouped.watches[0]).expect("serialises");
    assert!(rendered.contains("group = \"commit\""), "{rendered}");
    let windowed = group_config("fan_out_secs = 15");
    assert_eq!(windowed.watches[0].commit_group_window(), Some(15));
}
