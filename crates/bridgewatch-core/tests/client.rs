//! The GitLab client: URL shapes, pagination, status mapping and the request
//! ring.
//!
//! ⛔ The promise that a token never reaches a LOG line is tested in
//! `tests/client_log.rs` instead, and the split is load-bearing rather than
//! tidiness: the tests here drive the client's `debug!` callsite with no
//! subscriber installed, which is exactly what poisons `tracing`'s interest
//! cache for a test trying to capture it. See the `CAPTURE` comment in
//! `tests/support/mod.rs`.

mod support;

use std::sync::{Arc, Mutex};

use bridgewatch_core::client::http::HttpRequest;
use bridgewatch_core::client::{ClientError, GitLabClient, ListQuery, RequestRing};
use bridgewatch_core::config::{Account, AuthHeader, ProjectRef};
use bridgewatch_core::status::Status;
use bridgewatch_core::token::Secret;
use support::{Canned, client_with};

/// An exact ref and a single source are pushed to the API; the ordering is by
/// id so "newest" is not a guess.
#[tokio::test]
async fn an_exact_ref_and_one_source_go_into_the_query() {
    let transport = Canned::new(vec![(200, "[]".into(), None)]);
    let (client, _) = client_with(transport.clone(), AuthHeader::PrivateToken);

    client
        .list_pipelines(
            &ProjectRef::Id(82468124),
            &ListQuery::exact("main", Some("push".into()), 20),
        )
        .await
        .unwrap();

    let path = &transport.paths()[0];
    assert!(path.starts_with("/projects/82468124/pipelines?"), "{path}");
    for expected in [
        "ref=main",
        "source=push",
        "order_by=id",
        "sort=desc",
        "per_page=20",
    ] {
        assert!(path.contains(expected), "{path} is missing {expected}");
    }
}

/// A glob ref cannot be expressed to the API, so recent pipelines are listed by
/// `updated_at` and filtered here.
#[tokio::test]
async fn a_scanning_query_omits_ref_and_orders_by_updated_at() {
    let transport = Canned::new(vec![(200, "[]".into(), None)]);
    let (client, _) = client_with(transport.clone(), AuthHeader::PrivateToken);

    client
        .list_pipelines(&ProjectRef::Id(1), &ListQuery::scan(30))
        .await
        .unwrap();

    let path = &transport.paths()[0];
    assert!(!path.contains("ref="), "{path}");
    assert!(!path.contains("source="), "{path}");
    assert!(path.contains("order_by=updated_at"), "{path}");
}

/// A project path is URL-encoded into the URL.
#[tokio::test]
async fn a_project_path_is_url_encoded() {
    let transport = Canned::new(vec![(200, "[]".into(), None)]);
    let (client, _) = client_with(transport.clone(), AuthHeader::PrivateToken);

    client
        .list_pipelines(
            &ProjectRef::Path("acme-corp/monorepo".into()),
            &ListQuery::scan(10),
        )
        .await
        .unwrap();

    assert!(
        transport.paths()[0].starts_with("/projects/acme-corp%2Fmonorepo/"),
        "{}",
        transport.paths()[0]
    );
}

/// Pagination follows `x-next-page` and stops when it is absent.
#[tokio::test]
async fn jobs_paginate_on_x_next_page() {
    let page1 = r#"[{"id":1,"name":"a","status":"success"}]"#.to_string();
    let page2 = r#"[{"id":2,"name":"b","status":"failed"}]"#.to_string();
    let transport = Canned::new(vec![(200, page1, Some("2".into())), (200, page2, None)]);
    let (client, _) = client_with(transport.clone(), AuthHeader::PrivateToken);

    let jobs = client.pipeline_jobs(&ProjectRef::Id(1), 42).await.unwrap();

    assert_eq!(jobs.len(), 2, "both pages");
    assert_eq!(jobs[1].name, "b");
    let paths = transport.paths();
    assert_eq!(paths.len(), 2);
    assert!(
        paths[0].contains("page=1") && paths[0].contains("per_page=100"),
        "{paths:?}"
    );
    assert!(paths[1].contains("page=2"), "{paths:?}");
}

/// An instance that echoes the current page back must not loop forever.
#[tokio::test]
async fn pagination_stops_when_the_next_page_does_not_advance() {
    let body = r#"[{"id":1,"name":"a","status":"success"}]"#.to_string();
    let transport = Canned::new(vec![
        (200, body.clone(), Some("1".into())),
        (200, body, Some("1".into())),
    ]);
    let (client, _) = client_with(transport.clone(), AuthHeader::PrivateToken);

    let jobs = client.pipeline_jobs(&ProjectRef::Id(1), 42).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        transport.paths().len(),
        1,
        "one request, not an infinite loop"
    );
}

/// Statuses map to the error the poller reacts to.
#[tokio::test]
async fn statuses_map_to_the_right_errors() {
    for (status, check) in [
        (401u16, "auth"),
        (403, "auth"),
        (404, "not_found"),
        (429, "rate_limited"),
        (500, "server"),
        (418, "unexpected"),
        // A 3xx only reaches the client because the real transport refuses to
        // follow it (H1); it must name itself rather than read as "unexpected".
        (301, "redirect"),
        (302, "redirect"),
        (307, "redirect"),
        (308, "redirect"),
    ] {
        let transport = Canned::new(vec![(status, "{}".into(), None)]);
        let (client, _) = client_with(transport, AuthHeader::PrivateToken);
        let err = client
            .get_pipeline(&ProjectRef::Id(1), 1)
            .await
            .expect_err("non-2xx must be an error");

        match (check, &err) {
            ("auth", ClientError::Auth { .. }) => assert!(err.is_fatal()),
            ("not_found", ClientError::NotFound { .. }) => {}
            ("rate_limited", ClientError::RateLimited { .. }) => assert!(err.should_back_off()),
            ("server", ClientError::Server { .. }) => assert!(err.should_back_off()),
            ("unexpected", ClientError::Unexpected { .. }) => {}
            ("redirect", ClientError::Redirect { status: got, .. }) => {
                assert_eq!(*got, status);
                assert!(!err.is_fatal() && !err.should_back_off(), "{err:?}");
            }
            _ => panic!("{status} produced {err:?}"),
        }
    }
}

/// A body that is not what we expected is a decode error naming the path, not a
/// panic.
#[tokio::test]
async fn a_bad_body_is_a_decode_error() {
    let transport = Canned::new(vec![(200, "not json at all".into(), None)]);
    let (client, _) = client_with(transport, AuthHeader::PrivateToken);
    let err = client
        .get_pipeline(&ProjectRef::Id(1), 7)
        .await
        .unwrap_err();
    let ClientError::Decode { path, .. } = err else {
        panic!("expected Decode, got {err:?}");
    };
    assert!(path.contains("/pipelines/7"), "{path}");
}

/// A status GitLab adds tomorrow must not break a poll.
#[tokio::test]
async fn an_unknown_status_decodes_instead_of_failing() {
    let body = r#"{"id":1,"sha":"abc","ref":"main","status":"quantum_superposition"}"#;
    let transport = Canned::new(vec![(200, body.into(), None)]);
    let (client, _) = client_with(transport, AuthHeader::PrivateToken);

    let pipeline = client.get_pipeline(&ProjectRef::Id(1), 1).await.unwrap();
    assert_eq!(
        pipeline.status,
        Status::Unknown("quantum_superposition".into())
    );
    assert_eq!(pipeline.status.as_str(), "quantum_superposition");
    assert!(!pipeline.status.is_live());
}

/// Both auth header spellings are supported.
#[tokio::test]
async fn the_auth_header_is_configurable() {
    for (header, name, value) in [
        (AuthHeader::PrivateToken, "PRIVATE-TOKEN", "glpat-SECRET"),
        (
            AuthHeader::AuthorizationBearer,
            "Authorization",
            "Bearer glpat-SECRET",
        ),
    ] {
        let transport = Canned::new(vec![(200, "[]".into(), None)]);
        let (client, _) = client_with(transport.clone(), header);
        client
            .list_pipelines(&ProjectRef::Id(1), &ListQuery::scan(10))
            .await
            .unwrap();
        assert_eq!(transport.headers(), [(name.to_string(), value.to_string())]);
    }
}

/// Every request lands in the ring with the rate-limit headers, and the ring is
/// bounded. Nothing in it is the token.
#[tokio::test]
async fn the_request_ring_records_and_never_holds_the_token() {
    let transport = Canned::new(vec![]);
    let (client, ring) = client_with(transport, AuthHeader::PrivateToken);

    for i in 0..15u64 {
        let _ = client.get_pipeline(&ProjectRef::Id(1), i).await;
    }

    let entries = ring.entries();
    assert_eq!(entries.len(), 10, "the ring is bounded at its capacity");
    assert_eq!(
        entries[0].path, "/projects/1/pipelines/5",
        "the oldest were evicted"
    );
    assert_eq!(entries[0].method, "GET");
    assert_eq!(entries[0].status, Some(200));
    assert_eq!(entries[0].ratelimit_remaining, Some(1999));
    assert_eq!(entries[0].ratelimit_reset, Some(1789669380));

    let serialised = serde_json::to_string(&entries).unwrap();
    assert!(
        !serialised.contains("glpat-SECRET"),
        "the request log is shown in a debug pane and pasted into issues"
    );
}

/// A failed request is recorded too, with the error and no status.
#[tokio::test]
async fn a_failed_request_is_still_recorded() {
    let transport = Canned::new(vec![(429, "{}".into(), None)]);
    let (client, ring) = client_with(transport, AuthHeader::PrivateToken);
    let _ = client.get_pipeline(&ProjectRef::Id(1), 1).await;

    let entries = ring.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, Some(429));
    assert!(
        entries[0]
            .error
            .as_deref()
            .unwrap()
            .contains("rate limited")
    );
}

/// ⛔ Lo8. The two types that hold the rendered credential header print it
/// as `<redacted>` under `{:?}`. Both used to derive `Debug`, so one `{:?}` in a
/// `tracing` field, a panic message or an `unwrap` on a `Result` carrying a
/// request would have put the token in a log the user then pastes into an issue.
#[test]
fn debug_never_prints_the_credential_header() {
    for header in [AuthHeader::PrivateToken, AuthHeader::AuthorizationBearer] {
        let (client, _) = client_with(Canned::new(vec![]), header);
        let rendered = format!("{client:?} {client:#?}");
        assert!(!rendered.contains("glpat-SECRET"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert!(
            rendered.contains(header.header_name()),
            "the header NAME is kept, because which spelling was sent is what a \
             401 investigation needs: {rendered}"
        );
    }

    let request = HttpRequest {
        method: "GET",
        url: "https://gitlab.example/api/v4/projects/1".into(),
        path: "/projects/1".into(),
        headers: vec![("PRIVATE-TOKEN".into(), "glpat-SECRET".into())],
    };
    let rendered = format!("{request:?} {request:#?}");
    assert!(!rendered.contains("glpat-SECRET"), "{rendered}");
    assert!(rendered.contains("PRIVATE-TOKEN") && rendered.contains("<redacted>"));
    assert!(rendered.contains("/projects/1"), "the rest is still useful");
}

/// A `Secret` cannot be printed by accident.
#[test]
fn a_secret_redacts_itself() {
    let secret = Secret::new("glpat-SECRET");
    assert_eq!(format!("{secret}"), "<redacted>");
    assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
    assert_eq!(secret.expose(), "glpat-SECRET");
}

/// The fixture transport answers the same endpoints the real one does, which is
/// what makes it a fair stand-in.
#[tokio::test]
async fn the_fixture_transport_serves_the_recorded_endpoints() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let transport =
        Arc::new(bridgewatch_core::client::FixtureTransport::load(&dir).expect("fixture loads"));
    assert_eq!(transport.meta().primary, 2857464986);
    assert_eq!(transport.meta().project, 82468124);
    assert!(transport.meta().source.contains("2857464986"));

    let (client, _) = client_with(transport, AuthHeader::PrivateToken);
    let project = ProjectRef::Id(82468124);

    let pipeline = client.get_pipeline(&project, 2857464986).await.unwrap();
    assert_eq!(pipeline.sha7(), "ca41ab2");
    assert_eq!(pipeline.status, Status::Failed);

    let bridges = client.pipeline_bridges(&project, 2857464986).await.unwrap();
    assert_eq!(bridges.len(), 4);
    let android = bridges
        .iter()
        .find(|b| b.name == "trigger:android")
        .unwrap();
    assert_eq!(android.status, Status::Failed);

    let child = android.downstream_pipeline.as_ref().unwrap();
    let child_jobs = client
        .child_jobs(&ProjectRef::Id(child.project_id.unwrap()), child.id)
        .await
        .unwrap();
    assert!(
        child_jobs
            .iter()
            .any(|j| j.name == "verify:android" && j.status == Status::Failed)
    );

    let missing = client.pipeline_jobs(&project, 999).await.unwrap_err();
    assert!(matches!(missing, ClientError::NotFound { .. }));
}

/// Recording round-trips: a fixture recorded through the client reloads into a
/// transport that answers the same questions.
///
/// This is what `bridgewatch fixture record` runs, so it is covered without
/// needing a token or a network.
#[tokio::test]
async fn a_recorded_fixture_reloads_and_answers_the_same() {
    let source_dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let transport =
        Arc::new(bridgewatch_core::client::FixtureTransport::load(&source_dir).expect("loads"));
    let (client, _) = client_with(transport, AuthHeader::PrivateToken);

    let out = std::env::temp_dir().join(format!("bw-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);

    let recorded = bridgewatch_core::client::fixture::record(
        &client,
        &ProjectRef::Id(82468124),
        2857464986,
        &out,
        false,
    )
    .await
    .expect("records");

    for expected in ["fixture.json", "list.json", "jobs.json", "bridges.json"] {
        assert!(
            recorded.files.contains(&expected.to_string()),
            "{:?}",
            recorded.files
        );
    }
    assert_eq!(recorded.children.len(), 4, "one entry per bridge");
    assert_eq!(recorded.children["trigger:website"], Some(2857465135));
    assert!(
        recorded
            .files
            .iter()
            .filter(|f| f.starts_with("child-"))
            .count()
            == 4,
        "a child file per bridge with a downstream: {:?}",
        recorded.files
    );

    // The recording is usable as a fixture in its own right.
    let replayed = Arc::new(
        bridgewatch_core::client::FixtureTransport::load(&out).expect("the recording reloads"),
    );
    assert_eq!(replayed.meta().primary, 2857464986);
    let (client2, _) = client_with(replayed, AuthHeader::PrivateToken);
    let bridges = client2
        .pipeline_bridges(&ProjectRef::Id(82468124), 2857464986)
        .await
        .unwrap();
    assert_eq!(bridges.len(), 4);
    assert!(
        bridges
            .iter()
            .any(|b| b.name == "trigger:android" && b.status == Status::Failed)
    );

    let _ = std::fs::remove_dir_all(&out);
}

/// A dead bridge records as a bridge with no child file, and says so.
#[tokio::test]
async fn recording_a_dead_bridge_reports_it() {
    let dir = support::fixtures_dir().join("synth-dead-bridge");
    let transport =
        Arc::new(bridgewatch_core::client::FixtureTransport::load(&dir).expect("loads"));
    let (client, _) = client_with(transport, AuthHeader::PrivateToken);

    let out = std::env::temp_dir().join(format!("bw-record-dead-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let recorded = bridgewatch_core::client::fixture::record(
        &client,
        &ProjectRef::Id(82468124),
        2857464986,
        &out,
        false,
    )
    .await
    .expect("records");

    assert_eq!(
        recorded.children["trigger:website"], None,
        "no child to record"
    );
    assert_eq!(
        recorded
            .files
            .iter()
            .filter(|f| f.starts_with("child-"))
            .count(),
        3
    );
    let _ = std::fs::remove_dir_all(&out);
}

/// ⛔ M26. `ScrubMode::Check` reports and touches nothing.
///
/// `fixture scrub --check` used to scrub for real and then write the old bytes
/// back from memory: a gate whose failure mode was modifying the tree it was
/// checking, and which lost the file outright if interrupted between the writes.
#[test]
fn a_scrub_check_reads_and_never_writes() {
    use bridgewatch_core::client::fixture::{ScrubMode, scrub_dir_with};

    let dir = std::env::temp_dir().join(format!("bw-scrub-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dirty = dir.join("jobs.json");
    let raw = "[\n  {\n    \"id\": 1,\n    \"name\": \"x\",\n    \"user\": { \"name\": \"A Person\" }\n  }\n]\n";
    std::fs::write(&dirty, raw).unwrap();
    let modified = std::fs::metadata(&dirty).unwrap().modified().unwrap();

    let changed = scrub_dir_with(&dir, ScrubMode::Check).unwrap();
    assert_eq!(
        changed,
        std::slice::from_ref(&dirty),
        "the dirty file is reported"
    );
    assert_eq!(
        std::fs::read_to_string(&dirty).unwrap(),
        raw,
        "and not touched"
    );
    assert_eq!(
        std::fs::metadata(&dirty).unwrap().modified().unwrap(),
        modified,
        "not even rewritten with the same bytes"
    );

    let changed = scrub_dir_with(&dir, ScrubMode::Write).unwrap();
    assert_eq!(changed, std::slice::from_ref(&dirty));
    assert!(
        !std::fs::read_to_string(&dirty)
            .unwrap()
            .contains("A Person")
    );
    assert!(
        scrub_dir_with(&dir, ScrubMode::Check).unwrap().is_empty(),
        "and a clean directory checks clean"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The real transport
//
// ⛔ Everything above drives a fake. `ReqwestTransport` — the one that actually
// carries the credential — had no test at all, so its redirect policy, its
// headers and its handling of a 3xx were whatever reqwest's defaults happened
// to be. These two tests talk to a socket on loopback, which is the only way to
// gate a policy that lives inside the HTTP client.
// ---------------------------------------------------------------------------

use std::io::{BufRead as _, Read as _, Write as _};
use std::net::TcpListener;

/// A loopback HTTP server that answers every request with `response` and
/// records the request head it was sent, headers included.
///
/// Deliberately hand-rolled: a test server is the sort of thing a dependency is
/// added for, and this needs one endpoint, one method and no TLS.
fn tiny_server(response: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();

    std::thread::spawn(move || {
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
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            // Drain whatever the client still has to say, so it sees the
            // response rather than a reset connection.
            let _ = stream.read(&mut [0u8; 64]);
        }
    });
    (base, seen)
}

fn account_at(base_url: &str) -> Account {
    Account {
        base_url: base_url.to_string(),
        ..Account::default()
    }
}

/// The production transport sends the credential to the host it was configured
/// for, spelled the way the account asked, with bridgewatch's user agent.
#[tokio::test]
async fn the_real_transport_sends_the_configured_header() {
    let (base, seen) = tiny_server(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n[]",
    );
    let transport =
        bridgewatch_core::client::ReqwestTransport::new(std::time::Duration::from_secs(5))
            .expect("a transport builds");
    let client = GitLabClient::new(
        &account_at(&base),
        &Secret::new("glpat-loopback"),
        Arc::new(transport),
        RequestRing::new(4),
    );

    let pipelines = client
        .list_pipelines(&ProjectRef::Id(7), &ListQuery::exact("main", None, 5))
        .await
        .expect("200 with an empty list");
    assert!(pipelines.is_empty());

    // ⚠ Lowercased: hyper writes header names in lower case on the wire, so
    // the account's `PRIVATE-TOKEN` spelling arrives as `private-token`. That
    // is HTTP being case-insensitive, not the header being wrong, and it is
    // what anyone reading a packet capture will see.
    let head = seen.lock().unwrap().join("").to_ascii_lowercase();
    assert!(
        head.contains("get /api/v4/projects/7/pipelines?ref=main"),
        "{head}"
    );
    assert!(head.contains("private-token: glpat-loopback"), "{head}");
    assert!(head.contains("user-agent: bridgewatch/"), "{head}");
}

/// ⛔ H1. A redirect is refused, and the credential does not follow it.
///
/// reqwest's default policy follows up to ten hops and strips only
/// `Authorization`, `Cookie`, `cookie2`, `Proxy-Authorization` and
/// `WWW-Authenticate` on a cross-host hop — so `PRIVATE-TOKEN`, which is how
/// GitLab spells a personal access token, travelled to whatever host the
/// redirect named. Only `Authorization: Bearer` accounts were ever protected.
#[tokio::test]
async fn a_redirect_is_an_error_and_the_token_does_not_travel() {
    let (elsewhere, elsewhere_seen) = tiny_server(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n[]",
    );
    // The static response has to name the other server, and the port is only
    // known at run time, so the redirect target is leaked into a leaked string.
    // A test process is exactly where that is fine.
    let location: &'static str = Box::leak(
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {elsewhere}/api/v4/projects/7/pipelines\r\nContent-Length: 0\r\n\r\n"
        )
        .into_boxed_str(),
    );
    let (base, _) = tiny_server(location);

    let transport =
        bridgewatch_core::client::ReqwestTransport::new(std::time::Duration::from_secs(5))
            .expect("a transport builds");
    let client = GitLabClient::new(
        &account_at(&base),
        &Secret::new("glpat-must-not-travel"),
        Arc::new(transport),
        RequestRing::new(4),
    );

    let err = client
        .list_pipelines(&ProjectRef::Id(7), &ListQuery::exact("main", None, 5))
        .await
        .expect_err("a 3xx is an error, not a hop");
    assert!(
        matches!(err, ClientError::Redirect { status: 302, .. }),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("does not follow redirects"),
        "the message says why: {err}"
    );

    // Long enough that a followed redirect would have arrived.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let followed = elsewhere_seen.lock().unwrap();
    assert!(
        followed.is_empty(),
        "the redirect target was contacted, so the token travelled: {followed:?}"
    );
}
