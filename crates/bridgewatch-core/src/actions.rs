//! One job, named by its URL: which account and project it belongs to, the end
//! of its log, and the two things that can be done to it (retry, play).
//!
//! The tray and the CLI both start from a job's page URL, because that is what
//! a person has: the popover's job rows link to it, and it is what gets pasted
//! into a terminal. [`locate`] turns it into an account, a project and a job
//! id by the same host rule the shell's link opener uses (an account's
//! `base_url`, or for a github account the web host it implies), so a URL on a
//! host no account names is refused rather than guessed at.
//!
//! [`perform`] is the one place a write is sent from and the one place it is
//! logged: at `info`, with the account, the project, the job, the action and
//! how it ended. Never a token: nothing here holds one, and every error's
//! `Display` is built without one.

use crate::client::{CiClient, ClientError, JobActionOutcome, LOG_TAIL_BYTES, LogTail};
use crate::config::{Account, Config, ProjectRef, Provider};

/// A job, resolved against the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTarget {
    /// The `[accounts.*]` key it belongs to.
    pub account: String,
    /// That account's provider.
    pub provider: Provider,
    /// The project (`group/project`) or repository (`owner/repo`).
    pub project: ProjectRef,
    /// The job's id.
    pub job_id: u64,
}

/// What can be done to a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobAction {
    /// Run a finished job again: GitLab's retry, GitHub's re-run.
    Retry,
    /// Start a manual job (GitLab only).
    Play,
}

impl JobAction {
    /// The word for a log line and a sentence.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobAction::Retry => "retry",
            JobAction::Play => "play",
        }
    }
}

/// Why a URL could not be resolved to a job. The message says what to do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LocateError(pub String);

/// Resolve a job's page URL against the configuration.
///
/// `account`, when given, is the only account considered: the popover passes
/// the job's own watch's account, and `--account` does on the command line.
/// Otherwise the accounts whose host the URL is on are tried, and more than
/// one match is refused with their names rather than resolved by order.
///
/// Recognised shapes:
///
/// - GitLab: `<base_url>/<group>/<project>/-/jobs/<id>` (subgroups and a
///   `base_url` served under a path prefix included);
/// - GitHub: `<web host>/<owner>/<repo>/actions/runs/<run>/job/<id>`.
pub fn locate(config: &Config, url: &str, account: Option<&str>) -> Result<JobTarget, LocateError> {
    let url = url.trim();
    let Some(url_origin) = crate::oauth::origin_of(url) else {
        return Err(LocateError(format!(
            "{url:?} is not an http(s) URL of a job's page"
        )));
    };
    if let Some(name) = account
        && !config.accounts.contains_key(name)
    {
        return Err(LocateError(format!(
            "no account named {name:?} is configured"
        )));
    }
    let mut on_host = 0usize;
    let mut found: Vec<JobTarget> = Vec::new();
    for (name, acct) in &config.accounts {
        if account.is_some_and(|wanted| wanted != name) {
            continue;
        }
        let Some(rest) = rest_on(acct, &url_origin, url) else {
            continue;
        };
        on_host += 1;
        if let Some((project, job_id)) = parse_path(acct.provider, &rest) {
            found.push(JobTarget {
                account: name.clone(),
                provider: acct.provider,
                project,
                job_id,
            });
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 if on_host > 0 => Err(LocateError(format!(
            "{url:?} is not a job's page: expected <host>/<group>/<project>/-/jobs/<id> on \
             GitLab, or https://github.com/<owner>/<repo>/actions/runs/<run>/job/<id> on GitHub"
        ))),
        0 => Err(LocateError(match account {
            Some(name) => format!("{url:?} is not on the host of account {name:?}"),
            None => format!(
                "{url:?} is not on the host of any configured account, so bridgewatch has no \
                 token for it"
            ),
        })),
        _ => Err(LocateError(format!(
            "{url:?} is on the host of more than one account ({}); name one with --account",
            found
                .iter()
                .map(|t| t.account.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The part of `url` after `account`'s web root, when the URL is on it: the
/// path below a GitLab `base_url` (path prefix included), or below a github
/// account's web origin. Query and fragment are dropped.
fn rest_on(account: &Account, url_origin: &str, url: &str) -> Option<String> {
    let root = match account.provider {
        Provider::Gitlab => account.base_url.trim().trim_end_matches('/').to_string(),
        Provider::Github => crate::client::github::web_origin(&account.base_url)
            .trim_end_matches('/')
            .to_string(),
    };
    let root_origin = crate::oauth::origin_of(&root)?;
    if root_origin != url_origin {
        return None;
    }
    let root_path = root
        .split_once("://")
        .map(|(_, r)| r)
        .and_then(|r| r.find('/').map(|i| r[i..].to_string()))
        .unwrap_or_default();
    let after_origin = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let path = after_origin
        .find('/')
        .map(|i| &after_origin[i..])
        .unwrap_or("/");
    let path = path.split(['?', '#']).next().unwrap_or("");
    let rest = path.strip_prefix(root_path.as_str())?;
    if !root_path.is_empty() && !rest.starts_with('/') {
        return None;
    }
    Some(rest.to_string())
}

/// `(project, job id)` out of the path below an account's root.
fn parse_path(provider: Provider, rest: &str) -> Option<(ProjectRef, u64)> {
    let rest = rest.trim_matches('/');
    match provider {
        Provider::Gitlab => {
            let (project, after) = rest.split_once("/-/jobs/")?;
            let id = after.split('/').next()?.parse::<u64>().ok()?;
            let project = project.trim_matches('/');
            if !project.contains('/') || !project.split('/').all(is_path_segment) {
                return None;
            }
            Some((ProjectRef::Path(project.to_string()), id))
        }
        Provider::Github => {
            let parts: Vec<&str> = rest.split('/').collect();
            match parts.as_slice() {
                [owner, repo, "actions", "runs", run, "job", id, ..]
                    if is_path_segment(owner)
                        && is_path_segment(repo)
                        && run.parse::<u64>().is_ok() =>
                {
                    let id = id.parse::<u64>().ok()?;
                    Some((ProjectRef::Path(format!("{owner}/{repo}")), id))
                }
                _ => None,
            }
        }
    }
}

/// A group, project, owner or repository name: the characters GitLab and
/// GitHub allow in one, and nothing that could escape a path segment.
fn is_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The end of a job's log, [`LOG_TAIL_BYTES`] at most read.
pub async fn log_tail(client: &dyn CiClient, target: &JobTarget) -> Result<LogTail, ClientError> {
    client
        .job_log_tail(&target.project, target.job_id, LOG_TAIL_BYTES)
        .await
}

/// Retry or play one job, and log that it was done.
///
/// ⛔ The client refuses a write unless the account has `actions = true`
/// ([`ClientError::ActionsDisabled`]), whoever asks; the caller's own check,
/// where it has one, is a courtesy and never the guard.
///
/// Every outcome is logged at `info`, refusals included: a write is rare, it
/// is the one thing this app does that changes somebody else's system, and
/// "did the tray just retry that?" deserves an answer in the log file.
pub async fn perform(
    client: &dyn CiClient,
    target: &JobTarget,
    action: JobAction,
) -> Result<JobActionOutcome, ClientError> {
    let result = match action {
        JobAction::Retry => client.retry_job(&target.project, target.job_id).await,
        JobAction::Play => client.play_job(&target.project, target.job_id).await,
    };
    match &result {
        Ok(outcome) => tracing::info!(
            account = %target.account,
            project = %target.project,
            job = target.job_id,
            action = action.as_str(),
            outcome = "done",
            new_job = ?outcome.job_id,
            "job action"
        ),
        Err(e) => tracing::info!(
            account = %target.account,
            project = %target.project,
            job = target.job_id,
            action = action.as_str(),
            outcome = "refused",
            error = %e,
            "job action"
        ),
    }
    result
}
