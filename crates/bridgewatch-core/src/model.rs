//! The normalised model: one provider-neutral shape the whole engine reads.
//!
//! Nothing here is a wire format. Each provider decodes its own responses into
//! [`crate::client::wire`] and converts them into these types, so the verdict
//! engine, the cache, the notifier and the view model never learn which API a
//! pipeline came from. The names are GitLab's vocabulary because that is what
//! 0.1.0 published and `check --json`, the Rhai hook and every `expected.json`
//! pin, not because the meanings are GitLab's: a `Bridge` is "a dependent unit
//! of work reached from this one", which a GitHub workflow run is too.
//!
//! Optional fields are optional because a provider may omit them, and statuses
//! decode through [`Status`], which never fails on an unrecognised value.
//!
//! ⚠️ These types are `Serialize` as well as `Deserialize`, and the `Serialize`
//! half is a contract: the fixture recorder writes them back out as recorded
//! API responses. A field renamed, reordered or given a `skip_serializing_if`
//! here changes bytes that are committed to the repository.

use serde::{Deserialize, Serialize};

use crate::status::Status;

/// A pipeline, as returned by both the list endpoint and `GET /pipelines/{id}`.
///
/// The list row carries fewer fields than the detail object; the extra ones are
/// optional so a single type serves both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    /// Instance-wide pipeline id. This is what every other endpoint keys on.
    pub id: u64,
    /// Project-scoped pipeline number, shown in the GitLab UI.
    #[serde(default)]
    pub iid: Option<u64>,
    /// The project this pipeline belongs to. Child pipelines of a parent in the
    /// same project repeat the parent's id.
    #[serde(default)]
    pub project_id: Option<u64>,
    /// Full commit sha.
    #[serde(default)]
    pub sha: String,
    /// The git ref the pipeline ran on.
    #[serde(rename = "ref", default)]
    pub ref_name: String,
    /// Current status.
    pub status: Status,
    /// Pipeline source: `push`, `schedule`, `pipeline` (a child), `web`, ...
    #[serde(default)]
    pub source: Option<String>,
    /// Link to the pipeline in the GitLab UI.
    #[serde(default)]
    pub web_url: Option<String>,
    /// When the pipeline row was created.
    #[serde(default)]
    pub created_at: Option<String>,
    /// Last change to the pipeline. The cache uses this to decide whether a
    /// settled pipeline needs re-fetching.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// When the first job started.
    #[serde(default)]
    pub started_at: Option<String>,
    /// When the pipeline reached a settled status.
    #[serde(default)]
    pub finished_at: Option<String>,
}

impl Pipeline {
    /// The seven-character short sha the UI and notifications use.
    pub fn sha7(&self) -> String {
        self.sha.chars().take(7).collect()
    }

    /// The `(status, updated_at)` pair the cache validates a stored snapshot
    /// against. A settled pipeline whose pair has not moved is not re-fetched.
    pub fn revision(&self) -> (Status, Option<String>) {
        (self.status.clone(), self.updated_at.clone())
    }
}

/// A job inside a pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Instance-wide job id. Used as the tie-break ordering when `started_at`
    /// is null, which it is for every job that never ran.
    pub id: u64,
    /// Job name, including the `n/m` suffix GitLab appends to a parallel job.
    pub name: String,
    /// Current status.
    pub status: Status,
    /// The stage the job belongs to.
    #[serde(default)]
    pub stage: Option<String>,
    /// Whether a failure of this job is tolerated by the pipeline.
    #[serde(default)]
    pub allow_failure: bool,
    /// When the job started. Null for a job that never ran (manual, skipped).
    #[serde(default)]
    pub started_at: Option<String>,
    /// When the job finished.
    #[serde(default)]
    pub finished_at: Option<String>,
    /// Seconds the job has run, as GitLab computed it: the whole run for a
    /// finished job, the elapsed time at response time for a running one, and
    /// null for a job that never started. A float, because GitLab sends
    /// fractional seconds.
    #[serde(default)]
    pub duration: Option<f64>,
    /// Link to the job in the GitLab UI.
    #[serde(default)]
    pub web_url: Option<String>,
    /// GitLab's reason code for a failed job (`script_failure`,
    /// `stuck_or_timeout_failure`, ...), from the jobs list it already
    /// fetches. Null on a job that did not fail, and always null on GitHub,
    /// which has no such field and says why in [`Job::annotation`] instead.
    #[serde(default)]
    pub failure_reason: Option<String>,
    /// GitHub's first failure annotation of the job's check run (the
    /// `file:line` and message a failing step reported), when it was fetched.
    /// Only a failed pipeline's first blocking failure is ever asked about;
    /// see `GitHubClient::failure_annotation` and the poller's `annotate`.
    ///
    /// ⚠️ Skipped when absent, so a GitLab recording, which never has one,
    /// writes exactly the bytes it wrote before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation: Option<Annotation>,
}

/// One check-run annotation: where a failing step pointed, and what it said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    /// The file, relative to the repository root. GitHub writes `.github` for
    /// an annotation that is about the workflow rather than a file.
    #[serde(default)]
    pub path: Option<String>,
    /// The first line it points at.
    #[serde(default)]
    pub start_line: Option<u64>,
    /// What it said.
    #[serde(default)]
    pub message: String,
}

impl Annotation {
    /// `path:line: message`, on one line: the form a compiler error takes and
    /// the one an editor jumps to. The message is cut to its first non-empty
    /// line, and the location is left out when GitHub gave none worth
    /// pointing at (no path, or the `.github` placeholder).
    pub fn line(&self) -> Option<String> {
        let message = first_line(&self.message)?;
        let location = match (self.path.as_deref().map(str::trim), self.start_line) {
            (Some(p), _) if p.is_empty() || p == ".github" => None,
            (Some(p), Some(l)) if l > 0 => Some(format!("{p}:{l}")),
            (Some(p), _) => Some(p.to_string()),
            (None, _) => None,
        };
        Some(match location {
            Some(at) => format!("{at}: {message}"),
            None => message,
        })
    }
}

/// The most a first failing line carries, in characters. A notification body
/// and a popover row are both one line; the whole message is on the job page.
pub const FAILURE_LINE_MAX: usize = 200;

/// The first non-empty line of `text`, trimmed and cut to
/// [`FAILURE_LINE_MAX`] characters with an ellipsis. `None` when there is no
/// such line.
pub fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line.chars().count() <= FAILURE_LINE_MAX {
        return Some(line.to_string());
    }
    let cut: String = line.chars().take(FAILURE_LINE_MAX - 1).collect();
    Some(format!("{}\u{2026}", cut.trim_end()))
}

impl Job {
    /// The first failing line of a failed job: GitHub's annotation when one
    /// was fetched, else GitLab's reason code in words (`script_failure` reads
    /// "script failure"). `None` for a job that did not fail, and for a failed
    /// job its provider said nothing about.
    pub fn failure_line(&self) -> Option<String> {
        if self.status != Status::Failed {
            return None;
        }
        if let Some(line) = self.annotation.as_ref().and_then(Annotation::line) {
            return Some(line);
        }
        let reason = self.failure_reason.as_deref()?.trim();
        first_line(&reason.replace('_', " "))
    }

    /// Ordering key for "did this job start before that one".
    ///
    /// `started_at` first, job id as the tie-break, because a job that never ran
    /// has no start time and must still sort deterministically.
    pub fn order_key(&self) -> (Option<&str>, u64) {
        (self.started_at.as_deref(), self.id)
    }
}

/// A trigger job: a job in the parent pipeline whose effect is to create a
/// child pipeline.
///
/// `downstream_pipeline` is `None` when the child was never created, which is
/// the single most interesting failure shape bridgewatch exists to surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bridge {
    /// Instance-wide job id of the trigger job itself.
    pub id: u64,
    /// The trigger job's name, e.g. `trigger:website`.
    pub name: String,
    /// Status of the trigger job. With `strategy: depend` this mirrors the
    /// child's status; without it, it goes `success` the moment the child is
    /// created.
    pub status: Status,
    /// The stage the trigger job belongs to.
    #[serde(default)]
    pub stage: Option<String>,
    /// Whether a failure of the trigger job is tolerated.
    #[serde(default)]
    pub allow_failure: bool,
    /// When the trigger job started.
    #[serde(default)]
    pub started_at: Option<String>,
    /// Link to the trigger job in the GitLab UI.
    #[serde(default)]
    pub web_url: Option<String>,
    /// The child pipeline, when one was created.
    #[serde(default)]
    pub downstream_pipeline: Option<DownstreamPipeline>,
}

/// The child pipeline a [`Bridge`] created.
///
/// `project_id` matters: a multi-project trigger creates the child in another
/// project, and its jobs must be fetched from there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownstreamPipeline {
    /// The child pipeline's id.
    pub id: u64,
    /// The project the child was created in. May differ from the parent's.
    #[serde(default)]
    pub project_id: Option<u64>,
    /// The child's commit sha.
    #[serde(default)]
    pub sha: Option<String>,
    /// The child's ref.
    #[serde(rename = "ref", default)]
    pub ref_name: Option<String>,
    /// The child's status.
    pub status: Status,
    /// Link to the child pipeline in the GitLab UI.
    #[serde(default)]
    pub web_url: Option<String>,
}

/// Everything fetched for one pipeline in a single tick: the pipeline row, its
/// own jobs, its bridges, and the jobs of each child that was dived into.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDetail {
    /// The pipeline row this detail was built from.
    pub pipeline: Pipeline,
    /// The parent pipeline's own jobs.
    pub jobs: Vec<Job>,
    /// The parent pipeline's trigger jobs.
    pub bridges: Vec<Bridge>,
    /// Jobs of each dived child, keyed by child pipeline id. A bridge that was
    /// not dived into has no entry, which is different from having an empty one.
    pub child_jobs: std::collections::BTreeMap<u64, Vec<Job>>,
    /// Trigger jobs of each dived child, keyed by child pipeline id.
    ///
    /// Only populated when `dive.depth` is greater than 1: at depth 1 a child's
    /// own bridges are never asked for, so an absent entry means "not looked
    /// at" rather than "this child triggers nothing".
    #[serde(default)]
    pub child_bridges: std::collections::BTreeMap<u64, Vec<Bridge>>,
}

impl PipelineDetail {
    /// A detail with no jobs and no bridges, for a pipeline that has not been
    /// fetched beyond its list row.
    ///
    /// ⚠️ Evaluating one of these through the icon rules reports an empty job
    /// list as "nothing failed". Anything building a bare detail because a
    /// fetch FAILED must say so with
    /// [`crate::verdict::DetailSource::Unavailable`].
    pub fn bare(pipeline: Pipeline) -> Self {
        Self {
            pipeline,
            jobs: Vec::new(),
            bridges: Vec::new(),
            child_jobs: std::collections::BTreeMap::new(),
            child_bridges: std::collections::BTreeMap::new(),
        }
    }
}

/// The authenticated user, from `GET /user`. Used by the setup wizard to name
/// who a token belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// Instance-wide user id.
    pub id: u64,
    /// The login name. A project or group access token's bot user is named
    /// `project_<id>_bot_<hex>` or `group_<id>_bot_<hex>`.
    pub username: String,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// True for a bot user, which is what project and group access tokens
    /// authenticate as.
    #[serde(default)]
    pub bot: bool,
}

/// The token that authenticated a request, from
/// `GET /personal_access_tokens/self`. Never carries the token value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    /// Token id.
    pub id: u64,
    /// The name the token was created with.
    #[serde(default)]
    pub name: Option<String>,
    /// Granted scopes, e.g. `read_api`.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Expiry date, `YYYY-MM-DD`, when it has one.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Whether it is usable.
    #[serde(default)]
    pub active: Option<bool>,
}

/// A project, as `GET /projects` and `GET /projects/{id}` return it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Numeric project id.
    pub id: u64,
    /// `group/subgroup/project`.
    pub path_with_namespace: String,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// The default branch. Null for an empty repository.
    #[serde(default)]
    pub default_branch: Option<String>,
    /// Link to the project.
    #[serde(default)]
    pub web_url: Option<String>,
    /// When the provider last saw activity on it, as it reports it (RFC 3339):
    /// GitLab's `last_activity_at`, GitHub's `pushed_at`. `None` when the
    /// answer carries no such field.
    #[serde(default)]
    pub last_activity_at: Option<String>,
}
