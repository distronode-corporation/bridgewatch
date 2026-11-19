//! Crash reports verify the server's certificate, whatever `SSL_VERIFY` says.
//!
//! `sentry::init` reads the `SSL_VERIFY` environment variable, and
//! `SSL_VERIFY=false` makes the reqwest client `sentry` builds for itself accept
//! any certificate. `crash.rs` hands `sentry` a client of its own instead. This
//! file proves the difference on the wire, against a local TLS server whose
//! certificate (`tests/tls/`) no system trusts:
//!
//! - bridgewatch's reporter, with `SSL_VERIFY=false` and with `SSL_VERIFY=0`,
//!   refuses the certificate during the handshake, and no request reaches the
//!   server;
//! - `sentry`'s own client, with `SSL_VERIFY=false`, completes the handshake and
//!   sends the report. That is the control: it shows the switch is real and that
//!   the server would have accepted a report from a client that did not check;
//! - `sentry`'s own client with no `SSL_VERIFY` refuses too, so the server's
//!   certificate is refused for being untrusted, not for some other fault.
//!
//! (`SSL_VERIFY=0` never turned verification off even in `sentry`'s client: the
//! SDK parses the value as a bool, and `0` is not one. It is checked anyway.)
//!
//! Each case runs in a child process of this test binary, with the environment
//! variable set on the child: `sentry::init` binds a process-wide hub and panic
//! hook, and setting an environment variable inside a running test is `unsafe`.

#[allow(dead_code)]
#[path = "../src/crash.rs"]
mod crash;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Set on a child process: which client it reports through.
const CASE: &str = "BRIDGEWATCH_TLS_CASE";

/// What the stand-in saw.
#[derive(Default, Debug)]
struct Seen {
    /// Handshakes the client completed.
    handshakes: usize,
    /// Alerts the client sent while refusing a handshake, by name.
    refusals: Vec<String>,
    /// HTTP requests that arrived over a completed handshake.
    requests: usize,
}

/// A TLS server on a local port that presents the untrusted test certificate,
/// records every handshake and request, and answers each request 200.
fn untrusted_stand_in() -> (u16, Arc<Mutex<Seen>>) {
    let certs = vec![
        CertificateDer::from_pem_slice(include_bytes!("tls/untrusted-cert.pem"))
            .expect("the test certificate parses"),
    ];
    let key = PrivateKeyDer::from_pem_slice(include_bytes!("tls/untrusted-key.pem"))
        .expect("the test key parses");
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("a server config"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let record = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let config = config.clone();
            let record = record.clone();
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let conn = rustls::ServerConnection::new(config).expect("a server connection");
                let mut tls = rustls::StreamOwned::new(conn, stream);
                while tls.conn.is_handshaking() {
                    if let Err(e) = tls.conn.complete_io(&mut tls.sock) {
                        let why = match e
                            .get_ref()
                            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                        {
                            Some(rustls::Error::AlertReceived(alert)) => format!("{alert:?}"),
                            _ => format!("other: {e}"),
                        };
                        record.lock().unwrap().refusals.push(why);
                        return;
                    }
                }
                record.lock().unwrap().handshakes += 1;
                serve(tls, &record);
            });
        }
    });
    (port, seen)
}

/// Answer HTTP/1.1 requests on a completed handshake until the client closes.
fn serve(stream: impl Read + Write, record: &Mutex<Seen>) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        record.lock().unwrap().requests += 1;
        let reply =
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
        if reader.get_mut().write_all(reply).is_err() {
            return;
        }
        let _ = reader.get_mut().flush();
    }
}

/// Wait up to `limit` for the stand-in to have seen something, then return it.
fn settle(seen: &Mutex<Seen>, limit: Duration) -> (usize, Vec<String>, usize) {
    let start = Instant::now();
    loop {
        {
            let s = seen.lock().unwrap();
            if s.requests > 0 || !s.refusals.is_empty() || start.elapsed() > limit {
                // A little longer, for a retry or a second connection.
                drop(s);
                std::thread::sleep(Duration::from_millis(500));
                let s = seen.lock().unwrap();
                return (s.handshakes, s.refusals.clone(), s.requests);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The child: report one event through the client `CASE` names, then print
/// what the stand-in saw. Ignored unless run as a child by the test below.
#[test]
#[ignore = "run as a child process by ssl_verify_cannot_turn_off_verification"]
fn child_reports_once() {
    let Ok(case) = std::env::var(CASE) else {
        return;
    };
    let (port, seen) = untrusted_stand_in();
    let dsn = format!("https://public@127.0.0.1:{port}/1");
    match case.as_str() {
        "bridgewatch" => {
            let reporter = crash::Reporter::new(Some(&dsn), "tls-probe".into());
            reporter.set_consent(true);
            assert!(reporter.is_sending(), "the reporter has a client");
            sentry::capture_message("tls probe", sentry::Level::Error);
            if let Some(client) = sentry::Hub::current().client() {
                client.flush(Some(Duration::from_secs(10)));
            }
            let result = settle(&seen, Duration::from_secs(10));
            reporter.shutdown();
            println!("RESULT {result:?}");
        }
        "sentry" => {
            let mut options = sentry::ClientOptions::new();
            options.dsn = Some(dsn.parse().expect("a DSN"));
            let guard = sentry::init(options);
            sentry::capture_message("tls probe", sentry::Level::Error);
            guard.flush(Some(Duration::from_secs(10)));
            let result = settle(&seen, Duration::from_secs(10));
            drop(guard);
            println!("RESULT {result:?}");
        }
        other => panic!("unknown case {other}"),
    }
}

/// Run one case in a child process and parse what it printed.
fn run_case(case: &str, ssl_verify: Option<&str>) -> (usize, Vec<String>, usize) {
    let exe = std::env::current_exe().expect("this test binary");
    let mut command = Command::new(exe);
    command
        .args([
            "child_reports_once",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CASE, case)
        .env_remove("SSL_VERIFY")
        .env_remove("SENTRY_DSN");
    // Both clients would send through a proxy from the environment (sentry sets
    // one explicitly from `HTTP(S)_PROXY`, and reqwest reads them too), and the
    // stand-in is only reachable directly.
    for proxy in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        command
            .env_remove(proxy)
            .env_remove(proxy.to_ascii_lowercase());
    }
    if let Some(value) = ssl_verify {
        command.env("SSL_VERIFY", value);
    }
    let output = command.output().expect("run the child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "child {case} SSL_VERIFY={ssl_verify:?} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = stdout
        .lines()
        // libtest prints "test child_reports_once ... " on the same line first.
        .find_map(|l| l.split_once("RESULT ").map(|(_, r)| r))
        .unwrap_or_else(|| panic!("child {case} printed no RESULT:\n{stdout}"));
    parse_result(line)
}

/// `(handshakes, ["UnknownCA", ...], requests)`, as `{:?}` printed it.
fn parse_result(line: &str) -> (usize, Vec<String>, usize) {
    let inner = line.trim().trim_start_matches('(').trim_end_matches(')');
    let (handshakes, rest) = inner.split_once(", [").expect("handshakes");
    let (refusals, requests) = rest.rsplit_once("], ").expect("refusals");
    let refusals = refusals
        .split(", ")
        .filter(|r| !r.is_empty())
        .map(|r| r.trim_matches('"').to_string())
        .collect();
    (
        handshakes.trim().parse().expect("a count"),
        refusals,
        requests.trim().parse().expect("a count"),
    )
}

/// The client refused the certificate: no handshake, no request, and at least
/// one refusal for a certificate reason.
fn assert_refused(what: &str, (handshakes, refusals, requests): (usize, Vec<String>, usize)) {
    assert_eq!(handshakes, 0, "{what}: no handshake may complete");
    assert_eq!(requests, 0, "{what}: no request may arrive");
    assert!(
        refusals.iter().any(|r| matches!(
            r.as_str(),
            "UnknownCA" | "BadCertificate" | "CertificateUnknown"
        )),
        "{what}: the client refuses the certificate, got {refusals:?}"
    );
}

#[test]
fn ssl_verify_cannot_turn_off_verification() {
    // The control first: sentry's own client, told SSL_VERIFY=false, does send.
    let (handshakes, _, requests) = run_case("sentry", Some("false"));
    assert!(
        handshakes >= 1 && requests >= 1,
        "sentry's own client with SSL_VERIFY=false should have sent the report \
         (handshakes {handshakes}, requests {requests})"
    );
    // And without SSL_VERIFY it refuses, so the certificate is refused for
    // being untrusted.
    assert_refused(
        "sentry's own client, no SSL_VERIFY",
        run_case("sentry", None),
    );

    // bridgewatch's reporter refuses whatever SSL_VERIFY says.
    assert_refused(
        "bridgewatch, SSL_VERIFY=false",
        run_case("bridgewatch", Some("false")),
    );
    assert_refused(
        "bridgewatch, SSL_VERIFY=0",
        run_case("bridgewatch", Some("0")),
    );
    assert_refused("bridgewatch, no SSL_VERIFY", run_case("bridgewatch", None));
}
