//! The GitHub client: the status table, the run-to-pipeline mapping, `Link`
//! pagination, what a 403 means, and one whole tick end to end.
//!
//! ⛔ **Every body here is hand-written, holds only the keys the client
//! decodes, and is about `acme-corp/monorepo`.** Fixtures are recorded from a
//! real repository and are a later packet's work with a privacy allow-list of
//! its own; nothing in this file has been near the GitHub API, and it must stay
//! that way, because a pasted response carries commit messages and the names
//! and email addresses of whoever wrote them.
//!
//! ⚠️ The transport double is this file's own rather than `client.rs`'s: a
//! GitHub response has to carry `Link` and `X-OAuth-Scopes`, which are exactly
//! the two things the GitLab one has no room for.

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bridgewatch_core::client::http::{HttpRequest, HttpResponse, Transport};
use bridgewatch_core::client::{CiClient, ClientError, GitHubClient, ListQuery, RequestRing};
use bridgewatch_core::config::{Account, AuthHeader, Config, ProjectRef, Provider};
use bridgewatch_core::poll::Poller;
use bridgewatch_core::status::Status;
use bridgewatch_core::token::Secret;

/// One scripted response.
#[derive(Clone, Debug)]
struct Reply {
    status: u16,
    body: String,
    link: Option<String>,
    oauth_scopes: Option<String>,
    ratelimit_remaining: Option<u64>,
    ratelimit_reset: Option<u64>,
    retry_after: Option<u64>,
}

impl Reply {
    fn ok(body: &str) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            link: None,
            oauth_scopes: None,
            ratelimit_remaining: Some(4_998),
            ratelimit_reset: Some(1_789_669_380),
            retry_after: None,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            body: r#"{"message":"scripted"}"#.to_string(),
            ..Reply::ok("")
        }
    }

    fn link(mut self, link: &str) -> Self {
        self.link = Some(link.to_string());
        self
    }

    fn scopes(mut self, scopes: &str) -> Self {
        self.oauth_scopes = Some(scopes.to_string());
        self
    }

    fn remaining(mut self, remaining: u64) -> Self {
        self.ratelimit_remaining = Some(remaining);
        self
    }

    fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs);
        self
    }

    fn reset(mut self, at: u64) -> Self {
        self.ratelimit_reset = Some(at);
        self
    }
}

/// A transport that replays scripted responses in order and records what it was
/// asked for, headers included.
///
/// ⛔ An unscripted request PANICS rather than being answered with something
/// plausible. A test whose client made one more request than it meant to would
/// otherwise pass against a default, which is how a request-count claim stops
/// being a claim.
#[derive(Debug, Default)]
struct Scripted {
    replies: Mutex<Vec<Reply>>,
    seen: Mutex<Vec<HttpRequest>>,
}

impl Scripted {
    fn new(replies: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.path.clone())
            .collect()
    }

    fn headers(&self) -> Vec<(String, String)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .flat_map(|r| r.headers.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl Transport for Scripted {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        let path = request.path.clone();
        self.seen.lock().unwrap().push(request);
        let mut replies = self.replies.lock().unwrap();
        if replies.is_empty() {
            panic!("unscripted request: {path}");
        }
        let reply = replies.remove(0);
        Ok(HttpResponse {
            status: reply.status,
            body: reply.body,
            next_page: None,
            ratelimit_remaining: reply.ratelimit_remaining,
            ratelimit_reset: reply.ratelimit_reset,
            retry_after: reply.retry_after,
            etag: Some("W/\"a-weak-validator\"".to_string()),
            link: reply.link,
            oauth_scopes: reply.oauth_scopes,
        })
    }
}

/// ⚠️ `Account::for_provider`, never `Account { provider, ..default() }`: the
/// struct-update form takes GitLab's base URL, API path and header first and
/// then overwrites only the provider, which is a GitHub account pointed at
/// gitlab.com.
fn account() -> Account {
    Account::for_provider(Provider::Github)
}

fn github_client(transport: Arc<dyn Transport>) -> (GitHubClient, RequestRing) {
    let ring = RequestRing::new(10);
    (
        GitHubClient::new(
            &account(),
            &Secret::new("ghp_SECRET"),
            transport,
            ring.clone(),
        ),
        ring,
    )
}

fn repo() -> ProjectRef {
    ProjectRef::Path("acme-corp/monorepo".into())
}

/// One workflow run, with only the keys the client decodes.
fn run_body(status: &str, conclusion: &str) -> String {
    let conclusion = if conclusion.is_empty() {
        "null".to_string()
    } else {
        format!("\"{conclusion}\"")
    };
    format!(
        r#"{{"id":4001,"run_number":42,"head_sha":"0f1e2d3c4b5a69788796a5b4c3d2e1f00fedcba9",
            "head_branch":"main","status":"{status}","conclusion":{conclusion},"event":"push",
            "html_url":"https://github.example/acme-corp/monorepo/actions/runs/4001",
            "created_at":"2026-09-18T09:00:00Z","updated_at":"2026-09-18T09:04:00Z",
            "run_started_at":"2026-09-18T09:00:30Z"}}"#
    )
}

fn runs_body(status: &str, conclusion: &str) -> String {
    format!(
        r#"{{"total_count":1,"workflow_runs":[{}]}}"#,
        run_body(status, conclusion)
    )
}

/// One job, with only the keys the client decodes.
fn job_body(id: u64, name: &str, conclusion: &str, from: &str, to: &str) -> String {
    format!(
        r#"{{"id":{id},"name":"{name}","status":"completed","conclusion":"{conclusion}",
            "started_at":"{from}","completed_at":"{to}",
            "html_url":"https://github.example/acme-corp/monorepo/actions/runs/4001/job/{id}"}}"#
    )
}

fn jobs_body(jobs: &[String]) -> String {
    format!(
        r#"{{"total_count":{},"jobs":[{}]}}"#,
        jobs.len(),
        jobs.join(",")
    )
}

// ---------------------------------------------------------------------------
// The status table
// ---------------------------------------------------------------------------

/// GitHub's `status` + `conclusion` pair, folded onto the one vocabulary the
/// verdict engine reads. This is section 2.6 of the scoping report, whole.
#[test]
fn the_two_github_fields_fold_onto_one_status() {
    let cases: &[(&str, Option<&str>, Status)] = &[
        ("queued", None, Status::Pending),
        ("in_progress", None, Status::Running),
        ("requested", None, Status::Created),
        ("pending", None, Status::Created),
        ("waiting", None, Status::Manual),
        ("completed", Some("success"), Status::Success),
        ("completed", Some("failure"), Status::Failed),
        ("completed", Some("startup_failure"), Status::Failed),
        ("completed", Some("timed_out"), Status::Failed),
        ("completed", Some("cancelled"), Status::Canceled),
        ("completed", Some("skipped"), Status::Skipped),
        ("completed", Some("stale"), Status::Skipped),
        ("completed", Some("action_required"), Status::Manual),
        (
            "completed",
            Some("neutral"),
            Status::Unknown("neutral".into()),
        ),
        ("completed", None, Status::Unknown("completed".into())),
    ];
    for (status, conclusion, expected) in cases {
        assert_eq!(
            Status::from_github(status, *conclusion),
            *expected,
            "{status}/{conclusion:?}"
        );
    }

    // ⛔ The two that decide whether a fork PR reds the tray and whether the
    // fast poll interval is held open all weekend.
    assert!(Status::from_github("completed", Some("action_required")).is_gate());
    assert!(!Status::from_github("completed", Some("action_required")).is_failed());
    assert!(Status::from_github("waiting", None).is_settled());
    assert!(!Status::from_github("waiting", None).is_live());

    // And the open vocabulary: a value GitHub adds tomorrow keeps its name and
    // draws the question mark rather than a wrong colour.
    let new = Status::from_github("completed", Some("quantum_superposition"));
    assert_eq!(new, Status::Unknown("quantum_superposition".into()));
    assert!(!new.is_live() && !new.is_failed() && !new.is_success());
    assert_eq!(
        Status::from_github("hibernating", None),
        Status::Unknown("hibernating".into()),
        "the phase is open too: neither field is enumerated on a run"
    );
}

// ---------------------------------------------------------------------------
// Decoding and the mapping
// ---------------------------------------------------------------------------

/// A run becomes a pipeline, field by field.
#[tokio::test]
async fn a_workflow_run_becomes_a_pipeline() {
    let transport = Scripted::new(vec![Reply::ok(&run_body("completed", "success"))]);
    let (client, _) = github_client(transport.clone());

    let pipeline = client.get_pipeline(&repo(), 4001).await.unwrap();

    assert_eq!(pipeline.id, 4001);
    assert_eq!(pipeline.iid, Some(42), "the #42 GitHub's own UI shows");
    assert_eq!(pipeline.sha7(), "0f1e2d3");
    assert_eq!(pipeline.ref_name, "main");
    assert_eq!(pipeline.source.as_deref(), Some("push"));
    assert_eq!(pipeline.status, Status::Success);
    assert_eq!(
        pipeline.web_url.as_deref(),
        Some("https://github.example/acme-corp/monorepo/actions/runs/4001")
    );
    assert_eq!(pipeline.created_at.as_deref(), Some("2026-09-18T09:00:00Z"));
    assert_eq!(pipeline.updated_at.as_deref(), Some("2026-09-18T09:04:00Z"));
    assert_eq!(
        pipeline.started_at.as_deref(),
        Some("2026-09-18T09:00:30Z"),
        "run_started_at, which is not created_at for a run that queued"
    );
    assert_eq!(
        pipeline.finished_at, None,
        "GitHub reports no finish time for a run, and updated_at is not the same claim"
    );
    assert_eq!(
        pipeline.project_id, None,
        "the run's repository object is not decoded, and nothing needs it"
    );
    assert_eq!(
        transport.paths(),
        ["/repos/acme-corp/monorepo/actions/runs/4001"]
    );
}

/// Jobs decode, and the only structure a GitHub run has, a called workflow's
/// jobs, flattened into the caller's list, becomes the stage.
#[tokio::test]
async fn jobs_decode_and_a_called_workflows_jobs_carry_their_caller_as_the_stage() {
    let body = jobs_body(&[
        job_body(
            9001,
            "build",
            "success",
            "2026-09-18T09:00:30Z",
            "2026-09-18T09:01:00Z",
        ),
        job_body(
            9002,
            "release / publish",
            "success",
            "2026-09-18T09:01:00Z",
            "2026-09-18T09:02:00Z",
        ),
        // Ten levels of nesting are allowed, so a name can carry several
        // separators; the top-level caller is the useful grouping.
        job_body(
            9003,
            "release / sign / notarize",
            "skipped",
            "2026-09-18T09:02:00Z",
            "2026-09-18T09:02:00Z",
        ),
    ]);
    let transport = Scripted::new(vec![Reply::ok(&body)]);
    let (client, _) = github_client(transport.clone());

    let jobs = client.pipeline_jobs(&repo(), 4001).await.unwrap();

    assert_eq!(jobs.len(), 3);
    assert_eq!(jobs[0].name, "build");
    assert_eq!(jobs[0].stage, None, "a plain job belongs to no group");
    assert_eq!(jobs[0].status, Status::Success);
    assert_eq!(
        jobs[1].name, "release / publish",
        "the name is kept verbatim: it is what deploy_markers match"
    );
    assert_eq!(jobs[1].stage.as_deref(), Some("release"));
    assert_eq!(
        jobs[2].stage.as_deref(),
        Some("release"),
        "split on the FIRST separator"
    );
    assert_eq!(jobs[2].status, Status::Skipped);
    assert!(
        jobs.iter().all(|j| !j.allow_failure),
        "continue-on-error appears nowhere in the jobs API, so nothing can set this"
    );
    assert!(
        jobs.iter().all(|j| j.duration.is_none()),
        "GitHub sends no duration; the view derives one from the two timestamps"
    );
    assert_eq!(jobs[0].finished_at.as_deref(), Some("2026-09-18T09:01:00Z"));
    assert!(
        transport.paths()[0].contains("filter=latest"),
        "pinned rather than relied on: a re-run bumps run_attempt on the SAME id, \
         and filter=all would list every superseded attempt: {:?}",
        transport.paths()
    );
}

/// A job that never ran carries no `steps` key at all, which is a different
/// fact from an empty list and is why the field is an `Option`.
#[tokio::test]
async fn steps_decode_when_they_are_there_and_are_absent_when_they_are_not() {
    let body = r#"{"total_count":2,"jobs":[
        {"id":9101,"name":"build","status":"completed","conclusion":"failure",
         "steps":[{"name":"Set up job","number":1,"status":"completed","conclusion":"success"},
                  {"name":"cargo test","number":2,"status":"completed","conclusion":"failure"}]},
        {"id":9102,"name":"publish","status":"completed","conclusion":"skipped"}]}"#;
    let decoded: bridgewatch_core::client::wire::github::JobsResponse =
        serde_json::from_str(body).expect("the wrapper object decodes");

    let steps = decoded.jobs[0].steps.as_ref().expect("this job ran");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].name, "cargo test");
    assert_eq!(steps[1].status(), Status::Failed);
    assert_eq!(steps[0].status(), Status::Success);
    assert!(
        decoded.jobs[1].steps.is_none(),
        "absent, not empty: a job that never ran has no steps key"
    );
}

// ---------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------

/// The list query becomes GitHub's own filters, and a watch that named a
/// workflow asks that workflow's endpoint.
#[tokio::test]
async fn the_list_query_becomes_githubs_filters() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body("completed", "success")),
        Reply::ok(&runs_body("completed", "success")),
    ]);
    let (client, _) = github_client(transport.clone());

    client
        .list_pipelines(&repo(), &ListQuery::exact("main", Some("push".into()), 20))
        .await
        .unwrap();
    client
        .list_pipelines(
            &repo(),
            &ListQuery::exact("main", Some("push".into()), 20).for_workflow(Some("ci.yml")),
        )
        .await
        .unwrap();

    let paths = transport.paths();
    assert!(
        paths[0].starts_with("/repos/acme-corp/monorepo/actions/runs?"),
        "{paths:?}"
    );
    for expected in [
        "branch=main",
        "event=push",
        "exclude_pull_requests=true",
        "per_page=20",
        "page=1",
    ] {
        assert!(
            paths[0].contains(expected),
            "{paths:?} is missing {expected}"
        );
    }
    assert!(
        paths[1].starts_with("/repos/acme-corp/monorepo/actions/workflows/ci.yml/runs?"),
        "a named workflow asks that workflow's own endpoint: {paths:?}"
    );
    assert!(paths[1].contains("branch=main"), "{paths:?}");
}

/// ⛔ The slash in `owner/repo` survives, which is the opposite of GitLab's
/// percent-encoded `group%2Fproject`. Encoding it gives a 404 that reads like a
/// missing repository.
#[test]
fn a_repository_path_keeps_its_slash_and_a_gitlab_path_does_not() {
    let project = ProjectRef::Path("acme-corp/monorepo".into());
    assert_eq!(
        project.url_segment_for(Provider::Github),
        "acme-corp/monorepo"
    );
    assert_eq!(
        project.url_segment_for(Provider::Gitlab),
        "acme-corp%2Fmonorepo"
    );
    assert_eq!(
        project.url_segment(),
        "acme-corp%2Fmonorepo",
        "the unqualified spelling is still GitLab's, so no existing call site moved"
    );
    assert_eq!(
        ProjectRef::Path("acme corp/mono repo".into()).url_segment_for(Provider::Github),
        "acme%20corp/mono%20repo",
        "each segment is still encoded on its own, so nothing breaks out of one"
    );
}

/// ⛔ A numeric project is refused before anything is sent. There is no
/// `/repos/<id>` endpoint, and the value is nearly always a GitLab project id
/// left behind by moving a watch between accounts.
#[tokio::test]
async fn a_numeric_project_is_refused_without_a_request() {
    let transport = Scripted::new(vec![]);
    let (client, _) = github_client(transport.clone());

    let error = client
        .get_pipeline(&ProjectRef::Id(82468124), 1)
        .await
        .expect_err("GitHub cannot address a repository by id");

    assert!(
        matches!(error, ClientError::Unsupported { .. }),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("owner/repo"),
        "the message says what to write: {error}"
    );
    assert!(error.is_fatal(), "no amount of retrying invents the URL");
    assert!(
        transport.paths().is_empty(),
        "and nothing was sent anywhere"
    );
}

/// The headers GitHub requires, on every request.
///
/// ⚠️ The user agent is NOT among them: `ReqwestTransport` sets
/// `bridgewatch/<version>` on the client itself, which is asserted against a
/// real socket in `client.rs`. Adding a second one here would put two
/// `User-Agent` headers on the wire.
#[tokio::test]
async fn every_request_carries_the_accept_and_api_version_headers() {
    let transport = Scripted::new(vec![Reply::ok(&runs_body("completed", "success"))]);
    let account = Account {
        header: AuthHeader::AuthorizationBearer,
        ..Account::for_provider(Provider::Github)
    };
    let client = GitHubClient::new(
        &account,
        &Secret::new("ghp_SECRET"),
        transport.clone(),
        RequestRing::new(4),
    );

    client
        .list_pipelines(&repo(), &ListQuery::exact("main", None, 5))
        .await
        .unwrap();

    let headers = transport.headers();
    assert!(
        headers.contains(&(
            "Accept".to_string(),
            "application/vnd.github+json".to_string()
        )),
        "{headers:?}"
    );
    assert!(
        headers.contains(&("X-GitHub-Api-Version".to_string(), "2022-11-28".to_string())),
        "the version is pinned, because unpinned means whatever is current: {headers:?}"
    );
    assert!(
        headers.contains(&("Authorization".to_string(), "Bearer ghp_SECRET".to_string())),
        "{headers:?}"
    );
}

/// The credential does not reach a `{:?}` or the request ring.
///
/// ⚠️ The DEBUG LINE is held in `logging.rs` rather than here, and the reason is
/// worth knowing before moving it back: `tracing` caches a callsite's interest
/// globally, so a test that runs under a scoped subscriber can be raced by a
/// sibling test in the same binary that hits the same callsite with no
/// subscriber at all, and the assertion then reads an EMPTY log. Every test in
/// `logging.rs` runs under one. Measured: this test failed roughly one run in
/// two here, and passes under `--test-threads=1`.
#[test]
fn the_github_client_never_prints_its_credential() {
    let (client, _) = github_client(Scripted::new(vec![]));
    let rendered = format!("{client:?} {client:#?}");
    assert!(!rendered.contains("ghp_SECRET"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");

    let runtime = support::runtime();
    let transport = Scripted::new(vec![Reply::ok(&runs_body("completed", "success"))]);
    let (client, ring) = github_client(transport);
    runtime
        .block_on(client.list_pipelines(&repo(), &ListQuery::exact("main", None, 5)))
        .expect("the scripted page");

    assert_eq!(ring.len(), 1, "the request is in the ring");
    let serialised = serde_json::to_string(&ring.entries()).unwrap();
    assert!(
        !serialised.contains("ghp_SECRET"),
        "the ring is shown in a debug pane and pasted into issues: {serialised}"
    );
    assert!(
        serialised.contains("/repos/acme-corp/monorepo/actions/runs"),
        "and the path is what makes an entry worth keeping: {serialised}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// ⛔ The next page is FOLLOWED, not rebuilt. GitHub rewrites
/// `/repos/{owner}/{repo}/` to `/repositories/{id}/` in its `Link` URLs, and
/// the relations arrive in a different ORDER on each page, so this is parsed by
/// `rel=` and never by position.
#[tokio::test]
async fn jobs_paginate_by_following_the_link_header() {
    let page1 = jobs_body(&[job_body(
        9001,
        "build",
        "success",
        "2026-09-18T09:00:30Z",
        "2026-09-18T09:01:00Z",
    )]);
    let page2 = jobs_body(&[job_body(
        9002,
        "publish",
        "failure",
        "2026-09-18T09:01:00Z",
        "2026-09-18T09:02:00Z",
    )]);
    let transport = Scripted::new(vec![
        Reply::ok(&page1).link(
            "<https://api.github.com/repositories/1374862400/actions/runs/4001/jobs?filter=latest&per_page=100&page=2>; rel=\"next\", \
             <https://api.github.com/repositories/1374862400/actions/runs/4001/jobs?filter=latest&per_page=100&page=2>; rel=\"last\"",
        ),
        // Page 2 answers `prev, next, last, first` on the live API even when
        // there is nothing after it; here it simply has no next.
        Reply::ok(&page2).link(
            "<https://api.github.com/repositories/1374862400/actions/runs/4001/jobs?page=1>; rel=\"prev\", \
             <https://api.github.com/repositories/1374862400/actions/runs/4001/jobs?page=1>; rel=\"first\"",
        ),
    ]);
    let (client, _) = github_client(transport.clone());

    let jobs = client.pipeline_jobs(&repo(), 4001).await.unwrap();

    assert_eq!(jobs.len(), 2, "both pages");
    assert_eq!(jobs[1].name, "publish");
    let paths = transport.paths();
    assert_eq!(paths.len(), 2);
    assert_eq!(
        paths[1],
        "/repositories/1374862400/actions/runs/4001/jobs?filter=latest&per_page=100&page=2",
        "the rewritten URL is used exactly as it was given: {paths:?}"
    );
}

/// ⛔ A `Link` naming another host is REFUSED, because the request built from it
/// would carry the token there. Refusing loudly rather than stopping quietly is
/// the point: a silent stop produces a short job list that looks like a short
/// run.
#[tokio::test]
async fn a_next_page_on_another_host_is_refused_rather_than_followed() {
    let transport = Scripted::new(vec![
        Reply::ok(&jobs_body(&[job_body(
            9001,
            "build",
            "success",
            "2026-09-18T09:00:30Z",
            "2026-09-18T09:01:00Z",
        )]))
        .link("<https://evil.example/repositories/1/jobs?page=2>; rel=\"next\""),
    ]);
    let (client, _) = github_client(transport.clone());

    let error = client
        .pipeline_jobs(&repo(), 4001)
        .await
        .expect_err("the token must not travel");

    assert!(
        matches!(error, ClientError::Unsupported { .. }),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("evil.example"),
        "the message names where it was being sent: {error}"
    );
    assert_eq!(
        transport.paths().len(),
        1,
        "and the second request was never made"
    );
}

// ---------------------------------------------------------------------------
// What a 403 means
// ---------------------------------------------------------------------------

/// ⛔ The inherited defect, fixed: an exhausted GitHub rate limit arrives as a
/// **403**, and GitLab's classifier reads a 403 as `Auth`, which `is_fatal()`.
/// That parks the account with "check the token's scope" through a limit that
/// would have cleared on its own. The signals decide instead.
#[tokio::test]
async fn a_403_is_a_rate_limit_when_the_headers_say_so_and_auth_when_they_do_not() {
    // The primary hourly budget: 403, remaining 0, and a reset rather than a
    // retry-after.
    let transport = Scripted::new(vec![Reply::status(403).remaining(0).reset(1_789_672_980)]);
    let (client, _) = github_client(transport);
    let error = client.get_pipeline(&repo(), 4001).await.unwrap_err();
    let ClientError::RateLimited { retry_after, reset } = &error else {
        panic!("an exhausted primary limit is not an auth failure: {error:?}");
    };
    assert_eq!(*retry_after, None, "GitHub sends none for a primary limit");
    assert_eq!(*reset, Some(1_789_672_980));
    assert!(!error.is_fatal(), "a limit clears on its own");
    assert!(error.should_back_off());

    // A secondary limit: 403 with a retry-after and a budget that is not spent.
    let transport = Scripted::new(vec![Reply::status(403).remaining(3_912).retry_after(60)]);
    let (client, _) = github_client(transport);
    let error = client.get_pipeline(&repo(), 4001).await.unwrap_err();
    assert!(
        matches!(
            error,
            ClientError::RateLimited {
                retry_after: Some(60),
                ..
            }
        ),
        "{error:?}"
    );

    // A 429, which GitHub also uses for the primary limit.
    let transport = Scripted::new(vec![Reply::status(429).remaining(0)]);
    let (client, _) = github_client(transport);
    assert!(matches!(
        client.get_pipeline(&repo(), 4001).await.unwrap_err(),
        ClientError::RateLimited { .. }
    ));

    // ⚠️ And a 403 with NEITHER signal really is a permission problem: a token
    // that cannot see this repository. It stays fatal.
    let transport = Scripted::new(vec![Reply::status(403).remaining(4_900)]);
    let (client, _) = github_client(transport);
    let error = client.get_pipeline(&repo(), 4001).await.unwrap_err();
    assert!(
        matches!(error, ClientError::Auth { status: 403 }),
        "{error:?}"
    );
    assert!(error.is_fatal());

    // As does a 401, whatever the headers say.
    let transport = Scripted::new(vec![Reply::status(401).remaining(0)]);
    let (client, _) = github_client(transport);
    let error = client.get_pipeline(&repo(), 4001).await.unwrap_err();
    assert!(
        matches!(error, ClientError::Auth { status: 401 }),
        "{error:?}"
    );
}

/// The backoff has to come from `x-ratelimit-reset` when there is no
/// `retry-after`, because GitHub's primary window is an HOUR: doubling a
/// five-second interval against it spends the whole hour being refused.
#[test]
fn a_rate_limit_reset_becomes_the_retry_after_when_no_header_was_sent() {
    use bridgewatch_core::config::{PollConfig, RateLimitBackoff};
    use bridgewatch_core::poll::PollPolicy;

    let in_ten_minutes = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let mut policy = PollPolicy::new(
        &PollConfig {
            live_secs: 30,
            idle_secs: 120,
        },
        &RateLimitBackoff { max_secs: 900 },
    );
    policy.on_error(&ClientError::RateLimited {
        retry_after: None,
        reset: Some(in_ten_minutes),
    });

    let interval = policy.interval(true);
    assert!(
        interval >= std::time::Duration::from_secs(595)
            && interval <= std::time::Duration::from_secs(600),
        "about ten minutes, not the 60s the doubling would have chosen: {interval:?}"
    );

    // A reset already in the past is not an instruction to wait; the ordinary
    // doubling takes over.
    let mut policy = PollPolicy::new(
        &PollConfig {
            live_secs: 30,
            idle_secs: 120,
        },
        &RateLimitBackoff { max_secs: 900 },
    );
    policy.on_error(&ClientError::RateLimited {
        retry_after: None,
        reset: Some(1),
    });
    assert_eq!(policy.interval(true), std::time::Duration::from_secs(60));

    // ⚠️ And a GitLab 429 is untouched: its classifier leaves `reset` empty, so
    // nothing about an existing account's backoff moved.
    let mut policy = PollPolicy::new(
        &PollConfig {
            live_secs: 5,
            idle_secs: 60,
        },
        &RateLimitBackoff { max_secs: 300 },
    );
    policy.on_error(&ClientError::RateLimited {
        retry_after: Some(120),
        reset: None,
    });
    assert_eq!(policy.interval(true), std::time::Duration::from_secs(120));
}

// ---------------------------------------------------------------------------
// The wizard's four calls
// ---------------------------------------------------------------------------

/// `GET /user` answers who the token is.
#[tokio::test]
async fn the_current_user_is_read_from_the_user_endpoint() {
    let transport = Scripted::new(vec![Reply::ok(
        r#"{"id":7,"login":"acme-bot","name":"Acme Release Bot","type":"Bot"}"#,
    )]);
    let (client, _) = github_client(transport.clone());

    let user = client.current_user().await.unwrap();
    assert_eq!(user.id, 7);
    assert_eq!(user.username, "acme-bot");
    assert_eq!(user.name.as_deref(), Some("Acme Release Bot"));
    assert!(user.bot, "an App installation token authenticates as a Bot");
    assert_eq!(transport.paths(), ["/user"]);

    let transport = Scripted::new(vec![Reply::ok(
        r#"{"id":8,"login":"a-person","name":null,"type":"User"}"#,
    )]);
    let (client, _) = github_client(transport);
    let user = client.current_user().await.unwrap();
    assert!(!user.bot);
    assert_eq!(user.name, None, "a display name nobody set is absent");
}

/// ⛔ GitHub has no token-introspection endpoint. What a **classic** token has
/// is a response header; a fine-grained one has nothing at all, and saying so
/// is the honest answer rather than reporting an empty scope list as fact.
#[tokio::test]
async fn token_self_reports_a_classic_tokens_scopes_and_refuses_to_guess_otherwise() {
    let transport = Scripted::new(vec![
        Reply::ok(r#"{"id":7,"login":"a-person","type":"User"}"#).scopes("repo, read:org"),
    ]);
    let (client, _) = github_client(transport);
    let info = client.token_self().await.expect("a classic token says");
    assert_eq!(info.scopes, ["repo", "read:org"]);
    assert_eq!(info.id, 7, "there is no token id; the user's stands in");
    assert_eq!(info.name, None, "and no token name exists to report");
    assert_eq!(info.expires_at, None);

    // ⚠️ An EMPTY header is a classic token that was granted no scopes, which
    // is a real answer and not the same as no header.
    let transport = Scripted::new(vec![
        Reply::ok(r#"{"id":7,"login":"a-person","type":"User"}"#).scopes(""),
    ]);
    let (client, _) = github_client(transport);
    let info = client.token_self().await.expect("still an answer");
    assert!(info.scopes.is_empty());

    // No header: a fine-grained token or an App installation. The wizard reads
    // the failure as "kind unknown", exactly as it does for a GitLab instance
    // with no /personal_access_tokens/self.
    let transport = Scripted::new(vec![Reply::ok(
        r#"{"id":7,"login":"a-person","type":"User"}"#,
    )]);
    let (client, _) = github_client(transport);
    let error = client.token_self().await.expect_err("nothing to report");
    assert!(
        error.to_string().contains("X-OAuth-Scopes"),
        "the message says why rather than looking like an outage: {error}"
    );
}

/// The project picker lists repositories, newest push first, and its search
/// filters what it fetched.
#[tokio::test]
async fn repositories_are_listed_newest_push_first_and_searched_client_side() {
    let page = r#"[{"id":1,"full_name":"acme-corp/monorepo","name":"monorepo",
                    "default_branch":"main","html_url":"https://github.example/acme-corp/monorepo"},
                   {"id":2,"full_name":"acme-corp/website","name":"website",
                    "default_branch":"trunk","html_url":"https://github.example/acme-corp/website"}]"#;
    let transport = Scripted::new(vec![Reply::ok(page)]);
    let (client, _) = github_client(transport.clone());

    let (projects, truncated) = client.list_projects(None, 3).await.unwrap();
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].path_with_namespace, "acme-corp/monorepo");
    assert_eq!(projects[0].default_branch.as_deref(), Some("main"));
    assert!(!truncated, "one short page is the whole listing");
    assert_eq!(
        transport.paths(),
        ["/user/repos?sort=pushed&direction=desc&per_page=100&page=1"]
    );

    let transport = Scripted::new(vec![Reply::ok(page)]);
    let (client, _) = github_client(transport);
    let (projects, _) = client.list_projects(Some("WEB"), 3).await.unwrap();
    assert_eq!(
        projects.iter().map(|p| p.id).collect::<Vec<_>>(),
        [2],
        "the search is case-insensitive over owner/repo"
    );
}

/// A cut-short listing says so, which is what makes the picker offer a search
/// box rather than pretend it showed everything.
#[tokio::test]
async fn a_listing_stopped_at_the_page_cap_reports_itself_truncated() {
    let one = r#"[{"id":1,"full_name":"acme-corp/monorepo","name":"monorepo"}]"#;
    let next = "<https://api.github.com/user/repos?sort=pushed&per_page=100&page=2>; rel=\"next\"";
    let transport = Scripted::new(vec![Reply::ok(one).link(next), Reply::ok(one).link(next)]);
    let (client, _) = github_client(transport.clone());

    let (projects, truncated) = client.list_projects(None, 2).await.unwrap();
    assert_eq!(projects.len(), 2);
    assert!(truncated, "a next page existed and was not fetched");
    assert_eq!(transport.paths().len(), 2, "the cap held");
}

/// One repository, by path.
#[tokio::test]
async fn one_repository_resolves_by_owner_and_name() {
    let transport = Scripted::new(vec![Reply::ok(
        r#"{"id":1,"full_name":"acme-corp/monorepo","name":"monorepo","default_branch":"main",
            "html_url":"https://github.example/acme-corp/monorepo"}"#,
    )]);
    let (client, _) = github_client(transport.clone());

    let project = client.project(&repo()).await.unwrap();
    assert_eq!(project.id, 1);
    assert_eq!(project.path_with_namespace, "acme-corp/monorepo");
    assert_eq!(transport.paths(), ["/repos/acme-corp/monorepo"]);
}

/// ⛔ Bridges and child jobs answer empty and send NOTHING. A workflow run has
/// no trigger jobs: its own jobs are the whole picture, a called workflow's
/// included. This is a real answer rather than a gap, and it is what keeps a
/// GitHub tick at one request per pipeline plus the list.
#[tokio::test]
async fn bridges_and_child_jobs_are_empty_without_a_request() {
    let transport = Scripted::new(vec![]);
    let (client, _) = github_client(transport.clone());

    assert!(
        CiClient::pipeline_bridges(&client, &repo(), 4001)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        CiClient::child_jobs(&client, &repo(), 4002)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(transport.paths().is_empty(), "and nothing was sent");
}

// ---------------------------------------------------------------------------
// One whole tick
// ---------------------------------------------------------------------------

fn github_config(extra: &str) -> Config {
    let raw = format!(
        r#"
[accounts.gh]
provider = "github"
token = {{ env = "TOK" }}

[[watches]]
id = "bw-ci"
account = "gh"
project = "acme-corp/monorepo"
ref = "main"
sources = ["push"]
workflow = "ci.yml"
deploy_markers = ["publish"]
{extra}
"#
    );
    bridgewatch_core::config::parse_str(&raw, std::path::Path::new("test.toml"))
        .expect("a GitHub configuration loads")
        .config
}

fn github_poller(config: &Config, transport: Arc<Scripted>) -> Poller {
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

/// ⛔ The end-to-end claim: a GitHub watch produces a row that is the run, jobs
/// that are the run's jobs, and a verdict that follows `deploy_markers` exactly
/// as a GitLab pipeline's would. Nothing in the verdict engine knows which
/// provider it is reading.
#[tokio::test]
async fn a_github_watch_produces_a_row_whose_verdict_follows_the_deploy_markers() {
    let jobs = jobs_body(&[
        job_body(
            9001,
            "build",
            "success",
            "2026-09-18T09:00:30Z",
            "2026-09-18T09:01:00Z",
        ),
        job_body(
            9002,
            "publish",
            "success",
            "2026-09-18T09:01:00Z",
            "2026-09-18T09:02:30Z",
        ),
    ]);
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body("completed", "success")),
        Reply::ok(&jobs),
    ]);
    let config = github_config("");
    let mut poller = github_poller(&config, transport.clone());

    let snapshot = poller.tick().await.snapshot;

    assert_eq!(snapshot.icon_state.as_str(), "deployed");
    let watch = &snapshot.watches[0];
    let row = &watch.rows[0];
    assert_eq!(row.id, 4001);
    assert_eq!(row.sha7, "0f1e2d3");
    assert_eq!(row.ref_name, "main");
    assert_eq!(row.source.as_deref(), Some("push"));
    assert_eq!(row.status, "success");
    assert_eq!(
        row.deploy, "live",
        "the deploy engine's word for a marker that succeeded: it is out"
    );
    assert_eq!(
        row.deploy_marker.as_ref().map(|m| m.name.as_str()),
        Some("publish")
    );
    assert!(row.failures.is_empty());
    assert_eq!(
        row.parent_jobs
            .iter()
            .map(|j| j.name.as_str())
            .collect::<Vec<_>>(),
        ["build", "publish"],
        "the row's jobs are the run's jobs"
    );
    assert_eq!(
        row.parent_jobs[1].duration,
        Some(90.0),
        "derived from the two GitHub timestamps, exactly as a recorded GitLab \
         fixture's is"
    );
    assert!(row.bridges.is_empty(), "a run has no trigger jobs");

    // ⚠️ Two requests, not three: the bridges call costs nothing on GitHub.
    let paths = transport.paths();
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(
        paths[0].starts_with("/repos/acme-corp/monorepo/actions/workflows/ci.yml/runs?"),
        "the watch's workflow reached the client through the list query: {paths:?}"
    );
    assert_eq!(
        paths[1],
        "/repos/acme-corp/monorepo/actions/runs/4001/jobs?filter=latest&per_page=100"
    );
}

/// The same watch, with the marker job failed: `failed`, and the failing job is
/// named. The marker is what decides, not the run's own conclusion.
#[tokio::test]
async fn a_failed_deploy_marker_reds_a_github_watch() {
    let jobs = jobs_body(&[
        job_body(
            9001,
            "build",
            "success",
            "2026-09-18T09:00:30Z",
            "2026-09-18T09:01:00Z",
        ),
        job_body(
            9002,
            "publish",
            "failure",
            "2026-09-18T09:01:00Z",
            "2026-09-18T09:02:30Z",
        ),
    ]);
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body("completed", "failure")),
        Reply::ok(&jobs),
    ]);
    let config = github_config("");
    let mut poller = github_poller(&config, transport);

    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];

    assert_eq!(snapshot.icon_state.as_str(), "failed");
    assert_eq!(row.deploy, "failed");
    assert_eq!(row.failures, ["publish"]);
}

/// ⛔ A fork pull request from a first-time contributor: `completed` with
/// `conclusion: action_required` and **zero jobs**, waiting for a maintainer to
/// press Approve. It must read as parked, never as failed and never as green.
///
/// ⚠️ **Hand-built from the documented shape.** Neither research lane could
/// capture one (it needs a fork PR or a first-time contributor), so the pair of
/// fields and the empty job list are taken from GitHub's documentation and from
/// the histogram the scoping report measured, where it was 13% of all runs.
#[tokio::test]
async fn a_run_awaiting_a_maintainers_approval_is_parked_rather_than_failed_or_green() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body("completed", "action_required")),
        Reply::ok(r#"{"total_count":0,"jobs":[]}"#),
    ]);
    let config = github_config("");
    let mut poller = github_poller(&config, transport);

    let snapshot = poller.tick().await.snapshot;
    let row = &snapshot.watches[0].rows[0];

    assert_eq!(
        snapshot.icon_state.as_str(),
        "parked_gate",
        "not `failed`, which would red the tray on one run in eight, and not \
         `succeeded_no_deploy`, which would call an unapproved run green"
    );
    assert_eq!(row.status, "manual");
    assert!(row.parent_jobs.is_empty(), "the run has no jobs at all");
    assert!(row.failures.is_empty());
    assert!(!row.live, "nobody is working on it until a human acts");
}
