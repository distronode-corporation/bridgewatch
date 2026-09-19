//! Shared helpers for the integration tests.
//!
//! Every test drives the real [`Poller`] over a [`FixtureTransport`], so what is
//! exercised is the shipping code path and not a parallel implementation of it.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bridgewatch_core::client::{ClientError, FixtureTransport, GitLabClient, RequestRing};
use bridgewatch_core::config::edit::{ConfigEditor, Edit};
use bridgewatch_core::config::{self, Config};
use bridgewatch_core::poll::Poller;
use bridgewatch_core::token::Secret;

/// Everything a scoped subscriber wrote.
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

/// Run `f` with everything logged at `level` or louder captured, and nothing
/// installed globally.
///
/// ⚠ `with_default` is thread-scoped, so anything asynchronous has to be
/// POLLED inside the closure: a subscriber set here and a `block_on` outside it
/// would capture nothing, and the assertions would read as a missing log line
/// rather than as a broken harness.
pub fn with_log<T>(level: tracing::Level, f: impl FnOnce() -> T) -> (T, String) {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let out = tracing::subscriber::with_default(subscriber, f);
    (out, captured.text())
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
    let mut clients = BTreeMap::new();
    for (name, account) in &config.accounts {
        clients.insert(
            name.clone(),
            GitLabClient::new(
                account,
                &Secret::new("fixture-token"),
                transport.clone(),
                ring.clone(),
            ),
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
    let mut clients = BTreeMap::new();
    for (name, account) in &config.accounts {
        clients.insert(
            name.clone(),
            GitLabClient::new(
                account,
                &Secret::new("fixture-token"),
                transport.clone(),
                ring.clone(),
            ),
        );
    }
    Poller::with_clients(config, clients, ring).expect("poller builds")
}
