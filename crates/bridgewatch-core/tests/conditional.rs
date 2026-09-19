//! Conditional requests: a stored `ETag` goes back as `If-None-Match`, and a
//! `304` is answered with the body it confirms.
//!
//! ⛔ Nothing here has been near the GitHub or GitLab API. Every body is
//! hand-written about `acme-corp/monorepo`, every validator is invented, and
//! the two tests that need real HTTP talk to a socket on loopback.
//!
//! The transport double is this file's own: it has to script a 304 and an
//! `ETag` per reply, which neither `tests/github.rs`'s nor `tests/client.rs`'s
//! has room for.

use std::collections::BTreeMap;
use std::io::{BufRead as _, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use bridgewatch_core::client::http::{HttpRequest, HttpResponse, Transport};
use bridgewatch_core::client::{
    CiClient, ClientError, ConditionalTransport, ListQuery, RequestRing, ReqwestTransport,
    client_for,
};
use bridgewatch_core::config::{Account, Config, ProjectRef, Provider};
use bridgewatch_core::poll::Poller;
use bridgewatch_core::token::Secret;

/// One scripted response.
#[derive(Clone, Debug)]
struct Reply {
    status: u16,
    body: String,
    etag: Option<String>,
    link: Option<String>,
    oauth_scopes: Option<String>,
    ratelimit_remaining: Option<u64>,
    ratelimit_reset: Option<u64>,
    retry_after: Option<u64>,
}

impl Reply {
    fn ok(body: &str, etag: &str) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            etag: Some(etag.to_string()),
            link: None,
            oauth_scopes: None,
            ratelimit_remaining: Some(4_900),
            ratelimit_reset: Some(1_789_669_380),
            retry_after: None,
        }
    }

    /// A 200 that carries no validator at all.
    fn ok_untagged(body: &str) -> Self {
        Self {
            etag: None,
            ..Reply::ok(body, "")
        }
    }

    /// What GitHub sends for an unchanged resource: no body, no `Link`, the
    /// validator again, and the rate-limit headers as they stand.
    fn not_modified(etag: &str) -> Self {
        Self {
            status: 304,
            body: String::new(),
            ..Reply::ok("", etag)
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            body: r#"{"message":"scripted"}"#.to_string(),
            etag: None,
            ..Reply::ok("", "")
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

    fn reset(mut self, at: u64) -> Self {
        self.ratelimit_reset = Some(at);
        self
    }
}

/// Replays scripted responses in order and records every request, headers
/// included. An unscripted request PANICS, so a request-count claim is a claim.
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

    /// The `If-None-Match` each request carried, in order.
    fn validators(&self) -> Vec<Option<String>> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                r.headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("if-none-match"))
                    .map(|(_, value)| value.clone())
            })
            .collect()
    }

    fn unused(&self) -> usize {
        self.replies.lock().unwrap().len()
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
            etag: reply.etag,
            link: reply.link,
            oauth_scopes: reply.oauth_scopes,
        })
    }
}

/// A GitHub client exactly as the poller builds one: through the factory.
fn github(transport: Arc<dyn Transport>) -> (Arc<dyn CiClient>, RequestRing) {
    let ring = RequestRing::new(50);
    let client = client_for(
        &Account::for_provider(Provider::Github),
        &Secret::new("ghp_SECRET"),
        transport,
        ring.clone(),
    )
    .expect("the factory builds a GitHub client");
    (client, ring)
}

fn repo() -> ProjectRef {
    ProjectRef::Path("acme-corp/monorepo".into())
}

fn run_body(id: u64, status: &str, conclusion: &str) -> String {
    format!(
        r#"{{"id":{id},"run_number":42,"head_sha":"0f1e2d3c4b5a69788796a5b4c3d2e1f00fedcba9",
            "head_branch":"main","status":"{status}","conclusion":"{conclusion}","event":"push",
            "html_url":"https://github.example/acme-corp/monorepo/actions/runs/{id}",
            "created_at":"2026-09-18T09:00:00Z","updated_at":"2026-09-18T09:04:00Z",
            "run_started_at":"2026-09-18T09:00:30Z"}}"#
    )
}

fn runs_body(id: u64, status: &str, conclusion: &str) -> String {
    format!(
        r#"{{"total_count":1,"workflow_runs":[{}]}}"#,
        run_body(id, status, conclusion)
    )
}

fn jobs_body(ids: &[u64]) -> String {
    let jobs: Vec<String> = ids
        .iter()
        .map(|id| {
            format!(
                r#"{{"id":{id},"name":"job-{id}","status":"completed","conclusion":"success",
                    "started_at":"2026-09-18T09:00:30Z","completed_at":"2026-09-18T09:01:00Z",
                    "html_url":"https://github.example/acme-corp/monorepo/actions/runs/4001/job/{id}"}}"#
            )
        })
        .collect();
    format!(
        r#"{{"total_count":{},"jobs":[{}]}}"#,
        ids.len(),
        jobs.join(",")
    )
}

async fn list(client: &dyn CiClient) -> Result<Vec<u64>, ClientError> {
    let rows = client
        .list_pipelines(&repo(), &ListQuery::exact("main", None, 5))
        .await?;
    Ok(rows.into_iter().map(|p| p.id).collect())
}

fn job_ids(jobs: &[bridgewatch_core::model::Job]) -> Vec<u64> {
    jobs.iter().map(|j| j.id).collect()
}

fn statuses(ring: &RequestRing) -> Vec<Option<u16>> {
    ring.entries().iter().map(|e| e.status).collect()
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// A 200 stores the body; the next request for the same URL sends the
/// validator, and a 304 is answered with the stored body as if it were fresh.
#[tokio::test]
async fn a_304_after_a_200_serves_the_stored_body() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body(4001, "completed", "success"), "\"strong-1\""),
        Reply::not_modified("\"strong-1\""),
    ]);
    let (client, ring) = github(transport.clone());

    let first = list(client.as_ref()).await.expect("the 200");
    let second = list(client.as_ref()).await.expect("the 304, served");

    assert_eq!(first, [4001]);
    assert_eq!(second, first, "the 304 read exactly as the 200 did");
    assert_eq!(
        transport.paths()[0],
        transport.paths()[1],
        "the same URL twice"
    );
    assert_eq!(
        transport.validators(),
        [None, Some("\"strong-1\"".to_string())],
        "the first request had nothing to send; the second sent the stored validator"
    );
    assert_eq!(statuses(&ring), [Some(200), Some(304)]);
}

/// ⛔ The validator goes back byte for byte, quotes and all, and a weak one
/// keeps its `W/`. Both forms were measured on the same URL (weak without a
/// token, strong with one), and a server that does not recognise what it is
/// sent answers 200, which costs budget and looks like nothing at all.
#[tokio::test]
async fn weak_and_strong_validators_are_echoed_exactly() {
    for etag in [
        "W/\"686897696a7c876b7e\"",
        "\"686897696a7c876b7e\"",
        "W/\"with spaces, and a comma\"",
    ] {
        let transport = Scripted::new(vec![
            Reply::ok(&runs_body(4001, "completed", "success"), etag),
            Reply::not_modified(etag),
        ]);
        let (client, _) = github(transport.clone());
        list(client.as_ref()).await.unwrap();
        list(client.as_ref()).await.unwrap();
        assert_eq!(
            transport.validators()[1].as_deref(),
            Some(etag),
            "sent back unaltered"
        );
    }
}

/// A 200 with a new validator replaces the stored entry, body and all, so the
/// next 304 serves the NEW body.
#[tokio::test]
async fn a_changed_etag_replaces_the_stored_entry() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body(4001, "completed", "success"), "\"v1\""),
        Reply::ok(&runs_body(4002, "in_progress", ""), "\"v2\""),
        Reply::not_modified("\"v2\""),
    ]);
    let (client, _) = github(transport.clone());

    assert_eq!(list(client.as_ref()).await.unwrap(), [4001]);
    assert_eq!(list(client.as_ref()).await.unwrap(), [4002]);
    assert_eq!(
        list(client.as_ref()).await.unwrap(),
        [4002],
        "the 304 confirmed the second body, not the first"
    );
    assert_eq!(
        transport.validators(),
        [None, Some("\"v1\"".into()), Some("\"v2\"".into())]
    );
}

/// A 200 that carries no validator retires the stored one: sending it again
/// would ask the server to confirm a body that is no longer the current one.
#[tokio::test]
async fn a_200_without_an_etag_forgets_the_stored_one() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body(4001, "completed", "success"), "\"v1\""),
        Reply::ok_untagged(&runs_body(4002, "completed", "success")),
        Reply::ok_untagged(&runs_body(4002, "completed", "success")),
    ]);
    let (client, _) = github(transport.clone());
    for _ in 0..3 {
        list(client.as_ref()).await.unwrap();
    }
    assert_eq!(transport.validators(), [None, Some("\"v1\"".into()), None]);
}

/// ⛔ Two accounts never share a validator or a body, even for the same URL on
/// the same transport. Each client gets its own store from the factory.
#[tokio::test]
async fn two_accounts_never_share_an_entry() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body(4001, "completed", "success"), "\"account-a\""),
        Reply::ok(&runs_body(4001, "completed", "success"), "\"account-b\""),
        Reply::not_modified("\"account-a\""),
    ]);
    let (a, _) = github(transport.clone());
    let (b, _) = github(transport.clone());

    list(a.as_ref()).await.unwrap();
    list(b.as_ref()).await.unwrap();
    list(a.as_ref()).await.unwrap();

    let paths = transport.paths();
    assert!(paths.iter().all(|p| p == &paths[0]), "one URL: {paths:?}");
    assert_eq!(
        transport.validators(),
        [None, None, Some("\"account-a\"".into())],
        "B's first request was unconditional although A held a validator for \
         the same URL, and A's second sent A's own, not B's newer one"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

const PAGE_2: &str = "https://api.github.com/repositories/1/actions/runs/4001/jobs?filter=latest&per_page=100&page=2";

fn page_1_link() -> String {
    format!("<{PAGE_2}>; rel=\"next\", <{PAGE_2}>; rel=\"last\"")
}

/// ⛔ A 304 on page 1 must not end the listing. A 304 need not repeat `Link`,
/// and GitHub's measured 304s carry no body; without the stored header the
/// client would read page 1 as the last page and report half the jobs.
#[tokio::test]
async fn a_304_on_page_one_still_follows_to_page_two() {
    let transport = Scripted::new(vec![
        Reply::ok(&jobs_body(&[1, 2]), "\"p1\"").link(&page_1_link()),
        Reply::ok(&jobs_body(&[3]), "\"p2\""),
        // The second tick. Neither 304 carries a Link header.
        Reply::not_modified("\"p1\""),
        Reply::not_modified("\"p2\""),
    ]);
    let (client, ring) = github(transport.clone());

    let first = client.pipeline_jobs(&repo(), 4001).await.unwrap();
    let second = client.pipeline_jobs(&repo(), 4001).await.unwrap();

    assert_eq!(job_ids(&first), [1, 2, 3]);
    assert_eq!(job_ids(&second), [1, 2, 3], "both pages, from the store");
    let paths = transport.paths();
    assert_eq!(paths.len(), 4, "{paths:?}");
    assert_eq!(paths[0], paths[2]);
    assert_eq!(paths[1], paths[3]);
    assert!(
        paths[1].starts_with("/repositories/1/"),
        "followed verbatim"
    );
    assert_eq!(
        transport.validators(),
        [None, None, Some("\"p1\"".into()), Some("\"p2\"".into())],
        "each page is its own conditional request"
    );
    assert_eq!(
        statuses(&ring),
        [Some(200), Some(200), Some(304), Some(304)]
    );
}

/// Page 1 unchanged and page 2 changed: the listing is page 1 from the store
/// and page 2 fresh, never page 1 alone.
#[tokio::test]
async fn a_304_on_page_one_and_a_200_on_page_two_combine() {
    let transport = Scripted::new(vec![
        Reply::ok(&jobs_body(&[1, 2]), "\"p1\"").link(&page_1_link()),
        Reply::ok(&jobs_body(&[3]), "\"p2\""),
        Reply::not_modified("\"p1\""),
        Reply::ok(&jobs_body(&[3, 4]), "\"p2-b\""),
    ]);
    let (client, _) = github(transport.clone());

    client.pipeline_jobs(&repo(), 4001).await.unwrap();
    let second = client.pipeline_jobs(&repo(), 4001).await.unwrap();

    assert_eq!(job_ids(&second), [1, 2, 3, 4]);
}

// ---------------------------------------------------------------------------
// Rate limits and the request log
// ---------------------------------------------------------------------------

/// A 304 is recorded as a 304, with the 304's own rate-limit headers: the
/// debug pane shows the saving and the budget as it stands, not the stored
/// response's.
#[tokio::test]
async fn a_304_is_logged_as_a_304_with_its_own_rate_limit_headers() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body(4001, "completed", "success"), "\"v1\"")
            .remaining(4_900)
            .reset(1_789_669_380),
        Reply::not_modified("\"v1\"")
            .remaining(4_900)
            .reset(1_789_669_999),
    ]);
    let (client, ring) = github(transport.clone());

    list(client.as_ref()).await.unwrap();
    list(client.as_ref()).await.unwrap();

    let log = ring.entries();
    assert_eq!(statuses(&ring), [Some(200), Some(304)]);
    assert_eq!(log[1].error, None, "a revalidation is not an error");
    assert_eq!(log[1].ratelimit_remaining, Some(4_900), "the 304's own");
    assert_eq!(log[1].ratelimit_reset, Some(1_789_669_999), "the 304's own");
}

/// ⛔ A rate limit is a rate limit even when a body is stored: the stored body
/// is never served in place of an error, the error still drives the backoff,
/// and the entry survives it for the next 304.
#[tokio::test]
async fn a_rate_limit_after_a_stored_body_is_still_a_rate_limit() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body(4001, "completed", "success"), "\"v1\""),
        Reply::status(403).remaining(0).reset(1_789_670_000),
        Reply::not_modified("\"v1\""),
    ]);
    let (client, ring) = github(transport.clone());

    list(client.as_ref()).await.unwrap();
    let limited = list(client.as_ref())
        .await
        .expect_err("not served from the store");
    assert!(
        matches!(
            limited,
            ClientError::RateLimited {
                reset: Some(1_789_670_000),
                ..
            }
        ),
        "{limited:?}"
    );
    assert!(limited.should_back_off());
    assert_eq!(ring.entries()[1].ratelimit_remaining, Some(0));

    assert_eq!(list(client.as_ref()).await.unwrap(), [4001]);
    assert_eq!(
        transport.validators()[2].as_deref(),
        Some("\"v1\""),
        "the entry outlived the rate limit"
    );
}

/// A 304 that does not repeat `X-OAuth-Scopes` still answers `token_self`,
/// from the header stored with the body.
#[tokio::test]
async fn a_304_on_the_user_endpoint_keeps_the_stored_scopes() {
    let user = r#"{"id":7,"login":"octo","name":"Octo"}"#;
    let transport = Scripted::new(vec![
        Reply::ok(user, "\"u1\"").scopes("repo, workflow"),
        Reply::not_modified("\"u1\""),
    ]);
    let (client, _) = github(transport.clone());

    let first = client.token_self().await.unwrap();
    let second = client.token_self().await.unwrap();
    assert_eq!(first.scopes, ["repo", "workflow"]);
    assert_eq!(second.scopes, first.scopes);
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

fn request(url: &str) -> HttpRequest {
    HttpRequest {
        method: "GET",
        url: format!("https://api.github.com{url}"),
        path: url.to_string(),
        headers: Vec::new(),
    }
}

/// Over the entry bound, the least recently USED entry goes, and one that is
/// asked for every tick is never it. That is what keeps a settled watch free
/// while run ids churn past it.
#[tokio::test]
async fn eviction_drops_the_least_recently_used_entry() {
    let inner = Scripted::new(vec![
        Reply::ok("\"settled\"", "\"s\""),
        Reply::ok("\"run-1\"", "\"r1\""),
        Reply::not_modified("\"s\""),
        Reply::ok("\"run-2\"", "\"r2\""),
        Reply::not_modified("\"s\""),
        Reply::ok("\"run-1\"", "\"r1\""),
    ]);
    let transport = ConditionalTransport::with_bounds(inner.clone(), 2, 1 << 20);

    for url in [
        "/settled", "/run-1", "/settled", "/run-2", "/settled", "/run-1",
    ] {
        transport.execute(request(url)).await.unwrap();
    }

    assert_eq!(
        inner.validators(),
        [
            None,
            None,
            Some("\"s\"".into()),
            // /run-2 evicted /run-1, the older of the two it could drop.
            None,
            Some("\"s\"".into()),
            None,
        ],
        "the settled URL kept hitting while the other two took turns"
    );
    assert_eq!(transport.len(), 2);
}

/// The byte bound holds too, and a body larger than the whole budget is not
/// stored at all rather than evicting everything else to make room.
#[tokio::test]
async fn the_byte_bound_holds_and_an_oversized_body_is_not_stored() {
    let inner = Scripted::new(vec![
        Reply::ok("0123456789", "\"a\""),
        Reply::ok("0123456789", "\"b\""),
        Reply::ok(&"x".repeat(64), "\"huge\""),
        Reply::ok("0123456789", "\"c\""),
    ]);
    let transport = ConditionalTransport::with_bounds(inner.clone(), 100, 25);

    transport.execute(request("/a")).await.unwrap();
    transport.execute(request("/b")).await.unwrap();
    assert_eq!((transport.len(), transport.bytes()), (2, 20));

    transport.execute(request("/huge")).await.unwrap();
    assert_eq!(
        (transport.len(), transport.bytes()),
        (2, 20),
        "the oversized body was served and not stored"
    );

    transport.execute(request("/c")).await.unwrap();
    assert_eq!(
        (transport.len(), transport.bytes()),
        (2, 20),
        "the third small body evicted the oldest to stay within 25 bytes"
    );
}

/// ⚠️ A 304 to a request that carried no validator should not happen. It must
/// not panic, and it must not be served as an empty body: there is nothing to
/// serve, and retrying without a validator is the request that produced it.
/// It is an `Unexpected` error, which is neither fatal nor a backoff, so the
/// next tick simply asks again; and it is in the request log with its path.
#[tokio::test]
async fn an_unsolicited_304_is_an_error_and_not_a_panic() {
    let transport = Scripted::new(vec![Reply::not_modified("\"from-nowhere\"")]);
    let (client, ring) = github(transport.clone());

    let error = list(client.as_ref()).await.expect_err("nothing to serve");
    assert!(
        matches!(error, ClientError::Unexpected { status: 304, .. }),
        "{error:?}"
    );
    assert!(!error.is_fatal());
    assert!(!error.should_back_off());
    let entry = &ring.entries()[0];
    assert!(
        entry.error.as_deref().unwrap_or("").contains("304"),
        "{entry:?}"
    );
}

/// A store with no room makes every request unconditional, which is exactly
/// the behaviour before this cache existed.
#[tokio::test]
async fn a_zero_sized_store_sends_no_validators() {
    let inner = Scripted::new(vec![Reply::ok("[]", "\"a\""), Reply::ok("[]", "\"a\"")]);
    let transport = ConditionalTransport::with_bounds(inner.clone(), 0, 0);
    transport.execute(request("/a")).await.unwrap();
    transport.execute(request("/a")).await.unwrap();
    assert_eq!(inner.validators(), [None, None]);
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

/// ⛔ GitLab's behaviour does not change in this build: its client is not
/// wrapped, so even a response carrying an `ETag` is never sent back.
#[tokio::test]
async fn a_gitlab_client_never_sends_a_validator() {
    let transport = Scripted::new(vec![
        Reply::ok("[]", "W/\"gitlab-weak\""),
        Reply::ok("[]", "W/\"gitlab-weak\""),
    ]);
    let client = client_for(
        &Account::default(),
        &Secret::new("glpat-SECRET"),
        transport.clone(),
        RequestRing::new(4),
    )
    .unwrap();
    for _ in 0..2 {
        client
            .list_pipelines(&ProjectRef::Id(7), &ListQuery::exact("main", None, 5))
            .await
            .unwrap();
    }
    assert_eq!(transport.validators(), [None, None]);
}

// ---------------------------------------------------------------------------
// Two ticks of a real poller
// ---------------------------------------------------------------------------

fn github_config() -> Config {
    let raw = r#"
[accounts.gh]
provider = "github"
token = { env = "TOK" }

[[watches]]
id = "bw-ci"
account = "gh"
project = "acme-corp/monorepo"
ref = "main"
sources = ["push"]
workflow = "ci.yml"
"#;
    bridgewatch_core::config::parse_str(raw, std::path::Path::new("test.toml"))
        .expect("a GitHub configuration loads")
        .config
}

/// The whole path, twice: a live run is listed and its jobs fetched on both
/// ticks, the second tick is answered entirely by 304s, and the snapshot the
/// tray draws is the same. This is the "the second cache must agree with the
/// first" check: the pipeline cache sees the bytes a 200 would have carried.
#[tokio::test]
async fn a_second_tick_answered_by_304s_draws_the_same_snapshot() {
    let transport = Scripted::new(vec![
        Reply::ok(&runs_body(4001, "in_progress", ""), "\"runs\""),
        Reply::ok(&jobs_body(&[1, 2]), "\"jobs\""),
        Reply::not_modified("\"runs\""),
        Reply::not_modified("\"jobs\""),
    ]);
    let config = github_config();
    let ring = RequestRing::new(50);
    let mut clients: BTreeMap<String, Arc<dyn CiClient>> = BTreeMap::new();
    for (name, account) in &config.accounts {
        clients.insert(
            name.clone(),
            client_for(
                account,
                &Secret::new("ghp_SECRET"),
                transport.clone(),
                ring.clone(),
            )
            .unwrap(),
        );
    }
    let mut poller = Poller::with_clients(&config, clients, ring.clone()).unwrap();

    let first = poller.tick().await.snapshot;
    let second = poller.tick().await.snapshot;

    assert_eq!(transport.unused(), 0, "{:?}", transport.paths());
    assert_eq!(
        statuses(&ring),
        [Some(200), Some(200), Some(304), Some(304)]
    );
    let rows = |s: &bridgewatch_core::verdict::Snapshot| {
        serde_json::to_value(&s.watches).expect("the watches serialise")
    };
    assert_eq!(rows(&first), rows(&second));
    assert_eq!(first.icon_state.as_str(), second.icon_state.as_str());
}

// ---------------------------------------------------------------------------
// The production transport, on loopback
// ---------------------------------------------------------------------------

/// A loopback server that answers the Nth request with the Nth response and
/// records each request head.
fn sequenced_server(responses: Vec<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    std::thread::spawn(move || {
        let mut responses = responses.into_iter();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = String::new();
            let mut reader =
                std::io::BufReader::new(stream.try_clone().expect("the socket can be cloned"));
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let done = line == "\r\n" || line == "\n";
                        head.push_str(&line);
                        if done {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            recorder.lock().unwrap().push(head);
            let Some(response) = responses.next() else {
                return;
            };
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            let _ = stream.read(&mut [0u8; 64]);
        }
    });
    (base, seen)
}

/// Over real HTTP: the weak validator reaches the wire byte for byte, the 304
/// is served from the store, and GitHub's `x-ratelimit-*` spelling is read.
#[tokio::test]
async fn over_real_http_the_validator_is_sent_and_the_304_is_served() {
    let (base, seen) = sequenced_server(vec![
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         ETag: W/\"686897696a7c876b7e\"\r\n\
         X-RateLimit-Remaining: 4862\r\n\
         X-RateLimit-Reset: 1789689777\r\n\
         Connection: close\r\n\
         Content-Length: 11\r\n\r\n[\"payload\"]",
        "HTTP/1.1 304 Not Modified\r\n\
         ETag: W/\"686897696a7c876b7e\"\r\n\
         X-RateLimit-Remaining: 4862\r\n\
         X-RateLimit-Reset: 1789689777\r\n\
         Connection: close\r\n\r\n",
    ]);
    let real: Arc<dyn Transport> =
        Arc::new(ReqwestTransport::new(std::time::Duration::from_secs(5)).unwrap());
    let transport = ConditionalTransport::new(real);
    let get = || HttpRequest {
        method: "GET",
        url: format!("{base}/repos/acme-corp/monorepo/actions/runs"),
        path: "/repos/acme-corp/monorepo/actions/runs".into(),
        headers: Vec::new(),
    };

    let first = transport.execute(get()).await.unwrap();
    let second = transport.execute(get()).await.unwrap();

    assert_eq!((first.status, first.body.as_str()), (200, "[\"payload\"]"));
    assert_eq!(
        (second.status, second.body.as_str()),
        (304, "[\"payload\"]")
    );
    assert_eq!(first.ratelimit_remaining, Some(4862));
    assert_eq!(second.ratelimit_remaining, Some(4862));
    assert_eq!(second.ratelimit_reset, Some(1_789_689_777));

    let heads = seen.lock().unwrap().clone();
    assert_eq!(heads.len(), 2);
    assert!(
        !heads[0].to_ascii_lowercase().contains("if-none-match"),
        "{}",
        heads[0]
    );
    assert!(
        heads[1]
            .lines()
            .any(|l| l.eq_ignore_ascii_case("if-none-match: W/\"686897696a7c876b7e\"")),
        "{}",
        heads[1]
    );
}

/// ⚠️ The unprefixed (GitLab) spelling wins when a response carries both, so
/// a GitLab response reads exactly as it did before GitHub's was added.
#[tokio::test]
async fn the_gitlab_rate_limit_spelling_wins_over_github_s() {
    let (base, _) = sequenced_server(vec![
        "HTTP/1.1 200 OK\r\n\
         RateLimit-Remaining: 1999\r\n\
         RateLimit-Reset: 1789669380\r\n\
         X-RateLimit-Remaining: 5\r\n\
         X-RateLimit-Reset: 1\r\n\
         Connection: close\r\n\
         Content-Length: 2\r\n\r\n[]",
    ]);
    let real = ReqwestTransport::new(std::time::Duration::from_secs(5)).unwrap();
    let response = real
        .execute(HttpRequest {
            method: "GET",
            url: format!("{base}/projects/7/pipelines"),
            path: "/projects/7/pipelines".into(),
            headers: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(response.ratelimit_remaining, Some(1999));
    assert_eq!(response.ratelimit_reset, Some(1_789_669_380));
}
