//! GitLab's wire format: exactly the fields bridgewatch decodes, and nothing
//! else.
//!
//! Every field bridgewatch does not use is dropped, and every field it does use
//! that GitLab may omit is an [`Option`]. Statuses decode through [`Status`],
//! which never fails on an unrecognised value. Nothing here is ever
//! serialised: each type converts into its [`crate::model`] twin and the model
//! is what the rest of the crate sees.
//!
//! ⚠️ The GitLab wire shape and the normalised model are field-for-field the
//! same today, because the model was grown from this API. That is a fact about
//! GitLab rather than a rule: the conversions below are the only place allowed
//! to assume it, so a change on either side stays a compile error here instead
//! of a silent re-interpretation everywhere.

use serde::Deserialize;

use crate::model;
use crate::status::Status;

/// A pipeline, as returned by both the list endpoint and `GET /pipelines/{id}`.
///
/// The list row carries fewer fields than the detail object; the extra ones are
/// optional so a single type serves both.
#[derive(Debug, Clone, Deserialize)]
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
    /// Last change to the pipeline.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// When the first job started.
    #[serde(default)]
    pub started_at: Option<String>,
    /// When the pipeline reached a settled status.
    #[serde(default)]
    pub finished_at: Option<String>,
}

impl From<Pipeline> for model::Pipeline {
    fn from(p: Pipeline) -> Self {
        Self {
            id: p.id,
            iid: p.iid,
            project_id: p.project_id,
            sha: p.sha,
            ref_name: p.ref_name,
            status: p.status,
            source: p.source,
            web_url: p.web_url,
            created_at: p.created_at,
            updated_at: p.updated_at,
            started_at: p.started_at,
            finished_at: p.finished_at,
        }
    }
}

/// A job inside a pipeline.
#[derive(Debug, Clone, Deserialize)]
pub struct Job {
    /// Instance-wide job id.
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
    /// Seconds the job has run, as GitLab computed it. A float, because GitLab
    /// sends fractional seconds.
    #[serde(default)]
    pub duration: Option<f64>,
    /// Link to the job in the GitLab UI.
    #[serde(default)]
    pub web_url: Option<String>,
}

impl From<Job> for model::Job {
    fn from(j: Job) -> Self {
        Self {
            id: j.id,
            name: j.name,
            status: j.status,
            stage: j.stage,
            allow_failure: j.allow_failure,
            started_at: j.started_at,
            finished_at: j.finished_at,
            duration: j.duration,
            web_url: j.web_url,
        }
    }
}

/// A trigger job: a job in the parent pipeline whose effect is to create a
/// child pipeline.
///
/// `downstream_pipeline` is `None` when the child was never created, which is
/// the single most interesting failure shape bridgewatch exists to surface.
#[derive(Debug, Clone, Deserialize)]
pub struct Bridge {
    /// Instance-wide job id of the trigger job itself.
    pub id: u64,
    /// The trigger job's name, e.g. `trigger:website`.
    pub name: String,
    /// Status of the trigger job.
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

impl From<Bridge> for model::Bridge {
    fn from(b: Bridge) -> Self {
        Self {
            id: b.id,
            name: b.name,
            status: b.status,
            stage: b.stage,
            allow_failure: b.allow_failure,
            started_at: b.started_at,
            web_url: b.web_url,
            downstream_pipeline: b.downstream_pipeline.map(Into::into),
        }
    }
}

/// The child pipeline a [`Bridge`] created.
#[derive(Debug, Clone, Deserialize)]
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

impl From<DownstreamPipeline> for model::DownstreamPipeline {
    fn from(d: DownstreamPipeline) -> Self {
        Self {
            id: d.id,
            project_id: d.project_id,
            sha: d.sha,
            ref_name: d.ref_name,
            status: d.status,
            web_url: d.web_url,
        }
    }
}

/// The authenticated user, from `GET /user`.
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    /// Instance-wide user id.
    pub id: u64,
    /// The login name.
    pub username: String,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// True for a bot user, which is what project and group access tokens
    /// authenticate as.
    #[serde(default)]
    pub bot: bool,
}

impl From<User> for model::User {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            name: u.name,
            bot: u.bot,
        }
    }
}

/// The token that authenticated a request, from
/// `GET /personal_access_tokens/self`. Never carries the token value.
#[derive(Debug, Clone, Deserialize)]
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

impl From<TokenInfo> for model::TokenInfo {
    fn from(t: TokenInfo) -> Self {
        Self {
            id: t.id,
            name: t.name,
            scopes: t.scopes,
            expires_at: t.expires_at,
            active: t.active,
        }
    }
}

/// A project, as `GET /projects` and `GET /projects/{id}` return it.
#[derive(Debug, Clone, Deserialize)]
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
}

impl From<Project> for model::Project {
    fn from(p: Project) -> Self {
        Self {
            id: p.id,
            path_with_namespace: p.path_with_namespace,
            name: p.name,
            default_branch: p.default_branch,
            web_url: p.web_url,
        }
    }
}
