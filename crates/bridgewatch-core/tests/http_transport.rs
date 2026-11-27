//! `ReqwestTransport` against a socket on loopback, for what only a real HTTP
//! client can show: each response header it reads, a ranged tail, a server that
//! closes mid-body, a port nobody listens on, and (on Linux) a machine with no
//! CA roots. Plus the request ring's own housekeeping.

use std::io::{BufRead as _, Write as _};
use std::net::TcpListener;
use std::time::Duration;

use bridgewatch_core::client::{
    ClientError, HttpRequest, InFlight, RequestLog, RequestRing, ReqwestTransport, Transport,
};

/// A loopback server that reads one request head and writes `response`
/// verbatim, then closes the connection, once per connection.
fn serve(response: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                line.clear();
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    base
}

fn get(url: String, tail_bytes: Option<usize>) -> HttpRequest {
    HttpRequest {
        method: "GET",
        path: "/x".into(),
        url,
        headers: vec![("private-token".into(), "glpat-loopback".into())],
        body: None,
        anonymous: false,
        tail_bytes,
    }
}

fn transport() -> ReqwestTransport {
    ReqwestTransport::new(Duration::from_secs(5)).expect("a transport builds")
}

#[tokio::test]
async fn every_header_the_clients_read_is_taken_from_the_response() {
    let base = serve(
        "HTTP/1.1 200 OK\r\nx-next-page: 2\r\nretry-after: 30\r\nratelimit-remaining: 7\r\n\
         x-ratelimit-reset: 1789669380\r\netag: \"abc\"\r\nlink: <https://x/?page=2>; rel=\"next\"\r\n\
         x-oauth-scopes: \r\ngithub-authentication-token-expiration: 2026-12-01 00:00:00 UTC\r\n\
         location: https://elsewhere.example/\r\ncontent-length: 2\r\nconnection: close\r\n\r\n[]",
    );
    let response = transport()
        .execute(get(format!("{base}/x"), None))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "[]");
    assert_eq!(response.next_page.as_deref(), Some("2"));
    assert_eq!(response.retry_after, Some(30));
    assert_eq!(response.ratelimit_remaining, Some(7));
    assert_eq!(response.ratelimit_reset, Some(1789669380));
    assert_eq!(response.etag.as_deref(), Some("\"abc\""));
    assert!(response.link.is_some());
    assert_eq!(response.oauth_scopes.as_deref(), Some(""));
    assert_eq!(
        response.token_expiration.as_deref(),
        Some("2026-12-01 00:00:00 UTC")
    );
    assert_eq!(
        response.location.as_deref(),
        Some("https://elsewhere.example/")
    );
    assert!(!response.truncated);
}

/// A 206 says where its bytes start: only a range that starts past zero
/// means the head of the log is missing.
#[tokio::test]
async fn a_ranged_tail_is_truncated_only_when_it_starts_past_the_first_byte() {
    for (range, truncated) in [
        ("bytes 5-9/10", true),
        ("bytes 0-9/10", false),
        ("items 5-9/10", false),
        ("bytes five-9/10", false),
    ] {
        let response: &'static str = Box::leak(
            format!(
                "HTTP/1.1 206 Partial Content\r\ncontent-range: {range}\r\ncontent-length: 5\r\n\
                 connection: close\r\n\r\nhello"
            )
            .into_boxed_str(),
        );
        let base = serve(response);
        let answer = transport()
            .execute(get(format!("{base}/x"), Some(64)))
            .await
            .unwrap();
        assert_eq!(answer.body, "hello");
        assert_eq!(answer.truncated, truncated, "{range}");
    }
}

/// A body cut short is a transport error, whether it is read whole or as a
/// tail.
#[tokio::test]
async fn a_server_that_closes_mid_body_is_a_transport_error() {
    let base = serve("HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nab");
    for tail in [None, Some(64)] {
        let error = transport()
            .execute(get(format!("{base}/x"), tail))
            .await
            .unwrap_err();
        assert!(matches!(error, ClientError::Transport(_)), "{error:?}");
    }
}

/// A refused connection is described through its cause, and the URL reqwest
/// puts in the message is cut off.
#[tokio::test]
async fn a_refused_connection_says_why_without_the_url() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let error = transport()
        .execute(get(format!("http://127.0.0.1:{port}/x?secret=1"), None))
        .await
        .unwrap_err();
    let ClientError::Transport(message) = error else {
        panic!("a transport error");
    };
    assert!(!message.contains("secret=1"), "{message}");
    assert!(!message.contains(" for url ("), "{message}");
    assert!(
        message.contains(": "),
        "the cause chain is walked: {message}"
    );
}

/// ⛔ On Linux, a machine whose CA roots are all missing cannot build a TLS
/// client at all, and that is an error, not a panic. Run in a child process,
/// because the roots are read from the environment.
#[cfg(target_os = "linux")]
#[test]
fn a_machine_with_no_ca_roots_cannot_build_a_transport() {
    if std::env::var_os("BW_NO_ROOTS_CHILD").is_some() {
        let Err(ClientError::Transport(message)) = ReqwestTransport::new(Duration::from_secs(1))
        else {
            panic!("a transport built with no CA roots");
        };
        assert!(message.contains("CA certificates"), "{message}");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bw-no-roots-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.pem");
    std::fs::write(&empty, "").unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_machine_with_no_ca_roots_cannot_build_a_transport",
        ])
        .env("BW_NO_ROOTS_CHILD", "1")
        .env("SSL_CERT_FILE", &empty)
        .env("SSL_CERT_DIR", &dir)
        .status()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(status.success());
}

fn entry(path: &str) -> RequestLog {
    RequestLog {
        method: "GET".into(),
        path: path.into(),
        status: Some(200),
        ms: 1,
        ratelimit_remaining: None,
        ratelimit_reset: None,
        retry_after: None,
        error: None,
        at: chrono::Utc::now(),
    }
}

#[test]
fn the_ring_holds_its_capacity_and_can_be_shrunk_cleared_or_switched_off() {
    let ring = RequestRing::default();
    assert!(ring.is_empty());
    for n in 0..5 {
        ring.record(entry(&format!("/{n}")));
    }
    assert_eq!(ring.len(), 5);
    ring.set_capacity(2);
    let paths: Vec<String> = ring.entries().into_iter().map(|e| e.path).collect();
    assert_eq!(paths, ["/3", "/4"]);
    ring.clear();
    assert!(ring.is_empty());

    let off = RequestRing::new(0);
    off.record(entry("/dropped"));
    assert!(off.is_empty());
}

#[tokio::test]
async fn a_default_in_flight_bound_hands_out_a_slot() {
    let bound = InFlight::default();
    assert!(bound.acquire().await.is_some());
}
