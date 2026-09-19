//! GitHub's wire format: exactly the fields bridgewatch decodes, and nothing
//! else.
//!
//! ⛔ **The allow-list here is a privacy boundary, not a tidiness one.** A raw
//! workflow run carries `actor` and `triggering_actor` (whole user profiles),
//! `head_commit` (author and committer names, email addresses and the full
//! message), `display_title` (the commit's first line), `repository` and
//! `head_repository` (owner profiles); a raw job carries `runner_name`,
//! `runner_group_name` and `labels`, which on a self-hosted fleet name internal
//! machines. **None of them is declared below**, so serde drops them while the
//! body is being read and nothing downstream can leak what was never decoded.
//! Adding a field here is therefore a decision about what bridgewatch is
//! allowed to hold, not only about what it can use.
//!
//! ⚠️ Two shapes differ from GitLab and both are easy to get wrong. GitHub
//! wraps a collection in an object ([`RunsResponse`], [`JobsResponse`]) where
//! GitLab returns a bare array; and it splits one status into `status` plus
//! `conclusion`, which [`Status::from_github`] folds back into one.

use serde::Deserialize;

use crate::model;
use crate::status::Status;

/// `{ "total_count": N, "workflow_runs": [...] }`.
///
/// ⚠️ `total_count` is deliberately NOT decoded. It is not a count: unfiltered
/// it saturates at exactly 40000, and past a filtered listing's offset cap it
/// reads 0 while rows exist. Decoding it would invite sizing a fetch loop from
/// it, which is the one thing it cannot be used for.
#[derive(Debug, Clone, Deserialize)]
pub struct RunsResponse {
    /// The runs on this page, newest first.
    pub workflow_runs: Vec<WorkflowRun>,
}

/// `{ "total_count": N, "jobs": [...] }`. See [`RunsResponse`] on the count.
#[derive(Debug, Clone, Deserialize)]
pub struct JobsResponse {
    /// The jobs on this page.
    pub jobs: Vec<Job>,
}

/// One workflow run: GitHub's nearest thing to a pipeline.
///
/// ⚠️ There is no parent above it. One push produces one run per workflow, with
/// no field on any of them pointing at the others; folding those into a single
/// row is the commit group, which this build does not do yet, so one run is one
/// [`model::Pipeline`].
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowRun {
    /// Repository-wide run id. Every other endpoint keys on this.
    pub id: u64,
    /// The per-workflow run number, the `#42` GitHub's UI shows.
    #[serde(default)]
    pub run_number: Option<u64>,
    /// Full commit sha.
    #[serde(default)]
    pub head_sha: String,
    /// The branch the run is for. Null for a run on a tag or a detached ref.
    #[serde(default)]
    pub head_branch: Option<String>,
    /// The phase: `queued`, `in_progress`, `completed`, `waiting`, ...
    pub status: String,
    /// The outcome, null until `status` is `completed`.
    #[serde(default)]
    pub conclusion: Option<String>,
    /// The event that started it: `push`, `pull_request`, `schedule`, ...
    #[serde(default)]
    pub event: Option<String>,
    /// Link to the run in GitHub's UI.
    #[serde(default)]
    pub html_url: Option<String>,
    /// When the run row was created.
    #[serde(default)]
    pub created_at: Option<String>,
    /// Last change to the run.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// When the run actually started, which is not `created_at` for a run that
    /// queued.
    #[serde(default)]
    pub run_started_at: Option<String>,
}

impl From<WorkflowRun> for model::Pipeline {
    fn from(r: WorkflowRun) -> Self {
        Self {
            id: r.id,
            iid: r.run_number,
            // ⚠️ The run object does carry a `repository`, and it is not
            // decoded (see the module docs), so there is no project id to
            // report. Nothing needs one: `project_id` exists for GitLab's
            // multi-project children, and a GitHub run has no children to
            // fetch from anywhere else.
            project_id: None,
            sha: r.head_sha,
            ref_name: r.head_branch.unwrap_or_default(),
            status: Status::from_github(&r.status, r.conclusion.as_deref()),
            source: r.event,
            web_url: r.html_url,
            created_at: r.created_at,
            updated_at: r.updated_at,
            started_at: r.run_started_at,
            // ⚠️ GitHub reports no finish time for a run. `updated_at` is the
            // nearest thing and is not the same claim, so nothing is invented:
            // a completed run simply has no `finished_at`, exactly as a GitLab
            // pipeline that has not finished does.
            finished_at: None,
        }
    }
}

/// One job inside a workflow run.
///
/// ⚠️ A reusable (called) workflow's jobs arrive **in this same list**, named
/// `<caller job> / <called job>`, with the caller's `workflow_name` on every
/// one of them. The name is kept verbatim: it is what `deploy_markers` and
/// `[watches.jobs]` match against, and what GitHub's own UI shows.
#[derive(Debug, Clone, Deserialize)]
pub struct Job {
    /// Repository-wide job id.
    pub id: u64,
    /// Job name, including the `caller / callee` form above.
    pub name: String,
    /// `queued`, `in_progress` or `completed`.
    pub status: String,
    /// The outcome, null until `status` is `completed`.
    #[serde(default)]
    pub conclusion: Option<String>,
    /// When the job started.
    #[serde(default)]
    pub started_at: Option<String>,
    /// When the job finished. Null while it runs.
    #[serde(default)]
    pub completed_at: Option<String>,
    /// Link to the job in GitHub's UI.
    #[serde(default)]
    pub html_url: Option<String>,
    /// The job's steps.
    ///
    /// ⚠️ **Absent entirely on a job that never ran**, which is why this is an
    /// `Option<Vec<_>>` rather than a `Vec<_>` with a default: the two states
    /// are different facts, and a job with `Some([])` has not been seen in the
    /// wild. Nothing above the client reads steps yet; they are decoded here
    /// because they are the only place GitHub says WHICH part of a job failed,
    /// and because a type that exists is how the next packet's job detail can
    /// be reviewed against this allow-list rather than around it.
    #[serde(default)]
    pub steps: Option<Vec<Step>>,
}

impl From<Job> for model::Job {
    fn from(j: Job) -> Self {
        let stage = caller_of(&j.name);
        Self {
            id: j.id,
            name: j.name,
            status: Status::from_github(&j.status, j.conclusion.as_deref()),
            // ⛔ GitHub has no stages. `stage` is a display grouping, and the
            // one real grouping a GitHub run has is the called workflow a job
            // came from, which shows up only as the `caller / callee` naming.
            // Using the caller as the stage groups exactly the runs that have
            // something to group and leaves every other job ungrouped, which is
            // what `ui.jobs = "all"` already draws for a stage-less job. ⚠️ The
            // naming is UNDOCUMENTED (both research lanes found it only in live
            // responses), so it is read as a hint and never as a contract: if
            // GitHub changes it, every job simply has no stage again.
            stage,
            // ⛔ `continue-on-error` does not appear anywhere in the jobs API.
            // The YAML keyword changes what a failure DOES and the runner is
            // reported to force the conclusion to `success` besides, so there
            // is nothing here to read: on GitHub `[watches.jobs]` is the only
            // way to express a tolerated failure, and the README says so.
            allow_failure: false,
            started_at: j.started_at,
            finished_at: j.completed_at,
            // GitHub sends no duration. `verdict::job_duration` derives one
            // from the two timestamps, which is the same path a recorded GitLab
            // fixture takes, so a second computation here would only be a
            // second place to get it wrong.
            duration: None,
            web_url: j.html_url,
        }
    }
}

/// The caller job of a flattened reusable-workflow job name, if it is one.
///
/// Split on the FIRST separator: nesting reaches ten levels, so a name can
/// carry several, and the top-level caller is the useful grouping.
fn caller_of(name: &str) -> Option<String> {
    name.split_once(" / ").map(|(caller, _)| caller.to_string())
}

/// One step of a job. See [`Job::steps`] for why it is decoded.
#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    /// The step's name, as written in the workflow file or defaulted by the
    /// action it runs.
    pub name: String,
    /// Its position in the job, from 1.
    #[serde(default)]
    pub number: Option<u64>,
    /// `queued`, `in_progress` or `completed`. A step is never `waiting`.
    pub status: String,
    /// The outcome, null until `status` is `completed`.
    #[serde(default)]
    pub conclusion: Option<String>,
    /// When the step started.
    #[serde(default)]
    pub started_at: Option<String>,
    /// When the step finished.
    #[serde(default)]
    pub completed_at: Option<String>,
}

impl Step {
    /// The step's outcome, folded onto the shared vocabulary.
    pub fn status(&self) -> Status {
        Status::from_github(&self.status, self.conclusion.as_deref())
    }
}

/// The authenticated user, from `GET /user`.
///
/// ⚠️ Only the four fields the wizard names somebody by. `/user` also returns
/// an email address, a company, a location and a biography; none is declared,
/// so none is read.
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    /// Account id.
    pub id: u64,
    /// The login name.
    pub login: String,
    /// Display name, which a user may not have set.
    #[serde(default)]
    pub name: Option<String>,
    /// `User`, `Organization` or `Bot`. A GitHub App's installation token
    /// authenticates as a `Bot`.
    #[serde(rename = "type", default)]
    pub account_type: Option<String>,
}

impl From<User> for model::User {
    fn from(u: User) -> Self {
        let bot = u.account_type.as_deref() == Some("Bot");
        Self {
            id: u.id,
            username: u.login,
            name: u.name,
            bot,
        }
    }
}

/// A repository, from `GET /repos/{owner}/{repo}` and `GET /user/repos`.
#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    /// Numeric repository id.
    pub id: u64,
    /// `owner/repo`, which is also how a watch addresses it.
    pub full_name: String,
    /// The repository's own name, without the owner.
    #[serde(default)]
    pub name: Option<String>,
    /// The default branch. Null for an empty repository.
    #[serde(default)]
    pub default_branch: Option<String>,
    /// Link to the repository.
    #[serde(default)]
    pub html_url: Option<String>,
}

impl From<Repository> for model::Project {
    fn from(r: Repository) -> Self {
        Self {
            id: r.id,
            path_with_namespace: r.full_name,
            name: r.name,
            default_branch: r.default_branch,
            web_url: r.html_url,
        }
    }
}
