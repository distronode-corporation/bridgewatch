//! The verdict engine: pure functions from recorded API responses to the view
//! model the GUI renders and the CLI prints.
//!
//! Nothing here does I/O, reads a clock or needs a token, which is what lets the
//! whole of it be tested against recorded fixtures of real pipelines.

pub mod bridge;
pub mod deploy;
pub mod icon;
pub mod job;
pub mod script;

use serde::{Deserialize, Serialize};

pub use bridge::BridgeVerdict;
pub use deploy::{DeployOutcome, MarkerJob};
pub use icon::{IconState, PipelineFacts};
pub use script::{ScriptError, VerdictScript};

use crate::client::RequestLog;
use crate::config::{JobsMode, Role, Watch, WatchRules};
use crate::model::{Bridge, Job, PipelineDetail};
use crate::status::JobClass;

/// Whether a pipeline's `/jobs` and `/bridges` were actually read.
///
/// ⛔ The distinction is load-bearing and it is not visible in the data: a
/// [`PipelineDetail`] whose fetch failed and a pipeline with nothing wrong in it
/// both present an empty `jobs` list, and the rules read the second meaning. A
/// 429 or a 15 s timeout on the second request of a tick therefore used to turn
/// a red pipeline into the outline check with exit 0. The caller knows which it
/// has; the rules cannot work it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailSource {
    /// The detail was fetched, or served from a cache of a real fetch.
    Fetched,
    /// The fetch failed and nothing was cached, so only the list row is known.
    Unavailable,
}

/// One job, as the GUI draws it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobView {
    /// Job id.
    pub id: u64,
    /// Job name.
    pub name: String,
    /// The class after `[watches.jobs]` overrides. This is what the row colour
    /// comes from.
    pub class: JobClass,
    /// The raw GitLab status, kept because an override can hide a surprise.
    pub status: String,
    /// Whether GitLab tolerates a failure of this job.
    pub allow_failure: bool,
    /// The stage, when GitLab reported one.
    pub stage: Option<String>,
    /// Link to the job.
    pub web_url: Option<String>,
    /// When the job started, as GitLab reported it. Null for a job that never
    /// ran. The popover ticks a running job's elapsed time from this.
    pub started_at: Option<String>,
    /// When the job finished. Null while it runs and for a job that never ran.
    pub finished_at: Option<String>,
    /// Seconds the job ran; see [`job_duration`]. Null, never `0`, for a job
    /// that never started, so nothing renders a fake "0s".
    pub duration: Option<f64>,
}

impl JobView {
    fn from(job: &Job, class: JobClass) -> Self {
        Self {
            id: job.id,
            name: job.name.clone(),
            class,
            status: job.status.to_string(),
            allow_failure: job.allow_failure,
            stage: job.stage.clone(),
            web_url: job.web_url.clone(),
            started_at: job.started_at.clone(),
            finished_at: job.finished_at.clone(),
            duration: job_duration(job),
        }
    }
}

/// A job's duration in seconds.
///
/// GitLab's own `duration` wins when it sent one. Otherwise it is derived from
/// `finished_at - started_at`, which is what GitLab computes too: recorded
/// fixtures made before `duration` joined the allow-list carry only the
/// timestamps, and a self-managed instance behind a trimming proxy may as well.
/// A job with no finish time derives nothing (the UI ticks it from
/// `started_at`), and unparseable or reversed timestamps derive nothing rather
/// than a negative number.
pub fn job_duration(job: &Job) -> Option<f64> {
    if let Some(d) = job.duration {
        return Some(d);
    }
    let parse = |s: &Option<String>| {
        s.as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    };
    let (start, end) = (parse(&job.started_at)?, parse(&job.finished_at)?);
    let millis = end.signed_duration_since(start).num_milliseconds();
    (millis >= 0).then(|| millis as f64 / 1000.0)
}

/// One bridge, as the GUI draws it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeView {
    /// The trigger job's name.
    pub name: String,
    /// The trigger job's own status.
    pub status: String,
    /// The verdict word: `passed`, `failed`, `dead`, `awaiting_gate`, ...
    pub verdict: String,
    /// The job names the verdict names: the failures, the gates, the warnings.
    pub verdict_jobs: Vec<String>,
    /// The child pipeline's id, absent when the bridge is dead.
    pub child_id: Option<u64>,
    /// The child pipeline's link.
    pub child_url: Option<String>,
    /// The child's project, which differs for a multi-project trigger.
    pub child_project_id: Option<u64>,
    /// Whether the child's jobs were fetched. A bridge that was not dived into
    /// has an empty `jobs` list that means "not looked at", not "no jobs".
    pub dived: bool,
    /// The child's jobs, when dived.
    pub jobs: Vec<JobView>,
    /// The child's **own** trigger jobs, resolved the same way. Empty unless
    /// `dive.depth` took the walk past this level.
    #[serde(default)]
    pub bridges: Vec<BridgeView>,
    /// Link to the trigger job.
    pub web_url: Option<String>,
}

/// One pipeline, as the GUI draws it and as a Rhai verdict script receives it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineView {
    /// Pipeline id.
    pub id: u64,
    /// Project-scoped number shown in the GitLab UI.
    pub iid: Option<u64>,
    /// Full commit sha.
    pub sha: String,
    /// Short sha, for titles.
    pub sha7: String,
    /// The git ref.
    #[serde(rename = "ref")]
    pub ref_name: String,
    /// Pipeline source: `push`, `schedule`, ...
    pub source: Option<String>,
    /// The pipeline's own GitLab status.
    pub status: String,
    /// Link to the pipeline.
    pub web_url: Option<String>,
    /// The verdict: the icon state this pipeline alone would produce.
    pub state: IconState,
    /// The deploy outcome word: `absent`, `in_progress`, `live`, `failed`,
    /// `dead`, `canceled`.
    pub deploy: String,
    /// The marker job that succeeded, when one did.
    pub deploy_marker: Option<MarkerJob>,
    /// Blocking failures in the marker's own pipeline, when the deploy failed.
    pub deploy_failures: Vec<String>,
    /// Every blocking failure, plus the name of any bridge whose child was
    /// never created. This is the list a notification quotes.
    pub failures: Vec<String>,
    /// Tolerated failures worth showing.
    pub warnings: Vec<String>,
    /// Jobs waiting on a human.
    pub gates: Vec<String>,
    /// Blocking failures that started after the deploy marker succeeded.
    pub post_deploy_failures: Vec<String>,
    /// Failed or dead bridges other than the one carrying the deploy marker.
    pub sibling_failures: Vec<String>,
    /// The bridges, in the order GitLab returned them.
    pub bridges: Vec<BridgeView>,
    /// The parent pipeline's own jobs.
    pub parent_jobs: Vec<JobView>,
    /// When the pipeline was last touched, used by the cache.
    pub updated_at: Option<String>,
    /// When the pipeline was created.
    pub created_at: Option<String>,
    /// True while anything is genuinely in flight. A gate is not.
    pub live: bool,
}

/// One watch's contribution to the popover.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchView {
    /// The watch's configured id.
    pub id: String,
    /// Primary or secondary.
    pub role: Role,
    /// The icon state for this watch, computed on the highest pipeline id it
    /// matched. `None` for a secondary watch, which never touches the icon.
    pub icon_state: Option<IconState>,
    /// The pipelines, newest first.
    pub rows: Vec<PipelineView>,
    /// What went wrong fetching this watch, if anything.
    pub error: Option<String>,
    /// Which jobs this watch's rows list: its `show.jobs`, else `ui.jobs`.
    /// Resolved here so the frontend never re-implements the precedence.
    /// Every job is in the rows either way; this only says which to draw.
    #[serde(default)]
    pub jobs: JobsMode,
}

/// Everything the GUI needs to draw one frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// The tray icon state: the most severe state among primary watches.
    pub icon_state: IconState,
    /// Every watch, in configuration order.
    pub watches: Vec<WatchView>,
    /// Errors that are not attributable to one watch.
    pub errors: Vec<String>,
    /// When this snapshot was built.
    pub last_poll: chrono::DateTime<chrono::Utc>,
    /// The recent request ring, for the debug pane.
    pub request_log: Vec<RequestLog>,
}

impl Snapshot {
    /// An empty snapshot, before the first tick has completed.
    pub fn empty() -> Self {
        Self {
            icon_state: IconState::Unknown,
            watches: Vec::new(),
            errors: Vec::new(),
            last_poll: chrono::Utc::now(),
            request_log: Vec::new(),
        }
    }

    /// The tray state implied by the primary watches: worst news wins.
    pub fn icon_from_watches(watches: &[WatchView]) -> IconState {
        watches
            .iter()
            .filter(|w| w.role.is_primary())
            .filter_map(|w| w.icon_state)
            .min_by_key(|s| s.severity())
            .unwrap_or(IconState::Unknown)
    }

    /// True while any watch has something in flight, which is what the poll
    /// policy keys the fast interval on.
    pub fn any_live(&self) -> bool {
        self.watches.iter().flat_map(|w| &w.rows).any(|r| r.live)
    }
}

/// Resolve one level of the bridge tree, deepest first.
///
/// Returns each bridge's verdict alongside its view, because the view keeps only
/// the verdict's word and the level above needs the structured answer to decide
/// whether a grandchild broke its parent.
fn resolve_bridges(
    bridges: &[Bridge],
    detail: &PipelineDetail,
    rules: &WatchRules,
    seen: &mut std::collections::HashSet<u64>,
) -> Vec<(BridgeVerdict, BridgeView)> {
    let mut out = Vec::with_capacity(bridges.len());
    for b in bridges {
        // A bridge is a job, so `ignore` removes it from every verdict and from
        // the rows, exactly as it does for one of the parent's own jobs.
        if bridge::own_class(b, rules) == JobClass::Ignored {
            continue;
        }
        let child_id = b.downstream_pipeline.as_ref().map(|d| d.id);
        let child_jobs = child_id.and_then(|id| detail.child_jobs.get(&id));

        // A pipeline that has already been walked on this branch cannot be
        // walked again: ids are unique, so a repeat means a cycle, and a cycle
        // here would recurse until the stack ran out.
        let deeper = match child_id {
            Some(id) if seen.insert(id) => detail
                .child_bridges
                .get(&id)
                .map(|nested| resolve_bridges(nested, detail, rules, seen))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let nested: Vec<(String, BridgeVerdict)> = deeper
            .iter()
            .map(|(v, view)| (view.name.clone(), v.clone()))
            .collect();

        let verdict = bridge::verdict(b, child_jobs.map(Vec::as_slice), &nested, rules);
        let jobs = child_jobs
            .map(|js| {
                js.iter()
                    .map(|j| (job::classify(j, rules), j))
                    .filter(|(c, _)| *c != JobClass::Ignored)
                    .map(|(c, j)| JobView::from(j, c))
                    .collect()
            })
            .unwrap_or_default();

        let view = BridgeView {
            name: b.name.clone(),
            status: b.status.to_string(),
            verdict: verdict.word().to_string(),
            verdict_jobs: verdict.jobs().to_vec(),
            child_id,
            child_url: b
                .downstream_pipeline
                .as_ref()
                .and_then(|d| d.web_url.clone()),
            child_project_id: b.downstream_pipeline.as_ref().and_then(|d| d.project_id),
            dived: child_jobs.is_some(),
            jobs,
            bridges: deeper.into_iter().map(|(_, v)| v).collect(),
            web_url: b.web_url.clone(),
        };
        out.push((verdict, view));
    }
    out
}

/// Every pipeline the dive actually read jobs for, with whether it is still
/// live, walking the bridge tree to whatever depth was fetched.
fn dived_pipelines(
    bridges: &[Bridge],
    detail: &PipelineDetail,
    rules: &WatchRules,
    seen: &mut std::collections::HashSet<u64>,
    out: &mut Vec<(u64, bool)>,
) {
    for b in bridges {
        if bridge::own_class(b, rules) == JobClass::Ignored {
            continue;
        }
        let Some(down) = &b.downstream_pipeline else {
            continue;
        };
        if !seen.insert(down.id) {
            continue;
        }
        if detail.child_jobs.contains_key(&down.id) {
            out.push((down.id, down.status.is_live() || b.status.is_live()));
        }
        if let Some(nested) = detail.child_bridges.get(&down.id) {
            dived_pipelines(nested, detail, rules, seen, out);
        }
    }
}

/// Every bridge in the tree, flattened, parents before children.
fn flatten<'a>(views: &'a [BridgeView], out: &mut Vec<&'a BridgeView>) {
    for v in views {
        out.push(v);
        flatten(&v.bridges, out);
    }
}

/// Build the view for one pipeline.
///
/// `source` says whether the detail was genuinely read; see [`DetailSource`].
/// `script`, when present, replaces the icon rules for this watch. A script that
/// errors yields [`IconState::Unknown`] and the error is returned alongside, so
/// a broken script degrades to "I do not know" rather than to a wrong colour.
pub fn evaluate_pipeline(
    detail: &PipelineDetail,
    source: DetailSource,
    watch: &Watch,
    rules: &WatchRules,
    script: Option<&VerdictScript>,
) -> (PipelineView, Option<String>) {
    let parent_classified: Vec<(JobClass, &Job)> = detail
        .jobs
        .iter()
        .map(|j| (job::classify(j, rules), j))
        .filter(|(c, _)| *c != JobClass::Ignored)
        .collect();

    let mut seen = std::collections::HashSet::new();
    let resolved = resolve_bridges(&detail.bridges, detail, rules, &mut seen);
    let bridges: Vec<BridgeView> = resolved.into_iter().map(|(_, v)| v).collect();

    let mut all_bridges: Vec<&BridgeView> = Vec::new();
    flatten(&bridges, &mut all_bridges);

    // Deploy markers are looked for in the parent and in every pipeline that was
    // walked into, at every level `dive.depth` reached. A child that was not
    // dived cannot contribute a marker, which is a real limitation of
    // `dive.bridges = ""` and is documented as one.
    let mut dived = Vec::new();
    let mut seen_scopes = std::collections::HashSet::new();
    dived_pipelines(&detail.bridges, detail, rules, &mut seen_scopes, &mut dived);

    let mut scopes = vec![deploy::JobScope {
        pipeline_id: detail.pipeline.id,
        pipeline_live: detail.pipeline.status.is_live(),
        jobs: &detail.jobs,
    }];
    for (id, live) in &dived {
        if let Some(jobs) = detail.child_jobs.get(id) {
            scopes.push(deploy::JobScope {
                pipeline_id: *id,
                pipeline_live: *live,
                jobs,
            });
        }
    }

    let any_dead_bridge = all_bridges.iter().any(|b| b.verdict == "dead");
    let outcome = deploy::outcome(&scopes, rules, any_dead_bridge);

    let post_deploy_failures = outcome
        .marker()
        .map(|m| deploy::post_deploy_failures(&scopes, rules, m))
        .unwrap_or_default();
    let marker_scope_failures = outcome
        .marker()
        .map(|m| deploy::marker_scope_failures(&scopes, rules, m))
        .unwrap_or_default();

    let marker_pipeline = outcome.marker().map(|m| m.pipeline_id);
    // Siblings are top-level bridges only: a failure further down has already
    // been carried up into its own parent bridge's verdict, and counting it
    // twice would make one break look like two.
    let sibling_failures: Vec<String> = bridges
        .iter()
        .filter(|b| b.verdict == "failed" || b.verdict == "dead")
        .filter(|b| b.child_id.is_none() || b.child_id != marker_pipeline)
        .map(|b| b.name.clone())
        .collect();

    let parent_failures: Vec<String> = parent_classified
        .iter()
        .filter(|(c, _)| c.is_blocking_failure())
        .map(|(_, j)| j.name.clone())
        .collect();
    // A parent job that failed AFTER the marker succeeded is a post-deploy
    // failure and takes that policy. Counting it here as well would let rule 1
    // fire first, so `post_deploy_failure = "downgrade"` could never apply to a
    // job in the parent even though README rule 6 says it does.
    let parent_blocking = parent_failures
        .iter()
        .any(|name| !post_deploy_failures.contains(name));

    let mut failures = parent_failures.clone();
    for b in &bridges {
        if b.verdict == "dead" {
            failures.push(b.name.clone());
        } else if b.verdict == "failed" {
            if b.verdict_jobs.is_empty() {
                failures.push(b.name.clone());
            } else {
                failures.extend(b.verdict_jobs.iter().cloned());
            }
        }
    }
    // A marker that failed inside a still-running child is a failure the bridge
    // verdict cannot report — the bridge reads `running` while any sibling job
    // is in flight — so the deploy outcome contributes its own names. Without
    // this the `blocking_failure` notification waits for the longest unrelated
    // job in that pipeline.
    if let DeployOutcome::Failed { jobs } = &outcome {
        for name in jobs {
            if !failures.contains(name) {
                failures.push(name.clone());
            }
        }
    }

    let warnings: Vec<String> = parent_classified
        .iter()
        .filter(|(c, _)| c.is_warning_failure())
        .map(|(_, j)| j.name.clone())
        .chain(
            all_bridges
                .iter()
                .filter(|b| b.verdict == "passed_with_warnings")
                .flat_map(|b| b.verdict_jobs.iter().cloned()),
        )
        .collect();

    // Every job waiting on a human, wherever it is. A gate inside a child whose
    // bridge already read `passed` still costs somebody a button press, and a
    // list that only covered `awaiting_gate` bridges would hide exactly those.
    let gates: Vec<String> = parent_classified
        .iter()
        .filter(|(c, _)| c.is_gate())
        .map(|(_, j)| j.name.clone())
        .chain(
            all_bridges
                .iter()
                .flat_map(|b| b.jobs.iter())
                .filter(|j| j.class.is_gate())
                .map(|j| j.name.clone()),
        )
        .collect();

    // A pipeline is live only when a job or a bridge is actually in flight.
    // The pipeline's own `running` is not enough: a parent sits `running` for as
    // long as a bridge waits on a manual child, and calling that "running" is
    // exactly the mistake this tool exists to stop making.
    let nothing_fetched = detail.jobs.is_empty() && detail.bridges.is_empty();
    let anything_live = parent_classified.iter().any(|(c, _)| c.is_live())
        || all_bridges.iter().any(|b| b.verdict == "running")
        || (nothing_fetched && detail.pipeline.status.is_live());

    let facts = PipelineFacts {
        deploy: &outcome,
        parent_blocking,
        bridge_failed_or_dead: all_bridges
            .iter()
            .any(|b| b.verdict == "failed" || b.verdict == "dead"),
        sibling_failure: !sibling_failures.is_empty(),
        post_deploy_failure: !post_deploy_failures.is_empty(),
        marker_scope_failure: !marker_scope_failures.is_empty(),
        detail_unavailable: source == DetailSource::Unavailable,
        anything_live,
        // A manual job in the PARENT parks the pipeline just as surely as a
        // manual child does. Reading only the bridges meant every project
        // without bridges — which is most of them — showed the outline check
        // where README rule 8 promises the hourglass.
        awaiting_gate: all_bridges.iter().any(|b| b.verdict == "awaiting_gate")
            || parent_classified.iter().any(|(c, _)| c.is_gate()),
        parent_not_built: detail.pipeline.status.is_not_built(),
        any_blocking_failure: !failures.is_empty(),
        sibling_policy: watch.sibling_failure,
        post_deploy_policy: watch.post_deploy_failure,
    };

    let state = icon::state(&facts);
    // `deploy_failures` is "what else broke in the pipeline carrying the
    // marker", whether that stopped the deploy or merely happened around it.
    let deploy_failures = match &outcome {
        DeployOutcome::Failed { jobs } => jobs.clone(),
        _ => marker_scope_failures,
    };

    drop(all_bridges);

    let mut view = PipelineView {
        id: detail.pipeline.id,
        iid: detail.pipeline.iid,
        sha: detail.pipeline.sha.clone(),
        sha7: detail.pipeline.sha7(),
        ref_name: detail.pipeline.ref_name.clone(),
        source: detail.pipeline.source.clone(),
        status: detail.pipeline.status.to_string(),
        web_url: detail.pipeline.web_url.clone(),
        state,
        deploy: outcome.word().to_string(),
        deploy_marker: outcome.marker().cloned(),
        deploy_failures,
        failures,
        warnings,
        gates,
        post_deploy_failures,
        sibling_failures,
        bridges,
        parent_jobs: parent_classified
            .iter()
            .map(|(c, j)| JobView::from(j, *c))
            .collect(),
        updated_at: detail.pipeline.updated_at.clone(),
        created_at: detail.pipeline.created_at.clone(),
        live: anything_live,
    };

    let mut script_error = None;
    if let Some(script) = script {
        match script.evaluate(&view) {
            Ok(state) => view.state = state,
            Err(e) => {
                view.state = IconState::Unknown;
                script_error = Some(e.to_string());
            }
        }
    }

    (view, script_error)
}
