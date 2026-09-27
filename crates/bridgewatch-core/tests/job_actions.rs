//! A job's log tail, and the two writes (retry, play), against scripted
//! transports only.
//!
//! ⛔ Nothing here reaches a network. Every answer is written from the
//! providers' documented shapes (GitLab `jobs/:id/trace`, `retry`, `play`;
//! GitHub `actions/jobs/:id/logs` and `rerun`) and from what was measured on a
//! public run on 2026-09-26: GitHub's log endpoint answers `302` to a signed
//! blob URL, which ignores `Range`. Project names are placeholders.

mod support;

use std::sync::Arc;

use bridgewatch_core::actions::{self, JobAction, JobTarget};
use bridgewatch_core::client::http::Tail;
use bridgewatch_core::client::log::{self as joblog, LOG_TAIL_LINES, MAX_LINE_CHARS};
use bridgewatch_core::client::{
    CiClient, ClientError, ConditionalTransport, HttpRequest, HttpResponse, RequestRing,
    ScriptTransport, Transport, client_for,
};
use bridgewatch_core::config::{self, Account, OAuthSource, ProjectRef, Provider, TokenSource};
use bridgewatch_core::oauth::{
    self, BuiltinClients, Endpoints, OAuthSession, OAuthTransport, TokenSet, store,
};
use bridgewatch_core::token::Secret;
use support::MemoryStore;

const PROJECT: &str = "acme-corp/monorepo";
const BLOB: &str = "https://results.blob.example/actions/job-logs.txt?sv=2025&sig=SIGNATURE-SECRET";

fn project() -> ProjectRef {
    ProjectRef::Path(PROJECT.to_string())
}

fn account(provider: Provider, actions: bool) -> Account {
    Account {
        actions,
        ..Account::for_provider(provider)
    }
}

/// A client over `t` exactly as `client_for` builds one, with a literal token
/// an assertion can look for.
fn client_over(
    provider: Provider,
    actions: bool,
    t: Arc<ScriptTransport>,
) -> (Arc<dyn CiClient>, RequestRing) {
    let ring = RequestRing::new(16);
    let client = client_for(
        &account(provider, actions),
        &Secret::new("TOKEN-SECRET-0001"),
        t,
        ring.clone(),
    )
    .unwrap();
    (client, ring)
}

fn header(request: &HttpRequest, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn carries_a_credential(request: &HttpRequest) -> bool {
    request.headers.iter().any(|(n, v)| {
        n.eq_ignore_ascii_case("authorization")
            || n.eq_ignore_ascii_case("private-token")
            || v.contains("TOKEN-SECRET")
    })
}

fn redirect(status: u16, location: Option<&str>) -> HttpResponse {
    HttpResponse {
        location: location.map(str::to_string),
        ..HttpResponse::plain(status, "")
    }
}

// ---------------------------------------------------------------------------
// Cleaning
// ---------------------------------------------------------------------------

#[test]
fn ansi_escapes_and_control_characters_are_stripped_and_a_tab_is_kept() {
    let body = "\u{1b}[32;1mok\u{1b}[0m\tdone\u{7}\n\u{1b}]0;title\u{7}plain\u{1b}(B\n";
    assert_eq!(
        joblog::clean(body, Provider::Gitlab, false, 40),
        vec!["ok\tdone", "plain"]
    );
}

#[test]
fn gitlab_section_markers_go_and_the_section_title_stays() {
    let body = "\u{1b}[0Ksection_start:1790000000:step_script[collapsed=true]\r\u{1b}[0K\u{1b}[36;1mExecuting \"step_script\"\u{1b}[0;m\n\
                $ make test\n\
                \u{1b}[0Ksection_end:1790000010:step_script\r\u{1b}[0K\n\
                ERROR: Job failed: exit code 2\n";
    assert_eq!(
        joblog::clean(body, Provider::Gitlab, false, 40),
        vec![
            "Executing \"step_script\"",
            "$ make test",
            "ERROR: Job failed: exit code 2"
        ],
        "a line that held only a section_end is not a blank line the job printed"
    );
}

#[test]
fn a_line_overwritten_with_a_carriage_return_keeps_what_a_terminal_would_show() {
    let body = "Downloading 10%\rDownloading 55%\rDownloading 100%\r\nnext\r\n";
    assert_eq!(
        joblog::clean(body, Provider::Github, false, 40),
        vec!["Downloading 100%", "next"]
    );
}

#[test]
fn github_timestamps_are_trimmed_and_its_group_markers_tidied() {
    let body = "\u{feff}2026-09-26T20:43:51.0980920Z ##[group]Run make test\n\
                2026-09-26T20:43:51.0990000Z make: *** [test] Error 2\n\
                2026-09-26T20:43:51.1000000Z ##[endgroup]\n\
                2026-09-26T20:43:51.1122790Z ##[error]Process completed with exit code 2.\n";
    assert_eq!(
        joblog::clean(body, Provider::Github, false, 40),
        vec![
            "Run make test",
            "make: *** [test] Error 2",
            "##[error]Process completed with exit code 2."
        ]
    );
}

#[test]
fn the_gitlab_runners_timestamps_are_trimmed_and_a_continuation_joins_its_line() {
    let body = "2026-09-26T04:23:33.812998Z 01O Running tests\n\
                2026-09-26T04:23:34.000001Z 01E partial \n\
                2026-09-26T04:23:34.100001Z 01E+line\n";
    assert_eq!(
        joblog::clean(body, Provider::Gitlab, false, 40),
        vec!["Running tests", "partial line"]
    );
    // A GitLab job that prints its own timestamps keeps them.
    let own = "2026-09-26T04:23:33Z app started\n";
    assert_eq!(
        joblog::clean(own, Provider::Gitlab, false, 40),
        vec!["2026-09-26T04:23:33Z app started"]
    );
}

#[test]
fn bidirectional_overrides_are_removed_so_a_line_reads_as_what_it_says() {
    let body = "rm -rf /tmp/\u{202e}txt.exe\u{2066}x\u{2069}\n";
    assert_eq!(
        joblog::clean(body, Provider::Gitlab, false, 40),
        vec!["rm -rf /tmp/txt.exex"]
    );
}

#[test]
fn a_tail_drops_its_first_partial_line_and_only_the_last_lines_are_kept() {
    let mut body = String::from("ial line cut in half\n");
    for i in 0..100 {
        body.push_str(&format!("line {i}\n"));
    }
    body.push_str("\n\n");
    let lines = joblog::clean(&body, Provider::Gitlab, true, LOG_TAIL_LINES);
    assert_eq!(lines.len(), LOG_TAIL_LINES);
    assert_eq!(lines.first().unwrap(), "line 60");
    assert_eq!(
        lines.last().unwrap(),
        "line 99",
        "trailing blank lines are not the end of the log"
    );
    let whole = joblog::clean("only line\n", Provider::Gitlab, false, 40);
    assert_eq!(
        whole,
        vec!["only line"],
        "an untruncated body keeps its first line"
    );
}

#[test]
fn a_huge_line_is_cut_with_an_ellipsis() {
    let body = "x".repeat(MAX_LINE_CHARS * 3);
    let lines = joblog::clean(&body, Provider::Gitlab, false, 40);
    assert_eq!(lines[0].chars().count(), MAX_LINE_CHARS + 1);
    assert!(lines[0].ends_with('…'));
}

#[test]
fn a_tail_never_holds_much_more_than_twice_what_it_keeps() {
    let mut tail = Tail::new(1024);
    let chunk = vec![b'a'; 300];
    let mut peak = 0;
    for _ in 0..1000 {
        tail.push(&chunk);
        peak = peak.max(tail.held());
    }
    tail.push(b"END");
    assert!(peak <= 2 * 1024 + 300, "held {peak} bytes");
    let (bytes, dropped) = tail.finish();
    assert_eq!(bytes.len(), 1024);
    assert!(bytes.ends_with(b"END"));
    assert!(dropped);
    let (short, dropped) = {
        let mut t = Tail::new(1024);
        t.push(b"small");
        t.finish()
    };
    assert_eq!(short, b"small");
    assert!(!dropped, "a body shorter than the tail was not cut");
}

// ---------------------------------------------------------------------------
// GitHub: one explicit redirect follow, without the token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_github_log_follows_its_redirect_once_and_the_second_request_carries_no_credential() {
    let t = Arc::new(ScriptTransport::new());
    t.reply_with("/actions/jobs/42/logs", redirect(302, Some(BLOB)));
    t.reply_with(
        "results.blob.example",
        HttpResponse {
            truncated: true,
            ..HttpResponse::plain(
                200,
                "cut\n2026-09-26T20:43:51.0980920Z \u{1b}[31mboom\u{1b}[0m\n",
            )
        },
    );
    let (client, ring) = client_over(Provider::Github, false, t.clone());

    let tail = client.job_log_tail(&project(), 42, 4096).await.unwrap();
    assert_eq!(tail.lines, vec!["boom"]);
    assert!(tail.truncated);

    let sent = t.requests();
    assert_eq!(sent.len(), 2, "one request and exactly one follow");
    assert!(
        carries_a_credential(&sent[0]),
        "the API request is authenticated"
    );
    assert_eq!(sent[1].url, BLOB);
    assert!(sent[1].anonymous);
    assert!(
        !carries_a_credential(&sent[1]),
        "the signed URL got a credential header: {:?}",
        sent[1]
    );
    assert!(
        header(&sent[1], "Range").is_none(),
        "the blob store ignores Range"
    );
    assert_eq!(sent[1].tail_bytes, Some(4096));

    // Both are in the ring, and the signature is in neither.
    let logged = ring.entries();
    assert_eq!(logged.len(), 2);
    assert_eq!(
        logged[0].path,
        "/repos/acme-corp/monorepo/actions/jobs/42/logs"
    );
    assert_eq!(
        logged[1].path,
        "(redirect) results.blob.example/actions/job-logs.txt"
    );
    assert!(logged.iter().all(|l| !l.path.contains("SIGNATURE")));
}

#[tokio::test]
async fn a_redirect_to_anything_but_https_or_to_nowhere_is_refused_and_not_followed() {
    for location in [
        Some("http://results.blob.example/log.txt"),
        Some("file:///etc/passwd"),
        Some("https://user@results.blob.example/log.txt"),
        Some("/relative/log.txt"),
        Some(""),
        None,
    ] {
        let t = Arc::new(ScriptTransport::new());
        t.reply_with("/actions/jobs/42/logs", redirect(302, location));
        let (client, _) = client_over(Provider::Github, false, t.clone());
        let err = client.job_log_tail(&project(), 42, 4096).await.unwrap_err();
        assert!(
            matches!(err, ClientError::Unsupported { .. }),
            "{location:?}: {err:?}"
        );
        assert_eq!(t.requests().len(), 1, "{location:?} was followed");
    }
}

#[tokio::test]
async fn a_signed_url_that_refuses_is_not_read_as_the_accounts_token() {
    let t = Arc::new(ScriptTransport::new());
    t.reply_with("/actions/jobs/42/logs", redirect(302, Some(BLOB)));
    t.reply(
        "results.blob.example",
        403,
        "<Error>AuthenticationFailed</Error>",
    );
    let (client, _) = client_over(Provider::Github, false, t.clone());
    let err = client.job_log_tail(&project(), 42, 4096).await.unwrap_err();
    assert!(
        matches!(err, ClientError::Unexpected { status: 403, .. }),
        "an expired blob signature is not 'check the token': {err:?}"
    );
    // And a 401 on the API leg still is a bad token.
    let t = Arc::new(ScriptTransport::new());
    t.reply("/actions/jobs/42/logs", 401, "{}");
    let (client, _) = client_over(Provider::Github, false, t.clone());
    assert!(matches!(
        client.job_log_tail(&project(), 42, 4096).await,
        Err(ClientError::Auth { status: 401, .. })
    ));
}

#[tokio::test]
async fn the_second_leg_of_a_signed_in_account_goes_out_without_the_sign_in() {
    // An account that signs in puts its bearer token on EVERY request in
    // `OAuthTransport`; the anonymous flag is what keeps it off this one.
    let memory = MemoryStore::new();
    let account = Account {
        token: TokenSource::Oauth(OAuthSource { client_id: None }),
        ..Account::for_provider(Provider::Github)
    };
    store::save(
        "gh",
        &token_set("ghu_ACCESS-SECRET", "https://api.github.com", vec![]),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply_with("/actions/jobs/42/logs", redirect(302, Some(BLOB)));
    t.reply("results.blob.example", 200, "tail\n");
    let session = OAuthSession::new(
        "gh",
        &account,
        &OAuthSource { client_id: None },
        &builtin(),
        t.clone(),
        memory.clone(),
    )
    .unwrap();
    let client = client_for(
        &oauth::bearer_account(&account),
        &Secret::new(""),
        Arc::new(OAuthTransport::new(t.clone(), Arc::new(session))),
        RequestRing::new(8),
    )
    .unwrap();

    let tail = client.job_log_tail(&project(), 42, 4096).await.unwrap();
    assert_eq!(tail.lines, vec!["tail"]);
    let sent = t.requests();
    assert_eq!(
        header(&sent[0], "Authorization").as_deref(),
        Some("Bearer ghu_ACCESS-SECRET")
    );
    assert!(header(&sent[1], "Authorization").is_none(), "{:?}", sent[1]);
    assert!(
        !sent[1]
            .headers
            .iter()
            .any(|(_, v)| v.contains("ACCESS-SECRET"))
    );
}

#[tokio::test]
async fn a_credential_left_on_an_anonymous_request_is_taken_off_by_the_sign_in_layer() {
    let memory = MemoryStore::new();
    let account = Account {
        token: TokenSource::Oauth(OAuthSource { client_id: None }),
        ..Account::for_provider(Provider::Github)
    };
    let t = Arc::new(ScriptTransport::new());
    t.reply("results.blob.example", 200, "x");
    let session = OAuthSession::new(
        "gh",
        &account,
        &OAuthSource { client_id: None },
        &builtin(),
        t.clone(),
        memory.clone(),
    )
    .unwrap();
    let transport = OAuthTransport::new(t.clone(), Arc::new(session));
    let mut request = joblog::follow(Some(BLOB), 10, false, "/x").unwrap();
    request
        .headers
        .push(("Authorization".into(), "Bearer LEFT-BEHIND".into()));
    request
        .headers
        .push(("PRIVATE-TOKEN".into(), "LEFT-BEHIND".into()));
    transport.execute(request).await.unwrap();
    let sent = t.requests();
    assert_eq!(
        sent.len(),
        1,
        "an anonymous request needs no sign-in, so none was read"
    );
    assert!(
        !carries_a_credential(&sent[0])
            && !sent[0]
                .headers
                .iter()
                .any(|(_, v)| v.contains("LEFT-BEHIND"))
    );
}

#[tokio::test]
async fn a_log_request_is_never_cached_or_made_conditional() {
    let t = Arc::new(ScriptTransport::new());
    for _ in 0..2 {
        t.reply_with(
            "/logs",
            HttpResponse {
                etag: Some("\"v1\"".into()),
                ..HttpResponse::plain(200, "line\n")
            },
        );
    }
    let conditional = ConditionalTransport::new(t.clone());
    for _ in 0..2 {
        let request = HttpRequest {
            method: "GET",
            url: "https://api.github.com/repos/acme-corp/monorepo/actions/jobs/42/logs".into(),
            path: "/logs".into(),
            headers: vec![],
            body: None,
            anonymous: false,
            tail_bytes: Some(10),
        };
        conditional.execute(request).await.unwrap();
    }
    assert!(conditional.is_empty(), "a log body was stored");
    assert!(
        t.requests()
            .iter()
            .all(|r| header(r, "If-None-Match").is_none())
    );
}

// ---------------------------------------------------------------------------
// GitLab: a Range request, a 416, and the same one follow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_gitlab_trace_asks_for_its_tail_with_a_range_and_reads_a_206() {
    let t = Arc::new(ScriptTransport::new());
    t.reply_with(
        "/jobs/7/trace",
        HttpResponse {
            truncated: true,
            ..HttpResponse::plain(206, "partial\n$ make\nERROR: Job failed: exit code 1\n")
        },
    );
    let (client, ring) = client_over(Provider::Gitlab, false, t.clone());
    let tail = client.job_log_tail(&project(), 7, 1024).await.unwrap();
    assert_eq!(tail.lines, vec!["$ make", "ERROR: Job failed: exit code 1"]);
    assert!(tail.truncated);
    let sent = &t.requests()[0];
    assert_eq!(sent.path, "/projects/acme-corp%2Fmonorepo/jobs/7/trace");
    assert_eq!(header(sent, "Range").as_deref(), Some("bytes=-1024"));
    assert_eq!(header(sent, "Accept-Encoding").as_deref(), Some("identity"));
    assert_eq!(
        header(sent, "PRIVATE-TOKEN").as_deref(),
        Some("TOKEN-SECRET-0001")
    );
    assert_eq!(ring.entries()[0].status, Some(206));
}

#[tokio::test]
async fn an_empty_gitlab_trace_answering_416_is_an_empty_log_not_an_error() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/jobs/7/trace", 416, "");
    let (client, _) = client_over(Provider::Gitlab, false, t);
    let tail = client.job_log_tail(&project(), 7, 1024).await.unwrap();
    assert!(tail.lines.is_empty());
    assert!(!tail.truncated);
}

#[tokio::test]
async fn an_archived_gitlab_trace_is_followed_once_to_object_storage_without_the_token() {
    let cdn = "https://cdn.artifacts.example/job.log?Expires=1&Signature=CDN-SIGNATURE";
    let t = Arc::new(ScriptTransport::new());
    t.reply_with("/jobs/7/trace", redirect(302, Some(cdn)));
    t.reply("cdn.artifacts.example", 206, "Job succeeded\n");
    let (client, ring) = client_over(Provider::Gitlab, false, t.clone());
    let tail = client.job_log_tail(&project(), 7, 1024).await.unwrap();
    assert_eq!(tail.lines, vec!["Job succeeded"]);
    let sent = t.requests();
    assert_eq!(sent.len(), 2);
    assert!(!carries_a_credential(&sent[1]), "{:?}", sent[1]);
    assert_eq!(
        header(&sent[1], "Range").as_deref(),
        Some("bytes=-1024"),
        "the CDN honours Range"
    );
    assert!(ring.entries().iter().all(|l| !l.path.contains("SIGNATURE")));
}

// ---------------------------------------------------------------------------
// Writes: refused in the client unless the account opts in
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_on_an_account_without_actions_is_refused_before_anything_is_sent() {
    for provider in [Provider::Gitlab, Provider::Github] {
        let t = Arc::new(ScriptTransport::new());
        let (client, ring) = client_over(provider, false, t.clone());
        let err = client.retry_job(&project(), 7).await.unwrap_err();
        assert!(
            matches!(err, ClientError::ActionsDisabled),
            "{provider}: {err:?}"
        );
        assert!(err.to_string().contains("actions = true"), "{err}");
        if provider == Provider::Gitlab {
            assert!(matches!(
                client.play_job(&project(), 7).await,
                Err(ClientError::ActionsDisabled)
            ));
        }
        assert!(t.requests().is_empty(), "{provider}: a write was sent");
        assert!(ring.is_empty());
    }
}

#[tokio::test]
async fn a_gitlab_retry_posts_to_the_job_and_reads_the_job_it_created() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/jobs/7/retry",
        201,
        r#"{"id":8,"status":"pending","web_url":"https://gitlab.com/acme-corp/monorepo/-/jobs/8"}"#,
    );
    t.reply("/jobs/9/play", 200, r#"{"id":9,"status":"pending"}"#);
    let (client, ring) = client_over(Provider::Gitlab, true, t.clone());

    let outcome = client.retry_job(&project(), 7).await.unwrap();
    assert_eq!(outcome.job_id, Some(8));
    assert_eq!(
        outcome.web_url.as_deref(),
        Some("https://gitlab.com/acme-corp/monorepo/-/jobs/8")
    );
    let played = client.play_job(&project(), 9).await.unwrap();
    assert_eq!(played.job_id, Some(9));

    let sent = t.requests();
    assert_eq!(sent[0].method, "POST");
    assert_eq!(sent[0].path, "/projects/acme-corp%2Fmonorepo/jobs/7/retry");
    assert_eq!(sent[1].path, "/projects/acme-corp%2Fmonorepo/jobs/9/play");
    let logged = ring.entries();
    assert_eq!(logged.len(), 2, "writes are recorded like any request");
    assert_eq!(logged[0].method, "POST");
}

#[tokio::test]
async fn a_gitlab_403_on_a_write_is_the_tokens_scope_and_not_a_bad_token() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/retry",
        403,
        r#"{"error":"insufficient_scope","error_description":"The request requires higher privileges than provided by the access token.","scope":"api"}"#,
    );
    t.reply(
        "/retry",
        403,
        r#"{"message":"403 Forbidden - Job is not retryable"}"#,
    );
    t.reply(
        "/play",
        400,
        r#"{"message":"400 Bad request - Unplayable Job"}"#,
    );
    t.reply("/retry", 401, r#"{"message":"401 Unauthorized"}"#);
    let (client, _) = client_over(Provider::Gitlab, true, t);

    let scope = client.retry_job(&project(), 7).await.unwrap_err();
    assert!(
        matches!(
            scope,
            ClientError::WriteForbidden {
                status: 403,
                provider: Provider::Gitlab
            }
        ),
        "{scope:?}"
    );
    assert!(scope.to_string().contains("api scope"), "{scope}");
    assert!(!scope.to_string().contains("check the token"), "{scope}");

    let job = client.retry_job(&project(), 7).await.unwrap_err();
    assert!(
        matches!(&job, ClientError::JobRefused { message, .. } if message.contains("not retryable")),
        "{job:?}"
    );
    let play = client.play_job(&project(), 7).await.unwrap_err();
    assert!(
        matches!(&play, ClientError::JobRefused { message, .. } if message.contains("Unplayable")),
        "{play:?}"
    );

    let bad = client.retry_job(&project(), 7).await.unwrap_err();
    assert!(
        matches!(bad, ClientError::Auth { status: 401, .. }),
        "a 401 is still a bad token: {bad:?}"
    );
}

#[tokio::test]
async fn a_github_rerun_posts_and_a_permission_403_says_actions_write() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/actions/jobs/42/rerun", 201, "");
    t.reply(
        "/actions/jobs/42/rerun",
        403,
        r#"{"message":"Resource not accessible by integration"}"#,
    );
    t.reply_with(
        "/actions/jobs/42/rerun",
        HttpResponse {
            ratelimit_remaining: Some(0),
            ratelimit_reset: Some(1_790_000_000),
            ..HttpResponse::plain(403, r#"{"message":"API rate limit exceeded"}"#)
        },
    );
    t.reply(
        "/actions/jobs/42/rerun",
        403,
        r#"{"message":"This job cannot be re-run while its run is in progress"}"#,
    );
    let (client, _) = client_over(Provider::Github, true, t.clone());

    let outcome = client.retry_job(&project(), 42).await.unwrap();
    assert_eq!(
        outcome.job_id, None,
        "GitHub re-runs under the same id and says nothing"
    );
    let sent = &t.requests()[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/repos/acme-corp/monorepo/actions/jobs/42/rerun");
    assert_eq!(
        header(sent, "X-GitHub-Api-Version").as_deref(),
        Some("2022-11-28")
    );

    let denied = client.retry_job(&project(), 42).await.unwrap_err();
    assert!(
        matches!(
            denied,
            ClientError::WriteForbidden {
                provider: Provider::Github,
                ..
            }
        ),
        "{denied:?}"
    );
    assert!(denied.to_string().contains("Actions: write"), "{denied}");

    let limited = client.retry_job(&project(), 42).await.unwrap_err();
    assert!(
        matches!(limited, ClientError::RateLimited { .. }),
        "a rate limit is never a permission problem: {limited:?}"
    );

    let busy = client.retry_job(&project(), 42).await.unwrap_err();
    assert!(matches!(busy, ClientError::JobRefused { .. }), "{busy:?}");
}

#[tokio::test]
async fn play_on_github_is_unsupported_and_sends_nothing() {
    let t = Arc::new(ScriptTransport::new());
    let (client, _) = client_over(Provider::Github, true, t.clone());
    let err = client.play_job(&project(), 42).await.unwrap_err();
    assert!(matches!(err, ClientError::Unsupported { .. }), "{err:?}");
    assert!(t.requests().is_empty());
}

// ---------------------------------------------------------------------------
// GitLab sign-in: `api` only when the account opts in
// ---------------------------------------------------------------------------

fn builtin() -> BuiltinClients {
    BuiltinClients {
        github_com: Some("gh-app-0001"),
        github_app_slug: None,
        gitlab_com: Some("gl-app-0001"),
    }
}

fn token_set(access: &str, base: &str, scopes: Vec<String>) -> TokenSet {
    TokenSet {
        access_token: Secret::new(access),
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
        scopes,
        login: Some("someone".into()),
        client_id: if base.contains("github") {
            "gh-app-0001".into()
        } else {
            "gl-app-0001".into()
        },
        base_url: base.into(),
        obtained_at: 1_000,
    }
}

#[test]
fn a_gitlab_sign_in_asks_for_api_only_when_the_account_allows_actions() {
    assert_eq!(
        Endpoints::for_account(&account(Provider::Gitlab, false)).scope,
        Some("read_api")
    );
    assert_eq!(
        Endpoints::for_account(&account(Provider::Gitlab, true)).scope,
        Some("api")
    );
    assert_eq!(
        Endpoints::for_account(&account(Provider::Github, true)).scope,
        None
    );
}

#[tokio::test]
async fn a_write_on_a_read_api_sign_in_says_sign_in_again_and_sends_nothing() {
    let gitlab = Account {
        actions: true,
        token: TokenSource::Oauth(OAuthSource { client_id: None }),
        ..Account::for_provider(Provider::Gitlab)
    };
    for (scopes, allowed) in [
        (vec!["read_api".to_string()], false),
        (vec!["api".to_string()], true),
        (vec![], true),
    ] {
        let memory = MemoryStore::new();
        store::save(
            "gl",
            &token_set("gl-ACCESS-SECRET", "https://gitlab.com", scopes.clone()),
            memory.as_ref(),
        )
        .unwrap();
        let t = Arc::new(ScriptTransport::new());
        t.reply("/retry", 201, r#"{"id":8}"#);
        let session = OAuthSession::new(
            "gl",
            &gitlab,
            &OAuthSource { client_id: None },
            &builtin(),
            t.clone(),
            memory.clone(),
        )
        .unwrap();
        let client = client_for(
            &oauth::bearer_account(&gitlab),
            &Secret::new(""),
            Arc::new(OAuthTransport::new(t.clone(), Arc::new(session))),
            RequestRing::new(8),
        )
        .unwrap();
        let result = client.retry_job(&project(), 7).await;
        if allowed {
            assert!(result.is_ok(), "{scopes:?}: {result:?}");
            assert_eq!(t.requests().len(), 1);
        } else {
            let err = result.unwrap_err();
            assert!(
                matches!(err, ClientError::SignInForActions { .. }),
                "{err:?}"
            );
            assert!(err.to_string().contains("sign in again"), "{err}");
            assert!(
                t.requests().is_empty(),
                "a read_api token was sent on a write"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Job URLs
// ---------------------------------------------------------------------------

fn config_of(raw: &str) -> config::Config {
    config::parse_str(raw, std::path::Path::new("x.toml"))
        .expect("parses")
        .config
}

const TWO_PROVIDERS: &str = r#"
[accounts.gl]
token = { env = "BW_TEST_UNSET" }

[accounts.self]
base_url = "https://git.example.com/gitlab"
token = { env = "BW_TEST_UNSET" }

[accounts.gh]
provider = "github"
token = { env = "BW_TEST_UNSET" }

[[watches]]
id = "w"
account = "gl"
project = "acme-corp/monorepo"
"#;

#[test]
fn a_job_url_is_resolved_to_its_account_project_and_id_by_host() {
    let config = config_of(TWO_PROVIDERS);
    let found = |url: &str| actions::locate(&config, url, None);
    assert_eq!(
        found("https://gitlab.com/acme-corp/platform/monorepo/-/jobs/12345").unwrap(),
        JobTarget {
            account: "gl".into(),
            provider: Provider::Gitlab,
            project: ProjectRef::Path("acme-corp/platform/monorepo".into()),
            job_id: 12345,
        }
    );
    let prefixed =
        found("https://git.example.com/gitlab/acme-corp/monorepo/-/jobs/9?x=1#L3").unwrap();
    assert_eq!(prefixed.account, "self");
    assert_eq!(
        prefixed.project,
        ProjectRef::Path("acme-corp/monorepo".into())
    );
    let gh =
        found("https://github.com/acme-corp/monorepo/actions/runs/36269522104/job/108481615716")
            .unwrap();
    assert_eq!(gh.account, "gh");
    assert_eq!(gh.provider, Provider::Github);
    assert_eq!(gh.job_id, 108481615716);
}

#[test]
fn a_url_that_is_not_a_job_or_on_no_accounts_host_is_refused_with_why() {
    let config = config_of(TWO_PROVIDERS);
    let err = |url: &str, account: Option<&str>| {
        actions::locate(&config, url, account)
            .unwrap_err()
            .to_string()
    };
    assert!(
        err("https://gitlab.com/acme-corp/monorepo/-/pipelines/1", None)
            .contains("not a job's page")
    );
    assert!(
        err("https://evil.example/acme-corp/monorepo/-/jobs/1", None)
            .contains("any configured account")
    );
    assert!(
        err("https://gitlab.com.evil.example/a/b/-/jobs/1", None)
            .contains("any configured account")
    );
    assert!(err("file:///etc/passwd", None).contains("not an http(s) URL"));
    assert!(err("https://gitlab.com/../x/-/jobs/1", None).contains("not a job's page"));
    // Pinned to an account, a URL on another account's host is refused.
    assert!(
        err("https://github.com/a/b/actions/runs/1/job/2", Some("gl"))
            .contains("not on the host of account")
    );
    assert!(err("https://gitlab.com/a/b/-/jobs/1", Some("nope")).contains("no account named"));
}

#[test]
fn two_accounts_on_one_host_must_be_told_apart() {
    let config = config_of(&format!(
        "{TWO_PROVIDERS}\n[accounts.gl2]\ntoken = {{ env = \"BW_TEST_UNSET\" }}\n"
    ));
    let url = "https://gitlab.com/acme-corp/monorepo/-/jobs/1";
    let err = actions::locate(&config, url, None).unwrap_err().to_string();
    assert!(
        err.contains("gl, gl2") && err.contains("--account"),
        "{err}"
    );
    assert_eq!(
        actions::locate(&config, url, Some("gl2")).unwrap().account,
        "gl2"
    );
}

#[tokio::test]
async fn perform_goes_through_the_client_and_its_guard() {
    let t = Arc::new(ScriptTransport::new());
    let (client, _) = client_over(Provider::Gitlab, false, t.clone());
    let target = JobTarget {
        account: "gl".into(),
        provider: Provider::Gitlab,
        project: project(),
        job_id: 7,
    };
    assert!(matches!(
        actions::perform(client.as_ref(), &target, JobAction::Retry).await,
        Err(ClientError::ActionsDisabled)
    ));
    assert!(t.requests().is_empty());
}

#[test]
fn a_watch_view_says_actions_only_when_they_are_on() {
    let mut view: bridgewatch_core::verdict::WatchView = serde_json::from_str(
        r#"{"id":"w","role":"primary","icon_state":null,"rows":[],"error":null}"#,
    )
    .unwrap();
    assert!(!view.actions);
    assert!(
        !serde_json::to_string(&view).unwrap().contains("actions"),
        "an account that never opted in serialises as before"
    );
    view.actions = true;
    assert!(
        serde_json::to_string(&view)
            .unwrap()
            .contains("\"actions\":true")
    );
}
