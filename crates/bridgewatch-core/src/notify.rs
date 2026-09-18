//! Deciding what is worth interrupting somebody over, and rendering it.
//!
//! The core produces [`Notification`] values. Delivering them is the shell's
//! job: the CLI prints them, the Tauri app hands them to the OS.
//!
//! Two rules keep this quiet enough to leave switched on. **Secondary watches
//! never notify** — that is what the role is for. And **the first successful
//! tick baselines silently**: starting the app must not replay every failure of
//! the last week as a burst of notifications.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ClickTarget, Watch};
use crate::verdict::{PipelineView, WatchView};

/// The kinds of thing bridgewatch will tell you about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    /// A deploy marker succeeded.
    Deployed,
    /// A new blocking failure, or a bridge whose child was never created.
    BlockingFailure,
    /// A pipeline reached a settled state.
    Finished,
    /// A pipeline started.
    Started,
    /// A bridge parked at a manual gate.
    GateOpened,
}

impl NotifyKind {
    /// The name used in the dedupe key and in templates.
    pub fn as_str(&self) -> &'static str {
        match self {
            NotifyKind::Deployed => "deployed",
            NotifyKind::BlockingFailure => "blocking_failure",
            NotifyKind::Finished => "finished",
            NotifyKind::Started => "started",
            NotifyKind::GateOpened => "gate_opened",
        }
    }
}

/// A notification the shell should deliver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// Which watch produced it.
    pub watch: String,
    /// What happened.
    pub kind: NotifyKind,
    /// The pipeline it is about.
    pub pipeline_id: u64,
    /// Rendered title.
    pub title: String,
    /// Rendered body.
    pub body: String,
    /// Where a click should go. `None` when `click = "none"`, or when nothing
    /// suitable had a URL.
    pub url: Option<String>,
    /// The dedupe key that was recorded for this notification.
    pub key: String,
}

/// Which notifications have already been sent, and which watches have been
/// baselined.
///
/// Persisted, because the alternative is that restarting the app re-notifies
/// everything. It is the only thing bridgewatch keeps across launches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotifyLedger {
    /// Dedupe keys already delivered.
    #[serde(default)]
    pub seen: BTreeSet<String>,
    /// Watch ids whose first successful tick has happened.
    #[serde(default)]
    pub baselined: BTreeSet<String>,
}

/// How many keys a ledger keeps before the oldest are dropped.
///
/// Keys are pipeline-scoped, so this is a few hundred pipelines' worth.
pub const LEDGER_MAX_KEYS: usize = 2000;

/// The pipeline id a dedupe key begins with, for trimming.
///
/// A key that does not start with a number sorts first and is dropped first,
/// which is the right fate for a line that no longer matches the format.
fn key_pipeline_id(key: &str) -> u64 {
    key.split('|')
        .next()
        .and_then(|id| id.parse::<u64>().ok())
        .unwrap_or(0)
}

impl NotifyLedger {
    /// The tray application's ledger: `<state or data dir>/bridgewatch/notify.json`.
    pub fn default_path() -> PathBuf {
        Self::path_named("notify.json")
    }

    /// The CLI's ledger, `notify-cli.json` beside the app's.
    ///
    /// ⛔ Separate on purpose. The app and a `bridgewatch watch` running at the
    /// same time each load the ledger, add to their copy and write it back, so
    /// sharing one file meant the last writer silently deleted the other's
    /// keys and both re-announced pipelines the other had already announced.
    pub fn cli_path() -> PathBuf {
        Self::path_named("notify-cli.json")
    }

    fn path_named(file: &str) -> PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("bridgewatch")
            .join(file)
    }

    /// Load a ledger. A missing or unreadable file yields an empty ledger: a
    /// corrupt dedupe file must not stop the app, it can only cost one repeated
    /// notification.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Write the ledger, creating the directory if needed.
    ///
    /// ⚠ Written to a sibling and renamed over the target. A plain write that
    /// was interrupted (a full disk, a kill mid-tick) left a truncated file,
    /// which `load` reads as EMPTY: the next start then had no baselines and
    /// re-announced everything on screen.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "notify.json".into());
        let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        let result = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    /// Has this key already been delivered?
    pub fn contains(&self, key: &str) -> bool {
        self.seen.contains(key)
    }

    /// Record a key, trimming the oldest pipeline's keys when the ledger is
    /// full.
    ///
    /// ⛔ "Oldest" is the smallest pipeline **id**, not the smallest key. The
    /// set orders keys as strings, and `"1000000000|…"` sorts before
    /// `"999999999|…"`, so taking the set's first element drops the NEWEST
    /// pipeline the moment ids cross a power of ten — and the pipeline that was
    /// just trimmed is the one still on screen, so it re-notifies on every tick
    /// until it falls off the list.
    pub fn record(&mut self, key: String) {
        self.seen.insert(key);
        while self.seen.len() > LEDGER_MAX_KEYS {
            let Some(victim) = self
                .seen
                .iter()
                .min_by_key(|k| (key_pipeline_id(k), k.as_str()))
                .cloned()
            else {
                break;
            };
            self.seen.remove(&victim);
        }
    }

    /// Has this watch had its first successful tick?
    pub fn is_baselined(&self, watch: &str) -> bool {
        self.baselined.contains(watch)
    }

    /// Mark a watch as baselined.
    pub fn baseline(&mut self, watch: &str) {
        self.baselined.insert(watch.to_string());
    }
}

/// The dedupe key for one event: `pipeline|kind|sorted job ids`.
///
/// Job **names** rather than ids, because a retried job gets a new id and a
/// retry of the same failure is not news. Sorted, so the order GitLab happened
/// to return them in cannot produce a duplicate.
pub fn dedupe_key(pipeline_id: u64, kind: NotifyKind, jobs: &[String]) -> String {
    let mut sorted: Vec<&str> = jobs.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    format!("{pipeline_id}|{}|{}", kind.as_str(), sorted.join(","))
}

/// Build the notifications one watch's rows imply, recording each in the ledger.
///
/// Returns an empty list for a secondary watch, and for a watch that has not yet
/// been baselined — in which case every key that *would* have fired is recorded,
/// so the next tick only reports genuine changes.
pub fn notifications_for(
    watch: &Watch,
    view: &WatchView,
    ledger: &mut NotifyLedger,
) -> Vec<Notification> {
    if !watch.role.is_primary() {
        return Vec::new();
    }

    // ⛔ A tick whose list request failed has learned nothing, and baselining on
    // it is how a laptop that starts before its Wi-Fi does gets a burst of stale
    // `deployed` and `finished` notifications on the FIRST tick that works: the
    // empty tick claims the baseline, so the real one counts as change. An empty
    // row list with no error is a watch whose filter matched nothing, which is a
    // genuine baseline.
    if view.rows.is_empty() && view.error.is_some() {
        return Vec::new();
    }

    let baselining = !ledger.is_baselined(&watch.id);
    let mut out = Vec::new();

    for row in &view.rows {
        // `finished` is decided last. It overlaps every other kind by
        // construction — a pipeline that deployed is also a pipeline that
        // finished — and with the shipped config both fire on the same tick,
        // rendering the same title and the same body from the same template.
        let mut finished: Option<(NotifyKind, Vec<String>, String)> = None;
        let mut said_something = false;

        for (kind, jobs) in candidate_events(watch, row) {
            let key = dedupe_key(row.id, kind, &jobs);
            if ledger.contains(&key) {
                continue;
            }
            if baselining {
                ledger.record(key);
                continue;
            }
            if kind == NotifyKind::Finished {
                finished = Some((kind, jobs, key));
                continue;
            }
            match render(watch, row, kind, &jobs, key.clone()) {
                Ok(n) => {
                    // ⛔ Recorded only once the render worked. An unknown filter
                    // fails at render time — template validation is syntax-only
                    // — and recording first meant the key was burnt and the
                    // notification was lost for good, including after the
                    // template was fixed.
                    ledger.record(key);
                    out.push(n);
                    said_something = true;
                }
                Err(e) => {
                    tracing::warn!(watch = %watch.id, error = %e, "notification template failed")
                }
            }
        }

        if let Some((kind, jobs, key)) = finished {
            if said_something {
                // Something better was already said about this pipeline on this
                // tick. Record the key anyway, or it fires on its own next tick.
                ledger.record(key);
            } else {
                match render(watch, row, kind, &jobs, key.clone()) {
                    Ok(n) => {
                        ledger.record(key);
                        out.push(n);
                    }
                    Err(e) => {
                        tracing::warn!(watch = %watch.id, error = %e, "notification template failed")
                    }
                }
            }
        }
    }

    if baselining {
        ledger.baseline(&watch.id);
    }
    out
}

/// Which events this row would raise, before deduplication.
fn candidate_events(watch: &Watch, row: &PipelineView) -> Vec<(NotifyKind, Vec<String>)> {
    let mut events = Vec::new();

    if watch.notify.deployed && row.deploy == "live" {
        let marker = row
            .deploy_marker
            .as_ref()
            .map(|m| vec![m.name.clone()])
            .unwrap_or_default();
        events.push((NotifyKind::Deployed, marker));
    }

    if watch.notify.blocking_failure && !row.failures.is_empty() {
        // Up to three names: a notification listing twenty jobs is a wall, and
        // the popover is one click away.
        let jobs: Vec<String> = row.failures.iter().take(3).cloned().collect();
        events.push((NotifyKind::BlockingFailure, jobs));
    }

    // ⛔ `parked_gate` is not finished, and saying so is the one thing this tool
    // is for. Every other monitor calls that state "running forever"; calling it
    // "finished" is the same mistake wearing a different word. `canceled` stays:
    // it is genuinely terminal, and with `gate_opened = false` in the shipped
    // config nothing else would ever mention a pipeline somebody cancelled.
    if watch.notify.finished
        && !row.live
        && !matches!(
            row.state,
            crate::verdict::IconState::Unknown | crate::verdict::IconState::ParkedGate
        )
    {
        events.push((NotifyKind::Finished, vec![row.state.as_str().to_string()]));
    }

    if watch.notify.started && row.live {
        events.push((NotifyKind::Started, Vec::new()));
    }

    // ⚠️ A gate somewhere in a pipeline is not news; a pipeline that is *waiting*
    // on one is. With the shipped `[watches.jobs]` the four coverage shards are
    // manual on every push to `main`, so a bare "are there any gates" test fires
    // this on every push — for a set of jobs the override table exists to say
    // are expected to sit unrun. `parked_gate` is the state README rule 8 names,
    // and it is the one that means somebody has to press something.
    if watch.notify.gate_opened
        && row.state == crate::verdict::IconState::ParkedGate
        && !row.gates.is_empty()
    {
        let jobs: Vec<String> = row.gates.iter().take(3).cloned().collect();
        events.push((NotifyKind::GateOpened, jobs));
    }

    events
}

/// Render one notification through the watch's templates.
fn render(
    watch: &Watch,
    row: &PipelineView,
    kind: NotifyKind,
    jobs: &[String],
    key: String,
) -> Result<Notification, minijinja::Error> {
    let env = minijinja::Environment::new();
    let context = minijinja::context! {
        watch => minijinja::context! { id => watch.id.clone(), role => format!("{:?}", watch.role).to_lowercase() },
        kind => kind.as_str(),
        jobs => jobs,
        state => row.state.as_str(),
        deploy => row.deploy,
        sha7 => row.sha7,
        sha => row.sha,
        pipeline => row,
        failures => row.failures,
        warnings => row.warnings,
        gates => row.gates,
        r#ref => row.ref_name,
        source => row.source,
        id => row.id,
        url => row.web_url,
    };

    let title = env.render_str(&watch.notify.title, &context)?;
    let body = env.render_str(&watch.notify.body, &context)?;

    Ok(Notification {
        watch: watch.id.clone(),
        kind,
        pipeline_id: row.id,
        title,
        body,
        url: click_url(watch.notify.click, row),
        key,
    })
}

/// Resolve the configured click target to a URL.
pub fn click_url(target: ClickTarget, row: &PipelineView) -> Option<String> {
    match target {
        ClickTarget::None => None,
        ClickTarget::Pipeline => row.web_url.clone(),
        ClickTarget::MarkerJob => row
            .deploy_marker
            .as_ref()
            .and_then(|m| m.web_url.clone())
            .or_else(|| row.web_url.clone()),
        ClickTarget::FirstFailureOrPipeline => {
            first_failure_url(row).or_else(|| row.web_url.clone())
        }
    }
}

/// The URL of the first blocking failure, looking in the parent's jobs and then
/// in every dived child's.
fn first_failure_url(row: &PipelineView) -> Option<String> {
    let first = row.failures.first()?;
    row.parent_jobs
        .iter()
        .chain(row.bridges.iter().flat_map(|b| b.jobs.iter()))
        .find(|j| &j.name == first)
        .and_then(|j| j.web_url.clone())
        .or_else(|| {
            // A dead bridge has no job to point at; the trigger job is the next
            // best thing.
            row.bridges
                .iter()
                .find(|b| &b.name == first)
                .and_then(|b| b.web_url.clone())
        })
}
