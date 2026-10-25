//! The crash-report switch, proved on the wire rather than argued.
//!
//! A local HTTP server stands in for Sentry: the DSN points at it, so every
//! request the real `sentry` client and its real reqwest transport make lands
//! here and is counted. With consent off a panic and a captured error send
//! NOTHING; with consent on, one envelope arrives and every home-directory
//! path in it has lost its user name; with consent taken back, nothing again.
//!
//! ⚠ ONE test function, run in its own process (cargo runs each file in
//! `tests/` as a separate binary): `sentry::init` binds the process-wide hub
//! and installs a process-wide panic hook, so phases that ran as parallel
//! tests would see each other's panics.
//!
//! The app is a binary crate, so the module is included by path.

#[allow(dead_code)]
#[path = "../src/crash.rs"]
mod crash;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Every request the stand-in received: (request line, body).
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Accept connections for the life of the process, answering 200 to each
/// request and recording it. Keep-alive is honoured, since reqwest reuses its
/// connection.
fn stand_in_for_sentry() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let record = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let record = record.clone();
            std::thread::spawn(move || serve(stream, record));
        }
    });
    (port, seen)
}

fn serve(stream: TcpStream, seen: Seen) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
    let mut writer = stream;
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
        seen.lock().unwrap().push((
            request_line.trim_end().to_string(),
            String::from_utf8_lossy(&body).into_owned(),
        ));
        let _ = writer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        let _ = writer.flush();
    }
}

/// The paths a report must not carry, planted in the panic message: one per
/// platform, plus whatever this machine's own home directory is (which the
/// stack frames of a debug build carry for real).
const PLANTED: &str = "config at /home/alice/.config/bridgewatch/config.toml, \
                       /Users/bob/Library/Logs/bridgewatch and C:\\Users\\carol\\AppData\\bw";

fn panic_in_a_thread(message: &'static str) {
    let joined = std::thread::spawn(move || panic!("{message}")).join();
    assert!(joined.is_err(), "the thread did not panic");
}

fn capture_an_error() {
    let error = std::io::Error::other("could not read /home/alice/secret.toml");
    sentry::capture_error(&error);
}

/// Wait for `expected` requests to arrive (up to ten seconds), then watch for
/// `quiet` more, and say how many there are. The quiet window is how "and
/// nothing else arrived" is shown, including when `expected` is zero.
fn settle(seen: &Seen, expected: usize, quiet: Duration) -> usize {
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.lock().unwrap().len() < expected && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(quiet);
    seen.lock().unwrap().len()
}

fn flush() {
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(Duration::from_secs(5)));
    }
}

#[test]
fn consent_decides_whether_anything_reaches_the_crash_report_server() {
    let (port, seen) = stand_in_for_sentry();
    let dsn = format!("http://publickey@127.0.0.1:{port}/42");
    let release = crash::release_name(env!("CARGO_PKG_VERSION"), "test");
    let reporter = crash::Reporter::new(Some(&dsn), release.clone());
    assert!(reporter.has_address());

    // 1. Consent off from the start (unanswered reads as no): no client is
    //    ever created, and a panic and a captured error send nothing.
    reporter.set_consent(crash::consent_of(None));
    reporter.set_consent(crash::consent_of(Some(false)));
    assert!(!reporter.has_client(), "a client exists without consent");
    panic_in_a_thread("off before any client existed");
    capture_an_error();
    flush();
    assert_eq!(
        settle(&seen, 0, Duration::from_secs(2)),
        0,
        "a request reached the server with consent off: {:?}",
        seen.lock().unwrap()
    );

    // 2. Consent on: one panic is one envelope, scrubbed.
    reporter.set_consent(crash::consent_of(Some(true)));
    assert!(reporter.is_sending());
    panic_in_a_thread(PLANTED);
    flush();
    assert_eq!(
        settle(&seen, 1, Duration::from_secs(2)),
        1,
        "expected exactly one envelope: {:?}",
        seen.lock().unwrap()
    );
    let (request_line, body) = seen.lock().unwrap()[0].clone();
    assert!(
        request_line.starts_with("POST /api/42/envelope/"),
        "{request_line}"
    );
    // The panic message arrived, with every planted user name gone...
    assert!(
        body.contains("/home/<user>/.config/bridgewatch/config.toml"),
        "{body}"
    );
    assert!(
        body.contains("/Users/<user>/Library/Logs/bridgewatch"),
        "{body}"
    );
    assert!(body.contains(r"C:\\Users\\<user>\\AppData\\bw"), "{body}");
    for name in ["alice", "bob", "carol"] {
        assert!(!body.contains(name), "{name} survived the scrub: {body}");
    }
    // ...and so has this machine's own, which a debug build's stack frames and
    // module list carry for real.
    let home = std::env::var("HOME").unwrap_or_default();
    if home.starts_with("/home/") || home.starts_with("/Users/") {
        assert!(
            !body.contains(&format!("{home}/")),
            "the home directory {home} survived the scrub"
        );
    }
    // No user, no host name, no IP; the release is the bundle id's.
    let event = body
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("exception").is_some())
        .expect("an event item in the envelope");
    assert!(event.get("user").is_none(), "{event}");
    assert!(event.get("server_name").is_none(), "{event}");
    assert!(!body.contains("ip_address"), "{body}");
    assert_eq!(event["release"], release.as_str());
    assert!(release.starts_with("com.distronode.bridgewatch@"));

    // 3. Consent taken back at runtime: nothing more arrives.
    reporter.set_consent(crash::consent_of(Some(false)));
    assert!(!reporter.is_sending());
    panic_in_a_thread("off again");
    capture_an_error();
    flush();
    assert_eq!(
        settle(&seen, 1, Duration::from_secs(2)),
        1,
        "a request reached the server after consent was withdrawn: {:?}",
        seen.lock().unwrap()
    );

    // 4. And given again, the same client sends again (one more envelope).
    reporter.set_consent(crash::consent_of(Some(true)));
    capture_an_error();
    flush();
    assert_eq!(settle(&seen, 2, Duration::from_secs(2)), 2);
    let (_, body) = seen.lock().unwrap()[1].clone();
    assert!(body.contains("/home/<user>/secret.toml"), "{body}");
    assert!(!body.contains("alice"), "{body}");

    reporter.shutdown();
}

#[test]
fn the_bundle_id_is_the_one_tauri_conf_json_ships() {
    let conf: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses");
    assert_eq!(conf["identifier"], crash::BUNDLE_ID);
}

#[test]
fn a_test_build_has_no_crash_report_address() {
    // CI and local test builds never set BRIDGEWATCH_SENTRY_DSN; if this
    // fails, the environment that built the tests is baking an address in.
    assert_eq!(crash::baked_dsn(), None);
    assert_eq!(crash::baked_build(), "dev");
}
