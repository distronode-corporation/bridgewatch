//! Shared helpers for the integration tests.
//!
//! Every test drives the real [`Poller`] over a [`FixtureTransport`], so what is
//! exercised is the shipping code path and not a parallel implementation of it.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bridgewatch_core::client::{
    CiClient, ClientError, FixtureTransport, GitLabClient, RequestRing,
};
use bridgewatch_core::config::edit::{ConfigEditor, Edit};
use bridgewatch_core::config::{self, Config};
use bridgewatch_core::poll::Poller;
use bridgewatch_core::token::Secret;

/// Everything the binary's one subscriber has written so far.
#[derive(Clone, Default)]
pub struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    /// What has been written so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

/// The one subscriber a log-capturing test binary ever installs, and the buffer
/// it writes into.
///
/// ⛔ It is GLOBAL and installed once, where the obvious thing is a scoped
/// subscriber per test, because `tracing` answers "is this callsite enabled"
/// out of two caches that a scoped subscriber cannot keep honest under a
/// parallel test runner:
///
/// * the per-callsite `Interest`, cached in `DefaultCallsite::interest` and
///   read by every `debug!` before anything else. `tracing-core`'s
///   `rebuild_callsite_interest` ends in `interest.unwrap_or_else(
///   Interest::never)`, so a callsite first reached on a thread with NO
///   subscriber is cached as `never` for the life of the process, and the macro
///   then short-circuits on `!interest.is_never()` without ever consulting a
///   dispatcher;
/// * the process-wide max level hint, `LevelFilter::set_max` called from
///   `Callsites::rebuild_interest`, which is recomputed over whichever
///   dispatchers are registered at that instant.
///
/// `tracing::subscriber::with_default` is THREAD-scoped and builds a fresh
/// `Dispatch` per call, so a binary whose other tests drive the same code with
/// no subscriber races the one test that installs one. That is not theoretical:
/// on Linux `the_request_debug_line_never_carries_the_token` captured NOTHING
/// on 12 of 12 runs, and its three `assert!(!log.contains(..))` lines all passed
/// vacuously against the empty string.
///
/// One `Dispatch`, created once, settles both caches by construction:
/// `Dispatch::new` calls `callsite::register_dispatch`, which rebuilds the
/// interest of every callsite already registered and sets the max level; and
/// because it is the only `Dispatch` the process ever builds, every later
/// registration is computed against it from whichever thread gets there first.
///
/// ⛔ The remaining half of the guarantee is not in this file: a callsite
/// reached BEFORE the install still caches `never`, and the rebuild that would
/// fix it can lose the race to the registering thread's own store. So a binary
/// that captures logs must contain ONLY tests that go through [`with_log`] (or
/// [`start_capture`]) first, which is why the capturing tests live in test
/// files of their own.
static CAPTURE: std::sync::OnceLock<Captured> = std::sync::OnceLock::new();

/// Distinguishes one [`with_log`] call's lines from another's.
static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

fn capture() -> &'static Captured {
    CAPTURE.get_or_init(|| {
        let captured = Captured::default();
        let writer = captured.clone();
        // TRACE and no filter: the level a line was logged at is then a
        // property of the line, which is what a test asserting "this is debug
        // and not info" reads. Filtering here instead would put that assertion
        // back on a subscriber, which is the thing that cannot be scoped.
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("the only subscriber this test binary installs");
        captured
    })
}

/// Install the capturing subscriber for setup that runs OUTSIDE [`with_log`].
///
/// Nothing a test builds before its first capture logs today, but "it happens
/// not to" is the property that broke here once already: reaching a callsite
/// before the subscriber exists is what poisons the interest cache.
pub fn start_capture() {
    let _ = capture();
}

/// Run `f` against the binary's shared subscriber and return only the lines
/// `f` produced.
///
/// Every test in the binary writes into one buffer, so the selection happens
/// after the fact rather than through a subscriber: each call takes a unique
/// case number, `f` runs inside a span carrying it, and the formatter stamps
/// that span onto every line logged while it is entered.
///
/// ⚠ Anything asynchronous still has to be POLLED inside the closure. The span
/// is entered on this thread, so a `block_on` outside it would produce lines
/// carrying no marker, and they would be filtered out here.
pub fn with_log<T>(f: impl FnOnce() -> T) -> (T, String) {
    let captured = capture();
    let case = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
    let out = tracing::info_span!("bw_case", case).in_scope(f);
    // The closing brace is part of the marker: without it `case=1` also selects
    // `case=10`.
    let marker = format!("bw_case{{case={case}}}");
    let text = captured
        .text()
        .lines()
        .filter(|line| line.contains(&marker))
        .map(|line| format!("{line}\n"))
        .collect();
    (out, text)
}

/// The level `tracing` recorded on the first captured line containing `needle`.
///
/// ⛔ This is how a test proves a line is `debug` rather than `info` now that
/// the subscriber is shared and records everything. Installing two subscribers
/// at two levels and comparing what each saw asserted the same thing about the
/// FILTER; this asserts it about the event, which is where the decision
/// actually lives.
pub fn level_of(log: &str, needle: &str) -> Option<String> {
    let line = log.lines().find(|line| line.contains(needle))?;
    line.split_whitespace()
        .find(|word| matches!(*word, "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE"))
        .map(str::to_string)
}

/// A transport that replays a canned list of responses and records what it was
/// asked for, headers included.
///
/// Shared rather than private to `tests/client.rs` because the log-capturing
/// half of the client's tests lives in `tests/client_log.rs`, in a process of
/// its own, and both halves have to drive the same double or the one that
/// proves the credential never reaches a log line would be proving it about
/// some other code path.
#[derive(Debug, Default)]
pub struct Canned {
    responses: std::sync::Mutex<Vec<(u16, String, Option<String>)>>,
    seen: std::sync::Mutex<Vec<bridgewatch_core::client::HttpRequest>>,
}

impl Canned {
    /// Answer with each `(status, body, next_page)` in turn, then with `200 []`.
    pub fn new(responses: Vec<(u16, String, Option<String>)>) -> Arc<Self> {
        Arc::new(Self {
            responses: std::sync::Mutex::new(responses),
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Every path asked for, in order.
    pub fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.path.clone())
            .collect()
    }

    /// Every header sent, in order.
    pub fn headers(&self) -> Vec<(String, String)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .flat_map(|r| r.headers.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl bridgewatch_core::client::Transport for Canned {
    async fn execute(
        &self,
        request: bridgewatch_core::client::HttpRequest,
    ) -> Result<bridgewatch_core::client::HttpResponse, ClientError> {
        self.seen.lock().unwrap().push(request);
        let mut responses = self.responses.lock().unwrap();
        let (status, body, next_page) = if responses.is_empty() {
            (200, "[]".to_string(), None)
        } else {
            responses.remove(0)
        };
        Ok(bridgewatch_core::client::HttpResponse {
            status,
            body,
            next_page,
            ratelimit_remaining: Some(1999),
            ratelimit_reset: Some(1789669380),
            retry_after: None,
            etag: None,
            link: None,
        })
    }
}

/// A client over `transport`, spelling its credential the way `header` says.
///
/// The token is the same literal in every test so that an assertion about what
/// did NOT leak has something specific to look for.
pub fn client_with(
    transport: Arc<dyn bridgewatch_core::client::Transport>,
    header: bridgewatch_core::config::AuthHeader,
) -> (GitLabClient, RequestRing) {
    let account = bridgewatch_core::config::Account {
        header,
        ..Default::default()
    };
    let ring = RequestRing::new(10);
    (
        GitLabClient::new(
            &account,
            &Secret::new("glpat-SECRET"),
            transport,
            ring.clone(),
        ),
        ring,
    )
}

/// A current-thread runtime, for a test that has to drive a future inside
/// [`with_log`] rather than under `#[tokio::test]`.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

/// The repository's `examples/distronode.toml`, which is also the shipped example.
pub fn example_config_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/distronode.toml")
        .canonicalize()
        .expect("examples/distronode.toml exists")
}

/// The raw text of the example configuration.
pub fn example_config_raw() -> String {
    std::fs::read_to_string(example_config_path()).expect("readable")
}

/// The fixtures directory.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every fixture directory, sorted, identified by holding an `expected.json`.
pub fn fixture_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .expect("fixtures directory exists")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("expected.json").exists())
        .collect();
    dirs.sort();
    dirs
}

/// Load the example configuration, optionally applying a list of edits first.
///
/// Running the variant cases through [`ConfigEditor`] rather than through a
/// second hand-written TOML file means the edit path is exercised by every
/// fixture that has a variant, and that a variant cannot drift from the example.
pub fn config_with(edits: &[Edit]) -> Config {
    let raw = example_config_raw();
    let raw = if edits.is_empty() {
        raw
    } else {
        let mut editor = ConfigEditor::new(&raw).expect("example parses");
        editor.apply(edits).expect("edits apply");
        editor.to_toml()
    };
    config::parse_str(&raw, &example_config_path())
        .expect("example configuration is valid")
        .config
}

/// A poller wired to a fixture directory. No token is resolved and no request
/// leaves the process.
pub fn fixture_poller(config: &Config, dir: &Path) -> (Poller, Arc<FixtureTransport>) {
    let transport = Arc::new(FixtureTransport::load(dir).expect("fixture loads"));
    let ring = RequestRing::new(config.log.keep_requests.max(50));
    let mut clients: BTreeMap<String, Arc<dyn CiClient>> = BTreeMap::new();
    for (name, account) in &config.accounts {
        clients.insert(
            name.clone(),
            Arc::new(GitLabClient::new(
                account,
                &Secret::new("fixture-token"),
                transport.clone(),
                ring.clone(),
            )),
        );
    }
    (
        Poller::with_clients(config, clients, ring).expect("poller builds"),
        transport,
    )
}

/// A transport that answers from a fixture directory until a test tells it not
/// to.
///
/// ⛔ The failures that matter here cannot be recorded. A 429 on the second
/// request of a tick, a 15 s timeout on a child, a list request that does not
/// arrive: each one leaves the poller holding a partial picture, and the whole
/// class of defect is what the engine then *infers* from the gap. A fixture
/// directory can only ever describe a successful tick, so the gap has to be
/// scripted.
#[derive(Debug)]
pub struct ScriptedTransport {
    inner: FixtureTransport,
    rules: std::sync::Mutex<Vec<FailRule>>,
}

#[derive(Debug)]
struct FailRule {
    /// Substring of the request path this rule applies to.
    contains: String,
    /// How many more times it fires. `usize::MAX` is "every time".
    remaining: usize,
    error: ClientError,
}

impl ScriptedTransport {
    /// Wrap a fixture directory.
    pub fn load(dir: &Path) -> Self {
        Self {
            inner: FixtureTransport::load(dir).expect("fixture loads"),
            rules: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Fail every request whose path contains `contains`, `times` times.
    pub fn fail(&self, contains: &str, times: usize, error: ClientError) {
        self.rules.lock().unwrap().push(FailRule {
            contains: contains.to_string(),
            remaining: times,
            error,
        });
    }

    /// Fail every matching request until told otherwise.
    pub fn fail_always(&self, contains: &str, error: ClientError) {
        self.fail(contains, usize::MAX, error);
    }

    /// Every path requested so far, in order.
    pub fn seen(&self) -> Vec<String> {
        self.inner.seen()
    }

    /// Forget the recorded request paths.
    pub fn clear_seen(&self) {
        self.inner.clear_seen()
    }

    /// How many requests matched a path substring.
    pub fn count(&self, contains: &str) -> usize {
        self.seen().iter().filter(|p| p.contains(contains)).count()
    }
}

#[async_trait::async_trait]
impl bridgewatch_core::client::Transport for ScriptedTransport {
    async fn execute(
        &self,
        request: bridgewatch_core::client::HttpRequest,
    ) -> Result<bridgewatch_core::client::HttpResponse, ClientError> {
        let scripted = {
            let mut rules = self.rules.lock().unwrap();
            match rules
                .iter_mut()
                .find(|r| r.remaining > 0 && request.path.contains(&r.contains))
            {
                Some(rule) => {
                    if rule.remaining != usize::MAX {
                        rule.remaining -= 1;
                    }
                    Some(rule.error.clone())
                }
                None => None,
            }
        };
        // The request is recorded either way: the inner transport logs it, and a
        // test asserting that a failed child is asked for AGAIN needs the failed
        // attempt in the list too.
        let answered = self.inner.execute(request).await;
        match scripted {
            Some(e) => Err(e),
            None => answered,
        }
    }
}

/// A poller wired to a transport the test controls.
pub fn poller_with_transport(config: &Config, transport: Arc<ScriptedTransport>) -> Poller {
    let ring = RequestRing::new(config.log.keep_requests.max(50));
    let mut clients: BTreeMap<String, Arc<dyn CiClient>> = BTreeMap::new();
    for (name, account) in &config.accounts {
        clients.insert(
            name.clone(),
            Arc::new(GitLabClient::new(
                account,
                &Secret::new("fixture-token"),
                transport.clone(),
                ring.clone(),
            )),
        );
    }
    Poller::with_clients(config, clients, ring).expect("poller builds")
}
