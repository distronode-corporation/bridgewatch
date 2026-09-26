//! A tick's requests run side by side, and nothing it produces may show it.
//!
//! ⛔ The poller used to await every request in turn, and the popover's rows,
//! the notifications, the first error a watch reports and the debug pane's
//! request ring all came out in the order the loop was written. They now run
//! concurrently (a watch's pipelines, their jobs and bridges, their children,
//! and the watches themselves) and are applied afterwards in that same order.
//! Every test here drives the same tick twice, once over a transport that
//! answers at once and once over one that answers LATER requests FIRST, and
//! requires the two to be indistinguishable. A test that only ever saw
//! in-order answers would pass against a poller that applied results in
//! completion order, because with an instant transport the two orders are the
//! same order.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bridgewatch_core::client::{
    CiClient, ClientError, FixtureTransport, GitLabClient, HttpRequest, HttpResponse,
    MAX_IN_FLIGHT, RequestRing, Transport,
};
use bridgewatch_core::config::Config;
use bridgewatch_core::config::edit::Edit;
use bridgewatch_core::poll::{Poller, Tick};
use bridgewatch_core::token::Secret;

/// Answers every request after a delay that SHRINKS with arrival order, so of
/// any requests in flight together the last one sent is the first answered.
/// Records the order answers were handed back and the most it ever held at once.
#[derive(Debug)]
struct Reversing {
    inner: Arc<dyn Transport>,
    arrivals: AtomicUsize,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    answered: Mutex<Vec<String>>,
}

impl Reversing {
    fn new(inner: Arc<dyn Transport>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            arrivals: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            answered: Mutex::new(Vec::new()),
        })
    }

    fn answered(&self) -> Vec<String> {
        self.answered.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Transport for Reversing {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        let n = self.arrivals.fetch_add(1, Ordering::SeqCst);
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(16 - (n % 16) as u64)).await;
        let path = request.path.clone();
        let result = self.inner.execute(request).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.answered.lock().unwrap().push(path);
        result
    }
}

/// A poller over `transport` with one GitLab client per account, as
/// `support::fixture_poller` builds, and the ring it records into.
fn poller_over(config: &Config, transport: Arc<dyn Transport>) -> (Poller, RequestRing) {
    let ring = RequestRing::new(1000);
    let mut clients: std::collections::BTreeMap<String, Arc<dyn CiClient>> = Default::default();
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
    let poller = Poller::with_clients(config, clients, ring.clone()).expect("poller builds");
    (poller, ring)
}

/// Everything a tick produces that a person or the shell can observe, less
/// the wall-clock fields (`last_poll`, and each ring entry's `at` and `ms`).
fn observable(tick: &Tick, ring: &RequestRing) -> serde_json::Value {
    serde_json::json!({
        "icon": tick.snapshot.icon_state.as_str(),
        "watches": serde_json::to_value(&tick.snapshot.watches).unwrap(),
        "errors": tick.snapshot.errors,
        "notifications": serde_json::to_value(&tick.notifications).unwrap(),
        "ring": ring
            .entries()
            .iter()
            .map(|e| (e.method.clone(), e.path.clone(), e.status, e.error.clone()))
            .collect::<Vec<_>>(),
    })
}

/// Every case of every recorded fixture: the tick over a transport that answers
/// out of order reads exactly as the tick over one that answers at once, down
/// to the order of the request ring.
#[tokio::test]
async fn answers_that_arrive_out_of_order_change_nothing_a_tick_produces() {
    let mut cases = 0usize;
    let mut reordered = 0usize;
    for dir in support::fixture_dirs() {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("expected.json")).unwrap())
                .unwrap();
        for case in expected["cases"].as_array().expect("cases") {
            let overrides: Vec<Edit> =
                serde_json::from_value(case["overrides"].clone()).unwrap_or_default();
            let config = support::config_with(&overrides);
            let label = format!("{name}[{}]", case["name"]);

            let instant: Arc<dyn Transport> = Arc::new(FixtureTransport::load(&dir).unwrap());
            let (mut poller, ring) = poller_over(&config, instant);
            let want = observable(&poller.tick().await, &ring);

            let reversing = Reversing::new(Arc::new(FixtureTransport::load(&dir).unwrap()));
            let (mut poller, ring) = poller_over(&config, reversing.clone());
            let got = observable(&poller.tick().await, &ring);

            assert_eq!(got, want, "{label}");
            let peak = reversing.peak.load(Ordering::SeqCst);
            assert!(
                peak <= MAX_IN_FLIGHT,
                "{label}: {peak} requests in flight on one account"
            );
            let recorded: Vec<String> = ring.entries().iter().map(|e| e.path.clone()).collect();
            if reversing.answered() != recorded {
                reordered += 1;
            }
            cases += 1;
        }
    }
    assert!(cases >= 17, "only {cases} cases ran");
    // ⛔ The assertion above is only worth something if the answers really did
    // come back in another order. If this fails, the transport stopped
    // reordering (or the poller stopped running anything concurrently) and
    // every equality above passed without testing anything.
    assert!(
        reordered * 2 >= cases,
        "answers came back out of order in only {reordered} of {cases} cases"
    );
}

/// Several watches on one account are polled at once, and still never hold
/// more than [`MAX_IN_FLIGHT`] requests open between them.
#[tokio::test]
async fn watches_run_side_by_side_within_one_accounts_bound() {
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let config = support::config_with(&[]);
    assert!(config.watches.len() >= 2, "the example has several watches");
    let reversing = Reversing::new(Arc::new(FixtureTransport::load(&dir).unwrap()));
    let (mut poller, _) = poller_over(&config, reversing.clone());
    poller.tick().await;
    let peak = reversing.peak.load(Ordering::SeqCst);
    assert!(
        (2..=MAX_IN_FLIGHT).contains(&peak),
        "expected concurrency, bounded at {MAX_IN_FLIGHT}; the peak was {peak}"
    );
}

/// When two pipelines fail, the watch reports the first one's error in plan
/// order, whichever failure arrived first, and both are asked for again next
/// tick.
#[tokio::test]
async fn the_first_error_is_the_first_pipelines_and_not_the_first_to_arrive() {
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let config = support::config_with(&[]);

    let scripted = || {
        let t = support::ScriptedTransport::load(&dir);
        t.fail_always(
            "/jobs",
            ClientError::Transport("every jobs request is refused".into()),
        );
        t.fail_always(
            "/bridges",
            ClientError::Transport("every bridges request is refused".into()),
        );
        Arc::new(t)
    };

    let (mut poller, ring) = poller_over(&config, scripted());
    let want = observable(&poller.tick().await, &ring);
    let reversing = Reversing::new(scripted());
    let (mut poller, ring) = poller_over(&config, reversing.clone());
    let got = observable(&poller.tick().await, &ring);

    assert!(
        !want["errors"].as_array().unwrap().is_empty(),
        "the script did fail something: {want:#}"
    );
    assert_eq!(got, want);
}
