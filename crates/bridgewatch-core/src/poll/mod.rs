//! The poller: one tick fetches what the planner asked for, the verdict engine
//! turns it into a [`Snapshot`], and the notifier says what changed.

pub mod cache;
pub mod planner;
pub mod policy;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

pub use cache::{CacheEntry, PipelineCache};
pub use planner::{PipelinePlan, Plan};
pub use policy::{MAX_RETRY_AFTER, POLL_NOW_MIN_GAP, PollNow, PollPolicy};

use crate::client::{CiClient, ClientError, ListQuery, RequestRing};
use crate::config::{Config, JobsMode, ProjectRef, Role, Watch, WatchRules};
use crate::model::{Bridge, Pipeline, PipelineDetail};
use crate::notify::{Notification, NotifyLedger, notifications_for};
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
}

/// One watch's runtime state.
struct WatchState {
    watch: Watch,
    rules: WatchRules,
    cache: PipelineCache,
    policy: PollPolicy,
    /// `show.jobs` resolved against `ui.jobs`, once, at build time.
    jobs_mode: JobsMode,
    last_bridges: HashMap<u64, Vec<Bridge>>,
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
    sender: tokio::sync::watch::Sender<Snapshot>,
    /// Whether anything was in flight last tick. The backoff gate needs an
    /// interval before it knows this tick's answer, and last tick's is the only
    /// honest guess.
    last_any_live: bool,
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
            sender,
            last_any_live: false,
        })
    }

    /// Build a poller from a configuration, resolving each account's token and
    /// creating a real HTTP transport for it.
    pub fn from_config(
        config: &Config,
        provider: &dyn crate::token::TokenProvider,
    ) -> Result<Self, PollerError> {
        let ring = RequestRing::new(config.log.keep_requests);
        let mut clients = BTreeMap::new();
        for (name, account) in &config.accounts {
            let token = crate::token::resolve(&account.token, name, provider)
                .map_err(|e| PollerError::Token(name.clone(), e))?;
            let transport = crate::client::ReqwestTransport::new(std::time::Duration::from_secs(
                account.timeout_secs,
            ))
            .map_err(PollerError::Client)?;
            // ⛔ Through the factory, never by naming a client type: an account
            // whose provider has no client yet fails HERE, loudly, rather than
            // being handed a GitLab client pointed at somebody else's API.
            let client =
                crate::client::client_for(account, &token, Arc::new(transport), ring.clone())
                    .map_err(PollerError::Client)?;
            clients.insert(name.clone(), client);
        }
        Self::with_clients(config, clients, ring).map_err(PollerError::Config)
    }

    /// Load and persist the notification ledger at this path.
    pub fn with_ledger(mut self, path: std::path::PathBuf) -> Self {
        self.ledger = NotifyLedger::load(&path);
        self.ledger_path = Some(path);
        self
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

        for state in &mut self.watches {
            let Some(client) = self.clients.get(&state.watch.account) else {
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
                });
                continue;
            };

            // A watch that is backing off sits the tick out. The sleep between
            // ticks is the `min` over every watch, so a healthy watch on a fast
            // interval would otherwise drag a 429'd one back to the API at its
            // cadence and make `Retry-After` decorative. Nothing changed for
            // this watch, so nothing is notified either.
            if state.policy.should_defer(self.last_any_live) {
                if let Some(view) = &state.last_view {
                    if let Some(e) = &view.error {
                        errors.push(format!("{}: {e}", state.watch.id));
                    }
                    views.push(view.clone());
                }
                continue;
            }

            let view = poll_watch(client.as_ref(), state, self.script.as_deref()).await;
            if let Some(e) = &view.error {
                errors.push(format!("{}: {e}", state.watch.id));
            }
            log_transition(&state.watch.id, state.last_view.as_ref(), &view);
            notifications.extend(notifications_for(&state.watch, &view, &mut self.ledger));
            state.last_view = Some(view.clone());
            views.push(view);
        }

        if let Some(path) = &self.ledger_path
            && let Err(e) = self.ledger.save(path)
        {
            tracing::warn!(error = %e, "could not persist the notification ledger");
        }

        let snapshot = Snapshot {
            icon_state: Snapshot::icon_from_watches(&views),
            watches: views,
            errors,
            last_poll: chrono::Utc::now(),
            request_log: self.ring.entries(),
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
        }
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
) -> WatchView {
    let project = state.watch.project.clone();
    let query = list_query(&state.watch, &state.rules);

    let rows = match client.list_pipelines(&project, &query).await {
        Ok(rows) => rows,
        Err(e) => {
            state.policy.on_error(&e);
            // Keep the last frame rather than blanking the watch. The cache is
            // intact, so the only thing this tick learned is that one request
            // did not arrive, and the honest way to say that is an error
            // alongside the rows — not `unknown` where a verdict already exists.
            return match &state.last_view {
                Some(previous) => WatchView {
                    error: Some(e.to_string()),
                    ..previous.clone()
                },
                None => WatchView {
                    id: state.watch.id.clone(),
                    role: state.watch.role,
                    icon_state: None,
                    rows: Vec::new(),
                    error: Some(e.to_string()),
                    jobs: state.jobs_mode,
                },
            };
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

    for pipeline_plan in &plan.pipelines {
        let Some(row) = selected.iter().find(|r| r.id == pipeline_plan.id) else {
            continue;
        };
        match fetch_detail(client, &project, row, state).await {
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
                let incomplete = !fetched.errors.is_empty();
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

    WatchView {
        id: state.watch.id.clone(),
        role: state.watch.role,
        icon_state,
        rows: views,
        error,
        jobs: state.jobs_mode,
    }
}

/// One pipeline's detail, plus whatever went wrong fetching parts of it.
///
/// The detail is still usable when `errors` is non-empty; it is simply missing
/// a child, and the caller has to store it as incomplete rather than as whole.
struct FetchedDetail {
    detail: PipelineDetail,
    errors: Vec<ClientError>,
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
    state: &mut WatchState,
) -> Result<FetchedDetail, ClientError> {
    let jobs = client.pipeline_jobs(project, row.id).await?;
    let bridges = client.pipeline_bridges(project, row.id).await?;

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

    for level in 1..=depth {
        let mut next = Vec::new();
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
                let child_project = down
                    .project_id
                    .map(ProjectRef::Id)
                    .unwrap_or_else(|| level_project.clone());

                if due.contains(&down.id) {
                    match client.child_jobs(&child_project, down.id).await {
                        Ok(jobs) => {
                            child_jobs.insert(down.id, jobs);
                        }
                        Err(e) => {
                            // A child in another project the token cannot see is
                            // a real configuration outcome, not a bug — but it
                            // is still an outcome the user has to be told about,
                            // and the entry is dropped so the next tick asks
                            // again rather than remembering the gap forever.
                            tracing::debug!(child = down.id, error = %e, "child jobs unavailable");
                            child_jobs.remove(&down.id);
                            child_bridges.remove(&down.id);
                            errors.push(e);
                            reached.remove(&down.id);
                            continue;
                        }
                    }
                }

                if level >= depth {
                    continue;
                }
                let nested_previous = cached
                    .as_ref()
                    .and_then(|d| d.child_bridges.get(&down.id).cloned());
                if due.contains(&down.id) || nested_previous.is_none() {
                    match client.pipeline_bridges(&child_project, down.id).await {
                        Ok(nested) => {
                            child_bridges.insert(down.id, nested);
                        }
                        Err(e) => {
                            tracing::debug!(child = down.id, error = %e, "child bridges unavailable");
                            child_bridges.remove(&down.id);
                            errors.push(e);
                            continue;
                        }
                    }
                }
                if let Some(nested) = child_bridges.get(&down.id) {
                    next.push((nested.clone(), child_project, nested_previous));
                }
            }
        }
        frontier = next;
    }

    // Anything the dive no longer selects, or could not reach, is forgotten.
    child_jobs.retain(|id, _| reached.contains(id));
    child_bridges.retain(|id, _| reached.contains(id));

    state.last_bridges.insert(row.id, bridges.clone());

    Ok(FetchedDetail {
        detail: PipelineDetail {
            pipeline: row.clone(),
            jobs,
            bridges,
            child_jobs,
            child_bridges,
        },
        errors,
    })
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
    // GitLab ignores it; GitHub asks that workflow's own endpoint. Carried on
    // the query rather than read from the watch inside a client, so that the
    // clients keep taking one request description and nothing provider-shaped.
    query.for_workflow(watch.workflow.as_deref())
}

/// Filter and trim the list rows a watch should show.
///
/// Every unsettled pipeline is kept, plus `show.settled` of the most recent
/// settled ones, all bounded by `show.max_rows`.
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
    if out.is_empty()
        && watch.role.is_primary()
        && let Some(newest) = matching.first()
    {
        out.push(newest.clone());
    }
    out
}
