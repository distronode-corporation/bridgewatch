//! The poller: one tick fetches what the planner asked for, the verdict engine
//! turns it into a [`Snapshot`], and the notifier says what changed.

pub mod cache;
pub mod planner;
pub mod policy;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

pub use cache::{CacheEntry, PipelineCache};
pub use planner::{PipelinePlan, Plan};
pub use policy::{MAX_RETRY_AFTER, POLL_NOW_MIN_GAP, PollNow, PollPolicy};

use crate::client::{AnnotationAnswer, CiClient, ClientError, ListQuery, RequestRing, in_order};
use crate::config::{Config, JobsMode, ProjectRef, Provider, Role, Watch, WatchRules};
use crate::eta::EtaHistory;
use crate::model::{Bridge, Pipeline, PipelineDetail};
use crate::notify::{Notification, NotifyLedger, notifications_for, token_notifications};
use crate::status::{JobClass, Status};
use crate::verdict::{
    DetailSource, PipelineView, Snapshot, VerdictScript, WatchView, evaluate_pipeline,
};

/// The result of one tick.
#[derive(Debug, Clone)]
pub struct Tick {
    /// What the GUI should draw.
    pub snapshot: Snapshot,
    /// What the shell should deliver.
    pub notifications: Vec<Notification>,
    /// How long to wait before the next tick.
    pub next_interval: std::time::Duration,
    /// Each watch whose LIST request failed this tick, by id, with the error
    /// as it was raised. The view carries the same error as text, for
    /// display; this is for a caller that has to tell a refused token or a
    /// missing project (which no retry will fix) from a timeout or a 503.
    pub list_errors: BTreeMap<String, ClientError>,
    /// The watches whose list request was actually SENT this tick. Not one
    /// sitting out a backoff, and not one whose account is unknown. Whether
    /// it worked is [`Self::list_errors`].
    pub polled: BTreeSet<String>,
}

/// What [`Poller::look_up_commit`] found for one commit on one watch.
#[derive(Debug, Clone)]
pub enum CommitLookup {
    /// The project has no pipeline for this commit that the watch would show
    /// (by ref and source), or no such commit at all. Yet: a fresh push may
    /// still be on its way.
    NotFound,
    /// Its newest pipeline was skipped: it ran nothing, so it has no verdict.
    Skipped(Box<Pipeline>),
    /// Its newest pipeline that ran, judged as a row of the watch would be.
    Found(Box<PipelineView>),
}

/// One watch's runtime state.
struct WatchState {
    watch: Watch,
    rules: WatchRules,
    cache: PipelineCache,
    policy: PollPolicy,
    /// `show.jobs` resolved against `ui.jobs`, once, at build time.
    jobs_mode: JobsMode,
    /// The watch's account's provider, carried into every view. An unknown
    /// account reads as GitLab, the default; that watch shows an error anyway.
    provider: Provider,
    /// The watch's account's `actions`, carried into every view so the
    /// popover offers Retry and Play only where they can be sent.
    actions: bool,
    last_bridges: HashMap<u64, Vec<Bridge>>,
    /// What to add to a "not found" on this watch's list request, when its
    /// account signs in through a GitHub App (see `oauth::not_found_hint`).
    /// `None` for every other account, whose errors read as they always have.
    not_found_hint: Option<String>,
    /// The last view this watch produced.
    ///
    /// ⛔ It is what a failed list request falls back to. Returning no rows
    /// there drops the primary watch's icon to `unknown` for a doubled interval
    /// over a single dropped connection, while the cache still holds every
    /// detail the last tick built — an answer that is both wrong and available.
    last_view: Option<WatchView>,
}

/// Polls every watch and produces snapshots.
///
/// Built either from a configuration ([`Poller::from_config`]) or from
/// pre-made clients ([`Poller::with_clients`]), which is what lets the tests and
/// the offline demo drive it against recorded fixtures.
pub struct Poller {
    clients: BTreeMap<String, Arc<dyn CiClient>>,
    watches: Vec<WatchState>,
    script: Option<Arc<VerdictScript>>,
    ring: RequestRing,
    ledger: NotifyLedger,
    ledger_path: Option<std::path::PathBuf>,
    /// How long each watch's deploys have taken. Learned in the ordered half
    /// of [`Poller::tick`], so the GUI and `bridgewatch check` both learn.
    eta: EtaHistory,
    eta_path: Option<std::path::PathBuf>,
    /// What "now" is for an estimate's elapsed time. The wall clock, except
    /// in a test that has to put a pipeline a known distance into its run.
    clock: Clock,
    sender: tokio::sync::watch::Sender<Snapshot>,
    /// Whether anything was in flight last tick. The backoff gate needs an
    /// interval before it knows this tick's answer, and last tick's is the only
    /// honest guess.
    last_any_live: bool,
    /// The accounts [`Poller::from_config_per_account`] built a refusing
    /// client for, because their credential failed. Empty otherwise.
    refused: Vec<String>,
    /// What [`Poller::retry_refused`] needs to build one account's client
    /// again. `None` for a poller built from ready clients.
    retry: Option<Retry>,
    /// The accounts whose token's expiry is still to be asked for: GitLab
    /// tokens, whose `expires_at` costs one request (F3). Each is asked once
    /// and taken off; one whose answer was "later" (a rate limit, a 5xx, the
    /// network) is asked again next tick.
    expiry_due: BTreeSet<String>,
    /// Each account's [`crate::health::Observed::accepted`] count as of the
    /// last tick, to tell whether an answer accepted it since.
    accepted_seen: HashMap<String, u64>,
}

/// How a client's transport is made, per account: the network's
/// ([`ReqwestTransport`](crate::client::ReqwestTransport) on the account's
/// timeout), or one a test supplies.
type MakeTransport = Arc<
    dyn Fn(&crate::config::Account) -> Result<Arc<dyn crate::client::Transport>, ClientError>
        + Send
        + Sync,
>;

/// Everything needed to build a refused account's client again, in place.
struct Retry {
    /// The polled accounts, as the configuration said when the poller was
    /// built. A retry never reads the file: a changed file is a reload.
    accounts: BTreeMap<String, crate::config::Account>,
    store: Arc<dyn crate::token::TokenProvider>,
    transport: MakeTransport,
}

impl Poller {
    /// Build a poller from a configuration and a set of ready clients, keyed by
    /// account name.
    pub fn with_clients(
        config: &Config,
        clients: BTreeMap<String, Arc<dyn CiClient>>,
        ring: RequestRing,
    ) -> Result<Self, crate::config::ConfigError> {
        let mut watches = Vec::new();
        for watch in &config.watches {
            watches.push(WatchState {
                rules: WatchRules::compile(watch)?,
                policy: PollPolicy::new(
                    &watch.poll,
                    &config
                        .accounts
                        .get(&watch.account)
                        .map(|a| a.rate_limit_backoff.clone())
                        .unwrap_or_default(),
                ),
                jobs_mode: watch.effective_jobs(&config.ui),
                provider: config
                    .accounts
                    .get(&watch.account)
                    .map(|a| a.provider)
                    .unwrap_or_default(),
                actions: config
                    .accounts
                    .get(&watch.account)
                    .is_some_and(|a| a.actions),
                not_found_hint: config.accounts.get(&watch.account).and_then(|a| {
                    crate::oauth::not_found_hint(a, &crate::oauth::BuiltinClients::shipped())
                }),
                watch: watch.clone(),
                cache: PipelineCache::new(),
                last_bridges: HashMap::new(),
                last_view: None,
            });
        }

        let script = match (&config.verdict.script_source, &config.verdict.script) {
            (Some(source), _) => Some(Arc::new(
                VerdictScript::from_source(source)
                    .map_err(|e| crate::config::ConfigError::Pattern(e.to_string()))?,
            )),
            (None, Some(path)) => Some(Arc::new(
                VerdictScript::from_path(std::path::Path::new(path))
                    .map_err(|e| crate::config::ConfigError::Pattern(e.to_string()))?,
            )),
            (None, None) => None,
        };

        let (sender, _) = tokio::sync::watch::channel(Snapshot::empty());

        Ok(Self {
            clients,
            watches,
            script,
            ring,
            ledger: NotifyLedger::default(),
            ledger_path: None,
            eta: EtaHistory::default(),
            eta_path: None,
            clock: Arc::new(chrono::Utc::now),
            sender,
            last_any_live: false,
            refused: Vec::new(),
            retry: None,
            expiry_due: BTreeSet::new(),
            accepted_seen: HashMap::new(),
        })
    }

    /// Build a poller from a configuration, resolving the token of each account
    /// a watch uses and creating a real HTTP transport for it.
    ///
    /// ⚠️ An account that signs in (`token = { oauth = .. }`) resolves nothing
    /// here: its client is built on an [`crate::oauth::OAuthTransport`], which
    /// reads the stored sign-in on the first request and keeps it fresh. So an
    /// account that is not signed in yet is an error on ITS watches ("sign in
    /// again") and does not stop the other accounts polling. The credential
    /// store is kept for the life of the poller, for the refreshes, which is
    /// why `provider` is taken by a clonable value.
    pub fn from_config<P>(config: &Config, provider: &P) -> Result<Self, PollerError>
    where
        P: crate::token::TokenProvider + Clone + 'static,
    {
        Self::build(config, provider, false, network_transport())
    }

    /// [`Poller::from_config`], where an account whose token cannot be
    /// resolved fails only its own watches: the app's poller.
    ///
    /// ⛔ D4. One account's token error used to stop the whole poller: the
    /// icon went to unknown, every account stopped, and the strip read
    /// "cannot start polling". Now that account gets a client whose every
    /// request is refused with [`ClientError::Token`] before anything is
    /// sent, so its watches carry the error and the others keep polling, as
    /// an account that is not signed in always has. The CLI keeps
    /// [`Poller::from_config`], where the same error is exit 70.
    pub fn from_config_per_account<P>(config: &Config, provider: &P) -> Result<Self, PollerError>
    where
        P: crate::token::TokenProvider + Clone + 'static,
    {
        Self::build(config, provider, true, network_transport())
    }

    /// [`Poller::from_config_per_account`] over one transport the caller
    /// supplies for every account: a script, in a test.
    pub fn from_config_per_account_on<P>(
        config: &Config,
        provider: &P,
        transport: Arc<dyn crate::client::Transport>,
    ) -> Result<Self, PollerError>
    where
        P: crate::token::TokenProvider + Clone + 'static,
    {
        Self::build(
            config,
            provider,
            true,
            Arc::new(move |_: &crate::config::Account| Ok(transport.clone())),
        )
    }

    fn build<P>(
        config: &Config,
        provider: &P,
        per_account: bool,
        transport: MakeTransport,
    ) -> Result<Self, PollerError>
    where
        P: crate::token::TokenProvider + Clone + 'static,
    {
        let ring = RequestRing::new(config.log.keep_requests);
        let mut clients = BTreeMap::new();
        let store: Arc<dyn crate::token::TokenProvider> = Arc::new(provider.clone());
        // ⛔ Only the accounts a watch polls through. Every configured account
        // used to be resolved, so `bridgewatch check --watch x` read the
        // keychain (or ran the token command, or raised a macOS prompt) for
        // accounts `x` never touches, one after another, and an account with a
        // broken token failed a check that did not need it. An account no
        // watch names has no client, which is exactly what a watch naming an
        // account that does not exist already gets.
        let used: HashSet<&str> = config.watches.iter().map(|w| w.account.as_str()).collect();
        let mut refused = Vec::new();
        let mut polled = BTreeMap::new();
        for (name, account) in config
            .accounts
            .iter()
            .filter(|(name, _)| used.contains(name.as_str()))
        {
            let made = account_client(name, account, &store, &ring, &transport);
            let client = match made {
                Ok(client) => client,
                Err(failure) if per_account => {
                    let client = refusal(name, account, failure, &ring)?;
                    refused.push(name.clone());
                    client
                }
                Err(e) => return Err(e),
            };
            clients.insert(name.clone(), client);
            polled.insert(name.clone(), account.clone());
        }
        let mut poller = Self::with_clients(config, clients, ring).map_err(PollerError::Config)?;
        poller.expiry_due = poller
            .clients
            .keys()
            .filter(|name| !refused.contains(name))
            .filter(|name| {
                config
                    .accounts
                    .get(*name)
                    .is_some_and(expiry_costs_a_request)
            })
            .cloned()
            .collect();
        poller.refused = refused;
        poller.retry = per_account.then_some(Retry {
            accounts: polled,
            store,
            transport,
        });
        Ok(poller)
    }

    /// The accounts whose credential failed, and whose every request is
    /// refused: since the poller was built, or since the last
    /// [`Poller::retry_refused`] that still could not resolve them.
    pub fn refused_accounts(&self) -> &[String] {
        &self.refused
    }

    /// Resolve the credential of each refused account again, and swap in the
    /// real client of every one that now resolves.
    ///
    /// ⛔ In place. Every watch keeps its state (backoff, `retry_after`, the
    /// pipeline cache and the last view), the healthy accounts' and the
    /// recovered account's alike; only a client changes. Some causes of a
    /// refusal pass on their own (a locked keychain, a Secret Service not up
    /// yet at login, a command that timed out, a token saved since), and
    /// rebuilding the whole poller for them wiped every other account's
    /// backoff and cache each time. An account still refused keeps a refusing
    /// client, with the latest reason, and sends nothing.
    ///
    /// Returns the accounts that recovered. ⚠️ Blocking, like building: it
    /// reads the credential store or runs a token command.
    pub fn retry_refused(&mut self) -> Vec<String> {
        let Some(retry) = &self.retry else {
            return Vec::new();
        };
        let mut recovered = Vec::new();
        let mut still = Vec::new();
        for name in std::mem::take(&mut self.refused) {
            let Some(account) = retry.accounts.get(&name) else {
                continue;
            };
            let made = account_client(&name, account, &retry.store, &self.ring, &retry.transport);
            let client = match made {
                Ok(client) => {
                    tracing::info!(account = %name, "the account's credential resolves again");
                    recovered.push(name.clone());
                    client
                }
                Err(failure) => match refusal(&name, account, failure, &self.ring) {
                    Ok(client) => {
                        still.push(name.clone());
                        client
                    }
                    // Not a credential failure (a transport that cannot be
                    // built): keep the refusing client already in place.
                    _ => {
                        still.push(name);
                        continue;
                    }
                },
            };
            self.clients.insert(name, client);
        }
        self.refused = still;
        // ⚠ The one thing a recovery changes besides the client: the
        // recovered account's watches backed off on refusals the poller made
        // itself, which no server asked for, so that backoff is forgotten and
        // they poll on the next tick. Their cache and last view are kept, and
        // every other watch is untouched.
        for state in &mut self.watches {
            if recovered.contains(&state.watch.account) {
                state.policy.forget_refusals();
            }
        }
        for name in &recovered {
            if retry.accounts.get(name).is_some_and(expiry_costs_a_request) {
                self.expiry_due.insert(name.clone());
            }
        }
        recovered
    }

    /// The client each account polls through, by account name.
    ///
    /// Handed out so a job's log and a job action go through the SAME client
    /// the account's watches use: one in-flight bound per account, one ring,
    /// and for a signed-in account one session, so a refresh the poller made
    /// is the one the action sends.
    pub fn clients(&self) -> &BTreeMap<String, Arc<dyn CiClient>> {
        &self.clients
    }

    /// Load and persist the notification ledger at this path.
    pub fn with_ledger(mut self, path: std::path::PathBuf) -> Self {
        self.ledger = NotifyLedger::load(&path);
        self.ledger_path = Some(path);
        self
    }

    /// Load and persist the deploy-time history at this path.
    ///
    /// Without it the poller still learns, in memory, for as long as it lives.
    /// A fixture run should not call this: a rehearsal must not teach the real
    /// history how long a recorded pipeline took.
    pub fn with_eta_history(mut self, path: std::path::PathBuf) -> Self {
        self.eta = EtaHistory::load(&path);
        self.eta_path = Some(path);
        self
    }

    /// Read the time from `clock` rather than the wall. Only an estimate's
    /// elapsed time reads it.
    pub fn with_clock(
        mut self,
        clock: impl Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync + 'static,
    ) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// The deploy-time history, mostly for tests.
    pub fn eta_history(&self) -> &EtaHistory {
        &self.eta
    }

    /// Subscribe to snapshots. Each tick publishes one.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Snapshot> {
        self.sender.subscribe()
    }

    /// The request ring, for the debug pane.
    pub fn ring(&self) -> &RequestRing {
        &self.ring
    }

    /// The notification ledger, mostly for tests.
    pub fn ledger(&self) -> &NotifyLedger {
        &self.ledger
    }

    /// Run one tick across every watch.
    pub async fn tick(&mut self) -> Tick {
        let mut views = Vec::with_capacity(self.watches.len());
        let mut notifications = Vec::new();
        let mut errors = Vec::new();
        let mut list_errors = BTreeMap::new();
        let mut polled_ids = BTreeSet::new();

        // What each watch does this tick, decided before anything is sent.
        enum Turn {
            UnknownAccount,
            SitOut,
            Poll,
        }
        let turns: Vec<Turn> = self
            .watches
            .iter()
            .map(|state| {
                if !self.clients.contains_key(&state.watch.account) {
                    Turn::UnknownAccount
                } else if state.policy.should_defer(self.last_any_live) {
                    // A watch that is backing off sits the tick out. The sleep
                    // between ticks is the `min` over every watch, so a healthy
                    // watch on a fast interval would otherwise drag a 429'd one
                    // back to the API at its cadence and make `Retry-After`
                    // decorative. Nothing changed for this watch, so nothing is
                    // notified either.
                    Turn::SitOut
                } else {
                    Turn::Poll
                }
            })
            .collect();

        // The per-account lookups of this tick (F3), before the watches run:
        // a GitLab token's expiry (once per account, so the first tick knows
        // it), and who each account's `mine` watches are held to (once per
        // account per tick, so a failing `/user` is one failure for all of
        // them, cubic #66). Only for an account with a watch polling this
        // tick: one sitting out a backoff is asked nothing extra.
        //
        // ⛔ Side by side across accounts (cubic, #67), through `in_order`, so
        // the request log records them in account order whatever answers
        // first, and the results are applied in that order after the join.
        let polling: BTreeSet<&str> = self
            .watches
            .iter()
            .zip(&turns)
            .filter(|(_, turn)| matches!(turn, Turn::Poll))
            .map(|(state, _)| state.watch.account.as_str())
            .collect();
        let wants_owner: BTreeSet<&str> = self
            .watches
            .iter()
            .zip(&turns)
            .filter(|(state, turn)| state.watch.mine && matches!(turn, Turn::Poll))
            .map(|(state, _)| state.watch.account.as_str())
            .collect();
        enum Lookup {
            Expiry(String, Result<(), ClientError>),
            Owner(String, Result<String, ClientError>),
        }
        type LookupFuture<'a> =
            std::pin::Pin<Box<dyn std::future::Future<Output = Lookup> + Send + 'a>>;
        let mut lookups: Vec<LookupFuture<'_>> = Vec::new();
        for (name, client) in &self.clients {
            if self.expiry_due.contains(name) && polling.contains(name.as_str()) {
                let name = name.clone();
                lookups.push(Box::pin(async move {
                    let answer = client.read_token_expiry().await;
                    Lookup::Expiry(name, answer)
                }));
            }
            if wants_owner.contains(name.as_str()) {
                let name = name.clone();
                lookups.push(Box::pin(async move {
                    let answer = client.owner().await;
                    Lookup::Owner(name, answer)
                }));
            }
        }
        let mut owners: HashMap<String, Result<String, ClientError>> = HashMap::new();
        for lookup in in_order(lookups).await {
            match lookup {
                Lookup::Expiry(name, Err(e)) if e.should_back_off() => {
                    tracing::debug!(account = %name, error = %e, "the token's expiry was not read; asked again later");
                }
                Lookup::Expiry(name, Err(e)) => {
                    tracing::debug!(account = %name, error = %e, "the token's expiry cannot be read; not asked again");
                    self.expiry_due.remove(&name);
                }
                Lookup::Expiry(name, Ok(())) => {
                    self.expiry_due.remove(&name);
                }
                Lookup::Owner(name, answer) => {
                    owners.insert(name, answer);
                }
            }
        }
        let owners = &owners;

        // ⛔ Every polled watch runs at once, and everything a watch's result
        // touches outside its own state (the errors, the transition log, the
        // notifications and the ledger they are recorded in) is applied
        // afterwards, one watch at a time in configuration order. That order is
        // what the popover's rows, the notification order and the debug pane's
        // ring all follow; it must not become the order the responses arrived
        // in. Two watches on one account share that client's in-flight bound.

        let clients = &self.clients;
        let script = self.script.as_deref();
        let polled = in_order(
            self.watches
                .iter_mut()
                .zip(&turns)
                .filter(|(_, turn)| matches!(turn, Turn::Poll))
                .filter_map(|(state, _)| {
                    let client = clients.get(&state.watch.account)?;
                    let owner = state
                        .watch
                        .mine
                        .then(|| owners.get(&state.watch.account).cloned())
                        .flatten();
                    Some(async move { poll_watch(client.as_ref(), state, script, owner).await })
                }),
        )
        .await;
        let mut polled = polled.into_iter();
        // One clock for the whole apply phase, so two rows of one tick cannot
        // disagree about what time it is.
        let now = (self.clock)();
        let mut learned = false;

        for (state, turn) in self.watches.iter_mut().zip(turns) {
            match turn {
                Turn::UnknownAccount => {
                    errors.push(format!(
                        "watch {:?} refers to unknown account {:?}",
                        state.watch.id, state.watch.account
                    ));
                    views.push(WatchView {
                        id: state.watch.id.clone(),
                        role: state.watch.role,
                        icon_state: None,
                        rows: Vec::new(),
                        error: Some(format!("unknown account {:?}", state.watch.account)),
                        jobs: state.jobs_mode,
                        provider: state.provider,
                        actions: state.actions,
                    });
                }
                Turn::SitOut => {
                    if let Some(view) = &state.last_view {
                        if let Some(e) = &view.error {
                            errors.push(format!("{}: {e}", state.watch.id));
                        }
                        // Nothing was fetched, but time still passed: the
                        // elapsed half of an estimate is re-read.
                        let mut view = view.clone();
                        annotate_eta(&self.eta, state, &mut view, now);
                        views.push(view);
                    }
                }
                Turn::Poll => {
                    let Some((mut view, list_error, sent)) = polled.next() else {
                        continue;
                    };
                    // Only when the list request went out: a `mine` watch
                    // whose owner could not be read sent nothing.
                    if sent {
                        polled_ids.insert(state.watch.id.clone());
                    }
                    if let Some(e) = list_error {
                        list_errors.insert(state.watch.id.clone(), e);
                    }
                    // ⛔ Here, in configuration order, and never inside
                    // `poll_watch`: the history is shared by every watch, and
                    // the concurrent half may touch nothing but its own
                    // watch's state. Learning before annotating is what lets
                    // the tick that records the third sample show the first
                    // estimate.
                    learned |= learn_eta(&mut self.eta, state, &view);
                    annotate_eta(&self.eta, state, &mut view, now);
                    if let Some(e) = &view.error {
                        errors.push(format!("{}: {e}", state.watch.id));
                    }
                    log_transition(&state.watch.id, state.last_view.as_ref(), &view);
                    notifications.extend(notifications_for(&state.watch, &view, &mut self.ledger));
                    state.last_view = Some(view.clone());
                    views.push(view);
                }
            }
        }

        // Token health (F3), from what this tick's and earlier requests showed:
        // no request of its own. A refused account (its credential never
        // resolved) has sent nothing and says it on its watches already.
        let now_secs = now.timestamp();
        let mut tokens = Vec::new();
        let mut healthy = Vec::new();
        for (name, client) in &self.clients {
            if self.refused.contains(name) {
                continue;
            }
            let observed = client.credential();
            // Healthy for the ledger only when an answer ACCEPTED the
            // credential this tick, even while an expiry warns (cubic, #66).
            // ⛔ Not merely "no refusal held": an account that sent nothing,
            // sat out a backoff or lost its connection has said nothing, and
            // clearing its persisted refusal would announce it again on the
            // next 401 (Seer and cubic, #67).
            let seen = self.accepted_seen.insert(name.clone(), observed.accepted);
            if observed.refused.is_none() && seen.unwrap_or(0) < observed.accepted {
                healthy.push(name.clone());
            }
            if let Some(health) = crate::health::assess(name, &observed, now_secs) {
                tokens.push(health);
            }
        }
        notifications.extend(token_notifications(
            &tokens,
            &healthy,
            &mut self.ledger,
            now_secs,
        ));

        // Only when something was recorded: an idle tick leaves the file alone.
        if let Some(path) = &self.ledger_path
            && let Err(e) = self.ledger.save_if_changed(path)
        {
            tracing::warn!(error = %e, "could not persist the notification ledger");
        }
        // Only when a sample arrived, which is once per deployed pipeline, so
        // a `check` against a settled estate reads the file once and writes
        // nothing.
        if learned
            && let Some(path) = &self.eta_path
            && let Err(e) = self.eta.save_merged(path)
        {
            tracing::warn!(error = %e, "could not persist the deploy-time history");
        }

        let snapshot = Snapshot {
            icon_state: Snapshot::icon_from_watches(&views),
            watches: views,
            errors,
            last_poll: chrono::Utc::now(),
            request_log: self.ring.entries(),
            tokens,
        };

        let any_live = snapshot.any_live();
        self.last_any_live = any_live;
        let next_interval = self
            .watches
            .iter()
            .map(|s| s.policy.interval(any_live))
            .min()
            .unwrap_or(std::time::Duration::from_secs(60));

        // Per tick, so `debug` and not `info`: at the live interval this is a
        // line every few seconds.
        tracing::debug!(
            icon = %snapshot.icon_state,
            watches = snapshot.watches.len(),
            rows = snapshot.watches.iter().map(|w| w.rows.len()).sum::<usize>(),
            errors = snapshot.errors.len(),
            live = any_live,
            next_secs = next_interval.as_secs(),
            "tick complete"
        );

        let _ = self.sender.send(snapshot.clone());

        Tick {
            snapshot,
            notifications,
            next_interval,
            list_errors,
            polled: polled_ids,
        }
    }

    /// Look one commit's pipelines up DIRECTLY, for `bridgewatch wait`: the
    /// commit a watch's rows no longer hold (only the newest settled ones are
    /// kept, and a skipped pipeline never is), or not yet.
    ///
    /// `sha` may be abbreviated; it is expanded through the provider first,
    /// because both match a pipeline's sha in full only. The pipelines are
    /// held to the watch's ref and sources, as its rows are, and the newest
    /// one that ran is judged exactly as a row would be: same detail fetch,
    /// same rules, same script.
    ///
    /// ⚠️ Read-only: nothing it fetches enters the watch's cache, its backoff
    /// or the ledger, so a lookup cannot change what the next tick shows.
    ///
    /// # Errors
    ///
    /// The first request that failed, as raised. An unknown watch or account
    /// is [`ClientError::Unsupported`].
    pub async fn look_up_commit(
        &self,
        watch: &str,
        sha: &str,
    ) -> Result<CommitLookup, ClientError> {
        let unsupported = |message: String| ClientError::Unsupported { message };
        let state = self
            .watches
            .iter()
            .find(|s| s.watch.id == watch)
            .ok_or_else(|| unsupported(format!("no watch {watch:?}")))?;
        let client = self
            .clients
            .get(&state.watch.account)
            .ok_or_else(|| unsupported(format!("unknown account {:?}", state.watch.account)))?;
        let project = &state.watch.project;
        let full = if sha.len() >= 40 {
            sha.to_ascii_lowercase()
        } else {
            match client.commit_sha(project, sha).await? {
                Some(full) => full,
                None => return Ok(CommitLookup::NotFound),
            }
        };

        let query = mine(
            client.as_ref(),
            &state.watch,
            list_query(&state.watch, &state.rules),
        )
        .await?
        .for_sha(&full);
        let mut rows: Vec<Pipeline> = client
            .list_pipelines(project, &query)
            .await?
            .into_iter()
            // Held to the commit here too: a recorded fixture answers every
            // list with the same page, whatever it asked for.
            .filter(|p| p.sha.eq_ignore_ascii_case(&full))
            .filter(|p| state.rules.ref_matcher.matches(&p.ref_name))
            .filter(|p| WatchRules::source_matches(&state.watch.sources, p.source.as_deref()))
            .collect();
        rows.sort_by_key(|p| std::cmp::Reverse(p.id));

        let Some(row) = rows.iter().find(|p| p.status != Status::Skipped) else {
            return Ok(match rows.into_iter().next() {
                Some(skipped) => CommitLookup::Skipped(Box::new(skipped)),
                None => CommitLookup::NotFound,
            });
        };
        let fetched = fetch_detail(client.as_ref(), project, row, &query, state).await?;
        // A pipeline missing a child is not judged: "nothing failed" from a
        // list that is not all there is the B1 mistake. The caller asks again.
        if let Some(e) = fetched.errors.into_iter().next() {
            return Err(e);
        }
        let (view, _) = evaluate_pipeline(
            &fetched.detail,
            DetailSource::Fetched,
            &state.watch,
            &state.rules,
            self.script.as_deref(),
        );
        Ok(CommitLookup::Found(Box::new(view)))
    }

    /// Poll forever, sleeping the interval each tick asks for.
    ///
    /// `on_tick` receives every tick, which is where a shell delivers
    /// notifications. Returning `false` stops the loop.
    pub async fn run<F>(&mut self, mut on_tick: F)
    where
        F: FnMut(&Tick) -> bool,
    {
        loop {
            let tick = self.tick().await;
            if !on_tick(&tick) {
                return;
            }
            tokio::time::sleep(tick.next_interval).await;
        }
    }
}

/// The poller's clock. See [`Poller::with_clock`].
type Clock = Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>;

/// Record a sample for every row of this view whose deploy is `live` and that
/// has not been measured yet. Returns whether anything was recorded.
///
/// The marker's `finished_at` is not on the view (it would change the bytes of
/// every `check --json` with a deploy in it), so it is read from the cached
/// detail the view was built from. The job is found by id across the parent
/// and every child, because on a GitHub commit group the row's own id is also
/// its newest run's, and the pipeline id alone cannot say which list holds it.
fn learn_eta(history: &mut EtaHistory, state: &WatchState, view: &WatchView) -> bool {
    if !state.watch.eta || state.watch.deploy_markers.is_empty() {
        return false;
    }
    let mut learned = false;
    for row in &view.rows {
        let Some(marker) = &row.deploy_marker else {
            continue;
        };
        if history.contains(&state.watch.id, row.id) {
            continue;
        }
        let Some(entry) = state.cache.get(row.id) else {
            continue;
        };
        let finished_at = entry
            .detail
            .jobs
            .iter()
            .chain(entry.detail.child_jobs.values().flatten())
            .find(|job| job.id == marker.id)
            .and_then(|job| job.finished_at.as_deref());
        let Some(secs) = crate::eta::deploy_sample(row.created_at.as_deref(), finished_at) else {
            continue;
        };
        if history.record(&state.watch.id, row.id, secs) {
            tracing::debug!(
                watch = %state.watch.id,
                pipeline = row.id,
                secs,
                "recorded a deploy time"
            );
            learned = true;
        }
    }
    learned
}

/// Put an estimate on every running row of this view, and take it off every
/// other row. See [`crate::eta::estimate`] for when there is none.
fn annotate_eta(
    history: &EtaHistory,
    state: &WatchState,
    view: &mut WatchView,
    now: chrono::DateTime<chrono::Utc>,
) {
    let has_markers = !state.watch.deploy_markers.is_empty();
    let samples = history.samples(&state.watch.id);
    for row in &mut view.rows {
        row.eta = if state.watch.eta && crate::eta::is_running(row, has_markers) {
            crate::eta::elapsed_since(row.created_at.as_deref(), now)
                .and_then(|elapsed| crate::eta::estimate(&samples, elapsed))
        } else {
            None
        };
    }
}

/// Log a watch whose verdict moved, at `info`.
///
/// This is the level's whole purpose: `info` is "what a person debugging a
/// wrong tray needs, at human frequency", and a verdict changing is the only
/// thing that happens at human frequency. Everything per-tick and per-request
/// is `debug`.
///
/// ⚠ The transition is taken from the ROWS ([`WatchView::row_state`]) rather
/// than from `icon_state`, so a secondary watch is logged too. Its verdict
/// never reaches the tray, but it is on screen in the popover, and "the hourly
/// schedule went red" is exactly the line somebody is looking for.
/// The first tick reports `-> <state>`: a watch that has just started has no
/// previous verdict, and saying so beats inventing `unknown` as one.
fn log_transition(id: &str, previous: Option<&WatchView>, current: &WatchView) {
    let before = previous.and_then(WatchView::row_state);
    let after = current.row_state();
    if before == after {
        return;
    }
    let name = |s: Option<crate::verdict::IconState>| {
        s.map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "-".to_string())
    };
    tracing::info!(
        watch = id,
        from = name(before),
        to = name(after),
        "watch verdict changed"
    );
}

/// Why a poller could not be built.
#[derive(Debug, thiserror::Error)]
pub enum PollerError {
    /// An account's token could not be resolved.
    #[error("account {0}: {1}")]
    Token(String, #[source] crate::token::TokenError),
    /// An account that signs in names no application to sign in through.
    #[error("account {0}: {1}")]
    SignIn(String, #[source] crate::oauth::OAuthError),
    /// A transport could not be created.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The configuration does not compile.
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
}

/// Poll one watch: list, plan, fetch, evaluate.
async fn poll_watch(
    client: &dyn CiClient,
    state: &mut WatchState,
    script: Option<&VerdictScript>,
    owner: Option<Result<String, ClientError>>,
) -> (WatchView, Option<ClientError>, bool) {
    let project = state.watch.project.clone();
    let query = list_query(&state.watch, &state.rules);
    // `owner` is `Some` for a `mine` watch only: who to hold the list to,
    // or why that could not be said (and then no list is sent).
    let query = match owner {
        None => Ok(query),
        Some(Ok(login)) => Ok(query.for_user(Some(&login))),
        Some(Err(e)) => Err(e),
    };
    let sent = query.is_ok();
    let listed = match query {
        Ok(query) => client
            .list_pipelines(&project, &query)
            .await
            .map(|rows| (rows, query)),
        Err(e) => Err(e),
    };

    let (rows, query) = match listed {
        Ok(listed) => listed,
        Err(e) => {
            state.policy.on_error(&e);
            let message = match (&e, &state.not_found_hint) {
                (ClientError::NotFound { .. }, Some(hint)) => format!("{e}. {hint}"),
                _ => e.to_string(),
            };
            // Keep the last frame rather than blanking the watch. The cache is
            // intact, so the only thing this tick learned is that one request
            // did not arrive, and the honest way to say that is an error
            // alongside the rows — not `unknown` where a verdict already exists.
            let view = match &state.last_view {
                Some(previous) => WatchView {
                    error: Some(message),
                    ..previous.clone()
                },
                None => WatchView {
                    id: state.watch.id.clone(),
                    role: state.watch.role,
                    icon_state: None,
                    rows: Vec::new(),
                    error: Some(message),
                    jobs: state.jobs_mode,
                    provider: state.provider,
                    actions: state.actions,
                },
            };
            return (view, Some(e), sent);
        }
    };
    state.policy.on_success();

    let selected = select_rows(&rows, &state.watch, &state.rules);
    let plan = planner::plan(&selected, &state.cache);
    let mut error: Option<String> = None;
    // Pipelines whose own `/jobs` or `/bridges` could not be read this tick and
    // that have nothing cached. Evaluating one of these through the rules is B1:
    // an empty job list reads as "nothing failed".
    let mut unavailable: HashSet<u64> = HashSet::new();

    // ⛔ Fetched side by side, then applied one at a time in the plan's order,
    // which is the order the sequential loop used. Nothing below may move into
    // the fetch: the first error to be kept, the backoff and the cache all
    // have to see the pipelines in the same order on every tick, whichever
    // response arrived first. The fetch only reads `state`, which is what lets
    // every pipeline borrow it at once.
    let rows: Vec<&Pipeline> = plan
        .pipelines
        .iter()
        .filter_map(|p| selected.iter().find(|r| r.id == p.id))
        .collect();
    let shared: &WatchState = state;
    let results = in_order(
        rows.iter()
            .map(|row| fetch_detail(client, &project, row, &query, shared)),
    )
    .await;

    for (row, result) in rows.into_iter().zip(results) {
        match result {
            Ok(fetched) => {
                for e in &fetched.errors {
                    // ⛔ A failed child request used to be logged at `debug` and
                    // dropped: no error string, no backoff, and a bridge left
                    // reading its own status, which for a trigger job without
                    // `strategy: depend` is `success`. It is an error like any
                    // other.
                    state.policy.on_error(e);
                    error.get_or_insert_with(|| e.to_string());
                }
                // ⛔ An annotation that is still owed (a rate limit, a 5xx)
                // stores the detail as incomplete too, or a pipeline that has
                // settled would never be asked again and its first failing
                // line would be missing for good (Seer, #64). It is not an
                // error: no strip, no backoff, for a nicety.
                let incomplete = !fetched.errors.is_empty() || fetched.annotations_owed;
                state.last_bridges.insert(row.id, fetched.bridges);
                state.cache.insert_partial(row, fetched.detail, incomplete);
            }
            Err(e) => {
                state.policy.on_error(&e);
                error.get_or_insert_with(|| e.to_string());
                if state.cache.get(row.id).is_none() {
                    unavailable.insert(row.id);
                }
            }
        }
    }

    let keep: HashSet<u64> = selected.iter().map(|r| r.id).collect();
    state.cache.retain(&keep);
    state.last_bridges.retain(|id, _| keep.contains(id));

    let mut views: Vec<PipelineView> = Vec::new();
    for row in &selected {
        let (detail, detail_source) = match state.cache.get(row.id) {
            Some(entry) => (entry.detail.clone(), DetailSource::Fetched),
            None if unavailable.contains(&row.id) => {
                (PipelineDetail::bare(row.clone()), DetailSource::Unavailable)
            }
            None => (PipelineDetail::bare(row.clone()), DetailSource::Fetched),
        };
        let (view, script_error) =
            evaluate_pipeline(&detail, detail_source, &state.watch, &state.rules, script);
        if let Some(e) = script_error {
            error.get_or_insert(e);
        }
        views.push(view);
    }

    // The icon follows the highest pipeline id, which is the newest run, not the
    // one that happens to have finished most recently.
    let icon_state = if state.watch.role == Role::Primary {
        views.iter().max_by_key(|v| v.id).map(|v| v.state)
    } else {
        None
    };

    (
        WatchView {
            id: state.watch.id.clone(),
            role: state.watch.role,
            icon_state,
            rows: views,
            error,
            jobs: state.jobs_mode,
            provider: state.provider,
            actions: state.actions,
        },
        None,
        true,
    )
}

/// One pipeline's detail, plus whatever went wrong fetching parts of it.
///
/// The detail is still usable when `errors` is non-empty; it is simply missing
/// a child, and the caller has to store it as incomplete rather than as whole.
struct FetchedDetail {
    detail: PipelineDetail,
    errors: Vec<ClientError>,
    /// A first failing line could not be asked for yet; see [`annotate`].
    annotations_owed: bool,
    /// The pipeline's own bridges, for `last_bridges` once it is applied.
    bridges: Vec<Bridge>,
}

/// One child pipeline the dive reached this level, and what to ask about it.
struct ChildFetch {
    id: u64,
    project: ProjectRef,
    /// Its jobs are due: the planner says they may have moved.
    jobs: bool,
    /// Its own bridges are wanted: the dive goes deeper, and they are due or
    /// were never read.
    bridges: bool,
    /// Its bridges as of the previous tick, for the next level's planner.
    previous: Option<Vec<Bridge>>,
}

/// What asking about one [`ChildFetch`] returned.
struct ChildFetched {
    jobs: Option<Result<Vec<crate::model::Job>, ClientError>>,
    bridges: Option<Result<Vec<Bridge>, ClientError>>,
}

/// Fetch one pipeline's jobs, bridges and the children the dive rules select,
/// walking `dive.depth` levels of child pipeline.
///
/// `depth = 1` is the parent plus its own children and costs one request per
/// child. Each level past that costs one extra request per pipeline already
/// walked (its `/bridges`) plus one per pipeline it reaches, which is why it is
/// opt-in rather than the default.
async fn fetch_detail(
    client: &dyn CiClient,
    project: &ProjectRef,
    row: &Pipeline,
    query: &ListQuery,
    state: &WatchState,
) -> Result<FetchedDetail, ClientError> {
    // Through the row and its query rather than the id: see
    // `CiClient::listed_detail`. GitLab's answer is the same two requests.
    let (mut jobs, bridges) = client.listed_detail(project, row, query).await?;

    // Pipelines whose status has not moved keep their cached jobs; only the ones
    // the planner names are fetched again.
    let cached = state.cache.get(row.id).map(|e| e.detail.clone());
    let mut child_jobs: BTreeMap<u64, Vec<crate::model::Job>> = cached
        .as_ref()
        .map(|d| d.child_jobs.clone())
        .unwrap_or_default();
    let mut child_bridges: BTreeMap<u64, Vec<Bridge>> = cached
        .as_ref()
        .map(|d| d.child_bridges.clone())
        .unwrap_or_default();
    let mut errors: Vec<ClientError> = Vec::new();

    let depth = usize::from(state.watch.dive.depth.max(1));
    // Each frontier entry is one pipeline's bridges, the project they live in,
    // and the same list as of the previous tick.
    let mut frontier: Vec<(Vec<Bridge>, ProjectRef, Option<Vec<Bridge>>)> = vec![(
        bridges.clone(),
        project.clone(),
        state.last_bridges.get(&row.id).cloned(),
    )];
    // Every pipeline the dive selected, at any level. Doubles as the cycle
    // guard: ids are unique, so seeing one twice means the walk is going round.
    let mut reached: HashSet<u64> = HashSet::new();
    // The project each reached child lives in, for its annotation.
    let mut child_projects: HashMap<u64, ProjectRef> = HashMap::new();

    for level in 1..=depth {
        // Decide the whole level first, in bridge order, then ask for all of it
        // at once and apply the answers in that same order.
        //
        // ⚠ One difference from asking one child at a time, and it is on the
        // side of fewer requests: a child two bridges point at is asked about
        // once per tick even when that one ask fails. The one-at-a-time walk
        // forgot a failed child before reaching the second bridge and asked
        // again straight away, which against a rate limit is the wrong reflex.
        let mut wanted: Vec<ChildFetch> = Vec::new();
        for (level_bridges, level_project, previous) in std::mem::take(&mut frontier) {
            let held: HashSet<u64> = child_jobs.keys().copied().collect();
            let due: HashSet<u64> =
                planner::child_fetches(&level_bridges, previous.as_deref(), &held, &state.rules)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();

            for bridge in &level_bridges {
                if !state.rules.should_dive(&bridge.name, &bridge.status) {
                    continue;
                }
                let Some(down) = &bridge.downstream_pipeline else {
                    continue;
                };
                if !reached.insert(down.id) {
                    continue;
                }
                let nested_previous = cached
                    .as_ref()
                    .and_then(|d| d.child_bridges.get(&down.id).cloned());
                let is_due = due.contains(&down.id);
                let child_project = down
                    .project_id
                    .map(ProjectRef::Id)
                    .unwrap_or_else(|| level_project.clone());
                child_projects.insert(down.id, child_project.clone());
                wanted.push(ChildFetch {
                    id: down.id,
                    project: child_project,
                    jobs: is_due,
                    bridges: level < depth && (is_due || nested_previous.is_none()),
                    previous: nested_previous,
                });
            }
        }

        let answers = in_order(wanted.iter().map(|child| async move {
            let jobs = match child.jobs {
                true => Some(client.child_jobs(&child.project, child.id).await),
                false => None,
            };
            // A child whose jobs could not be read is not walked any deeper
            // this tick, so its bridges are not asked for either.
            let bridges = match (child.bridges, &jobs) {
                (true, None | Some(Ok(_))) => {
                    Some(client.pipeline_bridges(&child.project, child.id).await)
                }
                _ => None,
            };
            ChildFetched { jobs, bridges }
        }))
        .await;

        let mut next = Vec::new();
        for (child, answer) in wanted.into_iter().zip(answers) {
            match answer.jobs {
                Some(Ok(jobs)) => {
                    child_jobs.insert(child.id, jobs);
                }
                Some(Err(e)) => {
                    // A child in another project the token cannot see is a
                    // real configuration outcome, not a bug — but it is still
                    // an outcome the user has to be told about, and the entry
                    // is dropped so the next tick asks again rather than
                    // remembering the gap forever.
                    tracing::debug!(child = child.id, error = %e, "child jobs unavailable");
                    child_jobs.remove(&child.id);
                    child_bridges.remove(&child.id);
                    errors.push(e);
                    reached.remove(&child.id);
                    continue;
                }
                None => {}
            }

            if level >= depth {
                continue;
            }
            match answer.bridges {
                Some(Ok(nested)) => {
                    child_bridges.insert(child.id, nested);
                }
                Some(Err(e)) => {
                    tracing::debug!(child = child.id, error = %e, "child bridges unavailable");
                    child_bridges.remove(&child.id);
                    errors.push(e);
                    continue;
                }
                None => {}
            }
            if let Some(nested) = child_bridges.get(&child.id) {
                next.push((nested.clone(), child.project, child.previous));
            }
        }
        frontier = next;
    }

    // Anything the dive no longer selects, or could not reach, is forgotten.
    child_jobs.retain(|id, _| reached.contains(id));
    child_bridges.retain(|id, _| reached.contains(id));

    let annotations_owed = annotate(
        client,
        project,
        &mut jobs,
        &mut child_jobs,
        &child_projects,
        &state.rules,
    )
    .await;

    Ok(FetchedDetail {
        detail: PipelineDetail {
            pipeline: row.clone(),
            jobs,
            bridges: bridges.clone(),
            child_jobs,
            child_bridges,
        },
        errors,
        annotations_owed,
        bridges,
    })
}

/// Attach the first failing line's annotation to each pipeline's first
/// BLOCKING failure, the parent's and every reached child's (a commit group's
/// runs are its children), asking the client for those that have none yet.
/// Returns whether any answer is still owed ([`AnnotationAnswer::Later`]).
///
/// ⛔ Chosen here, under the watch's `[watches.jobs]` rules, and not in the
/// client, which cannot see them: the first raw failed job may be one the
/// rules ignore or call a warning, and annotating it left the failure the
/// notification names without a line (Seer, #64). "First" is list order, the
/// order the verdict's `failures` are in.
///
/// Costs nothing on GitLab (the default answer, no request) and nothing on a
/// later tick on GitHub (the client remembers every answer). A cached child
/// whose job already has its annotation is not asked at all.
async fn annotate(
    client: &dyn CiClient,
    project: &ProjectRef,
    jobs: &mut [crate::model::Job],
    child_jobs: &mut BTreeMap<u64, Vec<crate::model::Job>>,
    child_projects: &HashMap<u64, ProjectRef>,
    rules: &WatchRules,
) -> bool {
    let first = |list: &[crate::model::Job]| {
        list.iter()
            .position(|j| crate::verdict::job::classify(j, rules) == JobClass::BlockingFailure)
            .filter(|&i| list[i].annotation.is_none())
    };
    // (child id, or none for the parent; index of the job; its project)
    let mut asks: Vec<(Option<u64>, usize, &ProjectRef)> = Vec::new();
    if let Some(i) = first(jobs) {
        asks.push((None, i, project));
    }
    for (id, list) in child_jobs.iter() {
        if let (Some(i), Some(child_project)) = (first(list), child_projects.get(id)) {
            asks.push((Some(*id), i, child_project));
        }
    }
    if asks.is_empty() {
        return false;
    }
    let answers = {
        let (jobs, child_jobs) = (&*jobs, &*child_jobs);
        in_order(asks.iter().map(|(child, i, p)| {
            let job = match child {
                None => &jobs[*i],
                Some(id) => &child_jobs[id][*i],
            };
            client.failure_annotation(p, job)
        }))
        .await
    };
    let mut owed = false;
    for ((child, i, _), answer) in asks.into_iter().zip(answers) {
        let job = match child {
            None => &mut jobs[i],
            Some(id) => match child_jobs.get_mut(&id) {
                Some(list) => &mut list[i],
                None => continue,
            },
        };
        match answer {
            AnnotationAnswer::Known(found) => job.annotation = found,
            AnnotationAnswer::Later => owed = true,
        }
    }
    owed
}

/// The client one account is polled through: a real transport, and its
/// credential, resolved now or (for `token = { oauth = .. }`) on each request
/// by the sign-in's own session.
///
/// [`Poller::from_config`] builds every polled account's with this, the CLI's
/// `log`, `retry` and `play` build the one account a job URL names, and the
/// setup wizard's live steps build theirs, so none of them can come to
/// disagree about how an account authenticates.
///
/// `token` is a credential the caller already holds (the wizard's pasted token,
/// not yet stored anywhere); `None` resolves the account's own source through
/// `store`. A sign-in ignores it: its credential is the session's.
///
/// ⚠️ Can block: resolving a source reads the OS credential store (which may
/// raise a macOS prompt) or runs a program. An async caller runs it on a
/// blocking thread.
pub fn client_for_account(
    name: &str,
    account: &crate::config::Account,
    token: Option<crate::token::Secret>,
    store: Arc<dyn crate::token::TokenProvider>,
    ring: RequestRing,
) -> Result<Arc<dyn CiClient>, PollerError> {
    let transport: Arc<dyn crate::client::Transport> = Arc::new(
        crate::client::ReqwestTransport::new(std::time::Duration::from_secs(account.timeout_secs))
            .map_err(PollerError::Client)?,
    );
    client_for_account_on(name, account, token, store, transport, ring)
}

/// [`client_for_account`] over a transport the caller supplies: the network's
/// in [`client_for_account`], a script in a test.
pub fn client_for_account_on(
    name: &str,
    account: &crate::config::Account,
    token: Option<crate::token::Secret>,
    store: Arc<dyn crate::token::TokenProvider>,
    transport: Arc<dyn crate::client::Transport>,
    ring: RequestRing,
) -> Result<Arc<dyn CiClient>, PollerError> {
    // ⛔ Through the factory, never by naming a client type: an account whose
    // provider has no client yet fails HERE, loudly, rather than being handed a
    // GitLab client pointed at somebody else's API.
    match &account.token {
        crate::config::TokenSource::Oauth(source) => {
            let transport = crate::oauth::transport_for(
                name,
                account,
                source,
                &crate::oauth::BuiltinClients::shipped(),
                transport,
                store,
            )
            .map_err(|e| PollerError::SignIn(name.to_string(), e))?;
            crate::client::client_for(
                &crate::oauth::bearer_account(account),
                &crate::token::Secret::new(""),
                transport,
                ring,
            )
        }
        _ => {
            let token = match token {
                Some(token) => token,
                None => crate::token::resolve(name, account, store.as_ref())
                    .map_err(|e| PollerError::Token(name.to_string(), e))?,
            };
            crate::client::client_for(account, &token, transport, ring)
        }
    }
    .map_err(PollerError::Client)
}

/// The network's transport for an account, on its own timeout.
fn network_transport() -> MakeTransport {
    Arc::new(|account: &crate::config::Account| {
        let transport: Arc<dyn crate::client::Transport> =
            Arc::new(crate::client::ReqwestTransport::new(
                std::time::Duration::from_secs(account.timeout_secs),
            )?);
        Ok(transport)
    })
}

/// One account's client, resolving its credential through `store`: the
/// one path a build and [`Poller::retry_refused`] share.
fn account_client(
    name: &str,
    account: &crate::config::Account,
    store: &Arc<dyn crate::token::TokenProvider>,
    ring: &RequestRing,
    transport: &MakeTransport,
) -> Result<Arc<dyn CiClient>, PollerError> {
    let transport = transport(account).map_err(PollerError::Client)?;
    client_for_account_on(name, account, None, store.clone(), transport, ring.clone())
}

/// What a per-account poller does with a failed client build: a credential
/// failure becomes a refusing client; anything else is returned as the error
/// it was. The error text is the token error's display, which never holds a
/// token.
fn refusal(
    name: &str,
    account: &crate::config::Account,
    failure: PollerError,
    ring: &RequestRing,
) -> Result<Arc<dyn CiClient>, PollerError> {
    let message = match failure {
        PollerError::Token(_, e) => {
            tracing::warn!(account = %name, error = %e, "the account's token is not usable");
            e.to_string()
        }
        PollerError::SignIn(_, e) => {
            tracing::warn!(account = %name, error = %e, "the account cannot sign in");
            e.to_string()
        }
        other => return Err(other),
    };
    refused_client(name, account, message, ring.clone())
}

/// A client for an account whose credential failed: every request is
/// refused with [`ClientError::Token`], and none leaves the machine.
fn refused_client(
    name: &str,
    account: &crate::config::Account,
    message: String,
    ring: RequestRing,
) -> Result<Arc<dyn CiClient>, PollerError> {
    let transport = Arc::new(RefusedTransport {
        account: name.to_string(),
        message,
    });
    crate::client::client_for(account, &crate::token::Secret::new(""), transport, ring)
        .map_err(PollerError::Client)
}

/// The transport behind [`refused_client`]: it answers every request with the
/// same error, without sending it.
#[derive(Debug)]
struct RefusedTransport {
    account: String,
    message: String,
}

#[async_trait::async_trait]
impl crate::client::Transport for RefusedTransport {
    async fn execute(
        &self,
        _request: crate::client::HttpRequest,
    ) -> Result<crate::client::HttpResponse, ClientError> {
        Err(ClientError::Token {
            account: self.account.clone(),
            message: self.message.clone(),
        })
    }
}

/// Build the list query for a watch.
///
/// An exact ref is pushed to the API; a glob or regex cannot be, so recent
/// pipelines are listed by `updated_at` and filtered here. A single source is
/// pushed too; several are not, because GitLab's `source` takes one value.
///
/// ⚠️ The page is asked for once and is never paginated, so whatever it holds is
/// all the watch can see. Where a filter has to run **here** rather than at the
/// API, the page has to be big enough to survive it: `sources = ["push", "web"]`
/// on an exact ref cannot be pushed, and a project whose recent pipelines are
/// mostly hourly schedules would otherwise return a page of rows that all get
/// discarded and a watch that shows nothing.
pub fn list_query(watch: &Watch, rules: &WatchRules) -> ListQuery {
    let per_page = (watch.show.max_rows as u32 * 4).clamp(10, 100);
    let query = match rules.ref_matcher.exact() {
        Some(exact) => match watch.sources.as_slice() {
            // An empty list filters nothing, so the page can be exactly what the
            // watch shows.
            [] => ListQuery::exact(exact, None, per_page),
            [only] => ListQuery::exact(exact, Some(only.clone()), per_page),
            _ => ListQuery::exact(exact, None, per_page.max(30)),
        },
        None => ListQuery::scan(per_page.max(30)),
    };
    // GitLab ignores all three; GitHub asks that workflow's own endpoint, folds
    // a commit's runs into one row, and holds each row to its expected
    // workflows. Carried on the query rather than read from
    // the watch inside a client, so that the clients keep taking one request
    // description and nothing provider-shaped.
    query
        .for_workflow(watch.workflow.as_deref())
        .in_commit_groups(watch.commit_group_window())
        .expecting(watch.expected_workflows())
}

/// `query`, held to what the credential's owner started when the watch says
/// `mine = true`: one [`CiClient::owner`] lookup per client, remembered.
async fn mine(
    client: &dyn CiClient,
    watch: &Watch,
    query: ListQuery,
) -> Result<ListQuery, ClientError> {
    if !watch.mine {
        return Ok(query);
    }
    let login = client.owner().await?;
    Ok(query.for_user(Some(&login)))
}

/// Whether an account's token expiry costs a request to learn: a GitLab
/// token that is not a sign-in (F3). A sign-in states its own end, and GitHub
/// sends its expiry header on every response.
fn expiry_costs_a_request(account: &crate::config::Account) -> bool {
    account.provider == Provider::Gitlab
        && !matches!(account.token, crate::config::TokenSource::Oauth(_))
}

/// Filter and trim the list rows a watch should show.
///
/// Every unsettled pipeline is kept, plus `show.settled` of the most recent
/// settled ones, all bounded by `show.max_rows`. A `skipped` pipeline is passed
/// over entirely.
///
/// ⛔ GitLab creates a `skipped` pipeline for a push whose changes match no
/// job's rules (a docs-only push on a project that gates every job on
/// `changes:`), and on GitHub a commit whose every workflow run was skipped
/// folds to one. It ran nothing and says nothing, but it is settled, so it took
/// the `show.settled` slot, the real pipeline before it fell off the list, and
/// the icon (the newest id) read the not-built parent as `canceled` and fired
/// `finished: canceled` for a push that changed nothing. `canceled` is NOT
/// passed over: somebody stopped that pipeline, and that is news.
pub fn select_rows(rows: &[Pipeline], watch: &Watch, rules: &WatchRules) -> Vec<Pipeline> {
    let mut matching: Vec<Pipeline> = rows
        .iter()
        .filter(|p| rules.ref_matcher.matches(&p.ref_name))
        .filter(|p| WatchRules::source_matches(&watch.sources, p.source.as_deref()))
        .cloned()
        .collect();
    matching.sort_by_key(|p| std::cmp::Reverse(p.id));

    let mut out = Vec::new();
    let mut settled_taken = 0usize;
    for row in &matching {
        if out.len() >= watch.show.max_rows {
            break;
        }
        if row.status == Status::Skipped {
            continue;
        }
        if row.status.is_live() {
            out.push(row.clone());
        } else if settled_taken < watch.show.settled {
            settled_taken += 1;
            out.push(row.clone());
        }
    }

    // ⛔ A primary watch always keeps its newest match, whatever the trim says.
    // The icon is computed from the rows, so `show.settled = 0` with nothing
    // running — or `max_rows = 0`, which the schema accepts — produced zero rows
    // and a tray reading `unknown` on a perfectly green estate. A secondary
    // watch has no icon and is left to show exactly what it was asked for.
    //
    // The newest pipeline that ran anything, when there is one. A watch whose
    // every match was skipped still keeps the newest of those: it is the only
    // answer there is, and `canceled` is truer than `unknown` for it.
    if out.is_empty()
        && watch.role.is_primary()
        && let Some(newest) = matching
            .iter()
            .find(|p| p.status != Status::Skipped)
            .or_else(|| matching.first())
    {
        out.push(newest.clone());
    }
    out
}

#[cfg(test)]
mod retry_tests {
    //! [`Poller::retry_refused`]: a refused account is retried in place, and
    //! nothing else about the poller changes. Nothing here touches a real
    //! credential store or the network: an in-memory store and a script.

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::Poller;
    use crate::client::{HttpResponse, ScriptTransport};
    use crate::token::{Secret, TokenError, TokenProvider};

    /// A credential store in memory, shared by clones, so a test can save a
    /// token into the one the poller holds.
    #[derive(Clone, Default)]
    struct Memory(Arc<Mutex<BTreeMap<String, String>>>);

    impl TokenProvider for Memory {
        fn keyring_get(&self, service: &str, user: &str) -> Result<String, TokenError> {
            self.0
                .lock()
                .unwrap()
                .get(service)
                .cloned()
                .ok_or_else(|| TokenError::NoEntry {
                    service: service.into(),
                    user: user.into(),
                })
        }
        fn keyring_set(&self, service: &str, _: &str, secret: &str) -> Result<(), TokenError> {
            self.0.lock().unwrap().insert(service.into(), secret.into());
            Ok(())
        }
        fn keyring_delete(&self, service: &str, _: &str) -> Result<(), TokenError> {
            self.0.lock().unwrap().remove(service);
            Ok(())
        }
        fn env(&self, name: &str) -> Option<String> {
            (name == "TOK").then(|| "glpat-healthy".to_string())
        }
        fn run(&self, _: &[String]) -> Result<String, TokenError> {
            Err(TokenError::Empty)
        }
    }

    /// `moved` reads its own entry on gitlab.example.com; `ok` an env token.
    const BOTH: &str = r#"
        [accounts.moved]
        base_url = "https://gitlab.example.com"
        token = { own = true }

        [accounts.ok]
        token = { env = "TOK" }

        [[watches]]
        id = "broken"
        account = "moved"
        project = 2

        [[watches]]
        id = "fine"
        account = "ok"
        project = 1
        role = "secondary"
        "#;

    fn config(raw: &str) -> crate::Config {
        toml::from_str(raw).expect("the table deserialises")
    }

    /// A store whose `moved` token was saved for gitlab.com: refused for good.
    fn bound_elsewhere() -> Memory {
        let store = Memory::default();
        crate::token::set_own_token(
            "moved",
            "https://gitlab.com",
            &Secret::new("glpat-A"),
            &store,
        )
        .unwrap();
        store
    }

    /// Everything a watch keeps between ticks, as one comparable string.
    fn kept(poller: &Poller, id: &str) -> String {
        let state = poller
            .watches
            .iter()
            .find(|w| w.watch.id == id)
            .expect("the watch exists");
        format!(
            "{:?} | {:?} | {:?}",
            state.policy, state.cache, state.last_view
        )
    }

    fn requests_for(t: &ScriptTransport, project: u64) -> usize {
        let marker = format!("/projects/{project}/");
        t.requests()
            .iter()
            .filter(|r| r.url.contains(&marker))
            .count()
    }

    /// (a) A retry pass leaves every watch exactly as it was: the healthy
    /// account's backoff, `retry_after` and cached view survive, while the
    /// other account stays refused.
    #[tokio::test]
    async fn a_retry_pass_keeps_every_watchs_state() {
        let t = Arc::new(ScriptTransport::new());
        t.reply("/projects/1/pipelines", 200, "[]");
        t.reply_with(
            "/projects/1/pipelines",
            HttpResponse {
                retry_after: Some(60),
                ..HttpResponse::plain(429, "{}")
            },
        );
        let mut poller =
            Poller::from_config_per_account_on(&config(BOTH), &bound_elsewhere(), t.clone())
                .unwrap();
        assert_eq!(poller.refused_accounts(), ["moved"]);
        poller.tick().await; // fine: a view
        poller.tick().await; // fine: a 429, kept beside that view

        let fine = poller
            .watches
            .iter()
            .find(|w| w.watch.id == "fine")
            .unwrap();
        assert_eq!(fine.policy.consecutive_errors(), 1);
        assert!(format!("{:?}", fine.policy).contains("retry_after: Some(60s)"));
        assert!(fine.last_view.is_some());
        let before = (kept(&poller, "fine"), kept(&poller, "broken"));

        for _ in 0..3 {
            assert!(poller.retry_refused().is_empty());
        }
        assert_eq!(poller.refused_accounts(), ["moved"]);
        assert_eq!(
            (kept(&poller, "fine"), kept(&poller, "broken")),
            before,
            "a retry pass changes no watch's state"
        );
    }

    /// (b) A refused account becomes usable once its token is saved, without
    /// a reload, and its watches then poll; the other account's state is kept.
    #[tokio::test]
    async fn a_refused_account_recovers_in_place_once_its_token_is_saved() {
        let store = Memory::default(); // nothing stored for `moved` yet
        let t = Arc::new(ScriptTransport::new());
        t.reply("/projects/1/pipelines", 200, "[]");
        let mut poller =
            Poller::from_config_per_account_on(&config(BOTH), &store, t.clone()).unwrap();
        assert_eq!(poller.refused_accounts(), ["moved"]);
        let tick = poller.tick().await;
        let broken = tick
            .snapshot
            .watches
            .iter()
            .find(|w| w.id == "broken")
            .unwrap();
        assert!(
            broken
                .error
                .as_deref()
                .unwrap_or_default()
                .starts_with("account moved:"),
            "{:?}",
            broken.error
        );
        let fine_before = kept(&poller, "fine");

        crate::token::set_own_token(
            "moved",
            "https://gitlab.example.com",
            &Secret::new("glpat-B"),
            &store,
        )
        .unwrap();
        let broken_view = poller
            .watches
            .iter()
            .find(|w| w.watch.id == "broken")
            .map(|w| format!("{:?} | {:?}", w.cache, w.last_view))
            .unwrap();
        assert_eq!(poller.retry_refused(), ["moved"]);
        assert!(poller.refused_accounts().is_empty());
        assert_eq!(kept(&poller, "fine"), fine_before);
        // The recovered watch keeps its cache and last view; only the backoff
        // its own refusals caused is forgotten.
        let broken = poller
            .watches
            .iter()
            .find(|w| w.watch.id == "broken")
            .unwrap();
        assert_eq!(
            format!("{:?} | {:?}", broken.cache, broken.last_view),
            broken_view
        );
        assert_eq!(broken.policy.consecutive_errors(), 0);

        t.reply("/projects/2/pipelines", 200, "[]");
        t.reply("/projects/1/pipelines", 200, "[]");
        let tick = poller.tick().await;
        assert_eq!(requests_for(&t, 2), 1, "the recovered account polled");
        let broken = tick
            .snapshot
            .watches
            .iter()
            .find(|w| w.id == "broken")
            .unwrap();
        assert_eq!(broken.error, None);
    }

    /// (c) An account that stays refused sends nothing, however many passes
    /// and ticks go by.
    #[tokio::test]
    async fn a_permanently_refused_account_makes_no_request() {
        let raw = BOTH.split("[accounts.ok]").next().unwrap().to_string()
            + "\n[[watches]]\nid = \"broken\"\naccount = \"moved\"\nproject = 2\n";
        let t = Arc::new(ScriptTransport::new());
        let mut poller =
            Poller::from_config_per_account_on(&config(&raw), &bound_elsewhere(), t.clone())
                .unwrap();
        for _ in 0..4 {
            assert!(poller.retry_refused().is_empty());
            let tick = poller.tick().await;
            let error = tick.snapshot.watches[0].error.clone().unwrap_or_default();
            assert!(
                error.contains("was saved for https://gitlab.com"),
                "{error}"
            );
            assert!(!error.contains("glpat-A"), "{error}");
        }
        assert_eq!(poller.refused_accounts(), ["moved"]);
        assert!(t.requests().is_empty(), "{:?}", t.requests());
    }
}
