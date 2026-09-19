//! The GitHub Actions endpoints bridgewatch uses, and nothing else.
//!
//! # How a workflow run becomes a pipeline
//!
//! GitHub has no object above a workflow run, so in this build **one run is one
//! [`Pipeline`]** and a watch names the workflow it wants:
//!
//! | model | from |
//! | --- | --- |
//! | `Pipeline.id` | run `id` |
//! | `Pipeline.iid` | run `run_number`, the `#42` the UI shows |
//! | `Pipeline.sha` / `ref_name` / `source` | `head_sha` / `head_branch` / `event` |
//! | `Pipeline.status` | [`Status::from_github`] over `status` + `conclusion` |
//! | `Pipeline.web_url` | `html_url` |
//! | `Pipeline.created_at` / `updated_at` / `started_at` | `created_at` / `updated_at` / `run_started_at` |
//! | `Job.*` | the run's `/jobs`, `filter=latest` |
//! | `Job.stage` | the caller of a `caller / callee` job name, else none |
//! | `Job.allow_failure` | always `false`: the API has no such field |
//! | `Bridge` | **none.** A run has no trigger jobs |
//!
//! ⛔ [`CiClient::pipeline_bridges`] therefore answers with an empty list and
//! issues no request, which is a real answer rather than a gap: a run's jobs are
//! all in one list, reusable workflows included. Folding the several runs of one
//! push into a synthetic parent whose bridges are those runs is the commit
//! group, and it arrives with the configuration that switches it on.
//!
//! # What this module has to be careful about
//!
//! ⛔ **Pagination is a `Link` header, and its URLs are rewritten** to
//! `/repositories/<id>/...`, so a next page is FOLLOWED verbatim and never
//! rebuilt. The relations also change order between pages, so it is parsed by
//! `rel=` and never by position. A `Link` pointing at another host is refused
//! outright: following it would send the credential there.
//!
//! ⛔ **A 403 is not necessarily an auth failure.** An exhausted primary rate
//! limit arrives as a 403 (or a 429) carrying `x-ratelimit-remaining: 0`, and a
//! secondary limit as a 403 carrying `retry-after`. Classifying either as
//! [`ClientError::Auth`] parks the account as a bad token, which is why
//! [`status_error`] here is not the GitLab one.
//!
//! Every response is decoded into [`super::wire::github`] and converted into
//! [`crate::model`] before it leaves this module.

use std::sync::Arc;

use serde::de::DeserializeOwned;

use super::http::{HttpRequest, HttpResponse, RequestLog, RequestRing, Transport};
use super::wire::github as wire;
use super::{CiClient, ClientError, ListQuery};
use crate::config::{Account, ProjectRef, Provider};
use crate::model::{Bridge, Job, Pipeline, Project, TokenInfo, User};
use crate::token::Secret;

/// The REST API version this client pins.
///
/// GitHub versions its REST API by date and requires the header on every
/// request; a version is supported for at least 24 months after a newer one
/// ships, so pinning is the safe form and omitting it means "whatever is
/// current", which can change a response shape under a running tray.
/// `2022-11-28` is what `gh` itself sends today and what the responses this
/// client's decoders were written against were fetched with.
pub const API_VERSION: &str = "2022-11-28";

/// The media type GitHub wants for its JSON API.
pub const ACCEPT: &str = "application/vnd.github+json";

/// How many pages of 100 jobs one run may spend.
///
/// A `Link` chain is a sequence of URLs the SERVER chooses, so the loop that
/// follows it needs a bound of its own: without one a server that pointed page
/// 2 back at page 1 would be an infinite request loop, which is the GitHub
/// analogue of the "instance echoes the current page back" case GitLab's pager
/// already guards. A thousand jobs is far past anything a tray can draw; the
/// largest run either research lane measured was 108.
pub const JOB_PAGES: u32 = 10;

/// A client for one GitHub account.
///
/// Cloning is cheap: the transport, the ring and the token are shared.
///
/// ⛔ `Debug` is hand-written for the same reason [`super::GitLabClient`]'s is:
/// the derived one printed the rendered credential header.
#[derive(Clone)]
pub struct GitHubClient {
    base_url: String,
    api_path: String,
    header_name: &'static str,
    header_value: String,
    transport: Arc<dyn Transport>,
    ring: RequestRing,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("base_url", &self.base_url)
            .field("api_path", &self.api_path)
            .field("header_name", &self.header_name)
            .field("header_value", &super::http::REDACTED)
            .field("transport", &self.transport)
            .finish()
    }
}

impl GitHubClient {
    /// Build a client from an account definition and an already-resolved token.
    pub fn new(
        account: &Account,
        token: &Secret,
        transport: Arc<dyn Transport>,
        ring: RequestRing,
    ) -> Self {
        Self {
            base_url: account.base_url.trim_end_matches('/').to_string(),
            api_path: account.api_path.clone(),
            header_name: account.header.header_name(),
            header_value: account.header.header_value(token.expose()),
            transport,
            ring,
        }
    }

    /// The request ring this client records into.
    pub fn ring(&self) -> &RequestRing {
        &self.ring
    }

    /// `GET /repos/{owner}/{repo}/actions/runs`, or that workflow's own runs
    /// when the watch named one.
    ///
    /// `branch` and `event` filter server-side **together**, which GitLab's
    /// `source` cannot do. ⚠️ [`ListQuery::order_by`] is ignored: GitHub always
    /// returns newest first and offers no ordering parameter, which is the
    /// ordering both of `order_by`'s values were asking for anyway.
    pub async fn list_pipelines(
        &self,
        project: &ProjectRef,
        query: &ListQuery,
    ) -> Result<Vec<Pipeline>, ClientError> {
        let repo = repo_segment(project)?;
        let mut parts: Vec<String> = Vec::new();
        if let Some(r) = &query.ref_name {
            parts.push(format!("branch={}", urlencoding::encode(r)));
        }
        if let Some(s) = &query.source {
            parts.push(format!("event={}", urlencoding::encode(s)));
        }
        // Drops the `pull_requests` array, which is never read and was 45
        // entries on one sampled run.
        parts.push("exclude_pull_requests=true".to_string());
        parts.push(format!("per_page={}", query.per_page.clamp(1, 100)));
        parts.push("page=1".to_string());
        let path = match query
            .workflow
            .as_deref()
            .map(str::trim)
            .filter(|w| !w.is_empty())
        {
            Some(workflow) => format!(
                "/repos/{repo}/actions/workflows/{}/runs?{}",
                urlencoding::encode(workflow),
                parts.join("&")
            ),
            None => format!("/repos/{repo}/actions/runs?{}", parts.join("&")),
        };
        let (page, _) = self.get_json::<wire::RunsResponse>(&path).await?;
        Ok(page.workflow_runs.into_iter().map(Into::into).collect())
    }

    /// `GET /repos/{owner}/{repo}/actions/runs/{id}`.
    pub async fn get_pipeline(
        &self,
        project: &ProjectRef,
        id: u64,
    ) -> Result<Pipeline, ClientError> {
        let path = format!("/repos/{}/actions/runs/{id}", repo_segment(project)?);
        let (run, _) = self.get_json::<wire::WorkflowRun>(&path).await?;
        Ok(run.into())
    }

    /// `GET /repos/{owner}/{repo}/actions/runs/{id}/jobs`, all pages.
    ///
    /// `filter=latest` is pinned rather than relied on as a default: a re-run
    /// bumps `run_attempt` on the SAME run id, and the alternative value
    /// (`all`) would list every superseded attempt's jobs as though they were
    /// still current.
    pub async fn pipeline_jobs(
        &self,
        project: &ProjectRef,
        id: u64,
    ) -> Result<Vec<Job>, ClientError> {
        let path = format!(
            "/repos/{}/actions/runs/{id}/jobs?filter=latest&per_page=100",
            repo_segment(project)?
        );
        let (pages, more) = self
            .get_paginated::<wire::JobsResponse>(&path, JOB_PAGES)
            .await?;
        if more {
            // Said out loud rather than silently truncated: an under-reported
            // job list reads as a run with nothing wrong in it.
            tracing::warn!(
                run = id,
                pages = JOB_PAGES,
                "stopped following the jobs pagination at the page cap; \
                 this run's job list is incomplete"
            );
        }
        Ok(pages
            .into_iter()
            .flat_map(|p| p.jobs)
            .map(Into::into)
            .collect())
    }

    /// `GET /user`: who the token authenticates as.
    pub async fn current_user(&self) -> Result<User, ClientError> {
        let (user, _) = self.get_json::<wire::User>("/user").await?;
        Ok(user.into())
    }

    /// What can be said about the token itself, which on GitHub is not much.
    ///
    /// ⛔ There is no introspection endpoint: nothing answers "what is this
    /// token". What exists is a response header, `X-OAuth-Scopes`, which a
    /// **classic** personal access token gets on every response and a
    /// fine-grained token or App installation token never gets at all. So the
    /// scopes are read from `GET /user`'s own response, and the absence of the
    /// header is reported as "could not be established", the same answer a
    /// GitLab instance too old for `/personal_access_tokens/self` gives, and the
    /// one the wizard already turns into `TokenKind::Unknown`.
    ///
    /// ⚠️ Nothing here is invented. There is no token id (the user's own id
    /// stands in, so the field can be filled at all), no token name, no
    /// `active` flag, and no expiry: GitHub does send an expiry header on some
    /// credentials, but reporting it would need a second header field and the
    /// wizard does not draw it yet.
    pub async fn token_self(&self) -> Result<TokenInfo, ClientError> {
        let (user, meta) = self.get_json::<wire::User>("/user").await?;
        let Some(raw) = meta.oauth_scopes else {
            return Err(ClientError::NotFound {
                path: "the token's scopes (GitHub has no token-introspection endpoint, and \
                       this credential sent no X-OAuth-Scopes header, which is what a \
                       fine-grained token does)"
                    .to_string(),
            });
        };
        Ok(TokenInfo {
            id: user.id,
            name: None,
            scopes: raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            expires_at: None,
            active: None,
        })
    }

    /// `GET /user/repos`, most recently pushed first, for at most `max_pages`
    /// pages of 100.
    ///
    /// ⚠️ `search` filters **the pages that were fetched**, not the account.
    /// GitHub's repository search is a different endpoint with its own much
    /// smaller rate-limit budget and a query grammar of its own, and it cannot
    /// be asked for "mine" without first knowing the login; filtering what is
    /// already in hand keeps the picker to one budget and one request shape.
    /// The consequence is worth saying out loud: a repository outside the newest
    /// `max_pages × 100` is typed in rather than picked, which is exactly what
    /// the truncated flag is for.
    pub async fn list_projects(
        &self,
        search: Option<&str>,
        max_pages: u32,
    ) -> Result<(Vec<Project>, bool), ClientError> {
        let path = "/user/repos?sort=pushed&direction=desc&per_page=100&page=1";
        let (pages, truncated) = self
            .get_paginated::<Vec<wire::Repository>>(path, max_pages.max(1))
            .await?;
        let needle = search
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase);
        let projects = pages
            .into_iter()
            .flatten()
            .map(Project::from)
            .filter(|p| match &needle {
                Some(n) => p.path_with_namespace.to_lowercase().contains(n),
                None => true,
            })
            .collect();
        Ok((projects, truncated))
    }

    /// `GET /repos/{owner}/{repo}`.
    pub async fn project(&self, project: &ProjectRef) -> Result<Project, ClientError> {
        let path = format!("/repos/{}", repo_segment(project)?);
        let (repo, _) = self.get_json::<wire::Repository>(&path).await?;
        Ok(repo.into())
    }

    /// Follow `Link: rel="next"` from `first`, which already carries its query,
    /// for at most `max_pages` pages.
    ///
    /// The flag is true when a next page existed and was not fetched, which is
    /// the same answer GitLab's capped pager gives from its next-page number.
    async fn get_paginated<T: DeserializeOwned>(
        &self,
        first: &str,
        max_pages: u32,
    ) -> Result<(Vec<T>, bool), ClientError> {
        let mut out = Vec::new();
        let mut path = first.to_string();
        for _ in 0..max_pages.max(1) {
            let (value, meta) = self.get_json::<T>(&path).await?;
            out.push(value);
            let Some(next) = meta.link.as_deref().and_then(next_link) else {
                return Ok((out, false));
            };
            path = self.local_path(&next)?;
        }
        Ok((out, true))
    }

    /// Turn an absolute next-page URL from a `Link` header into a path this
    /// client may request.
    ///
    /// ⛔ **The origin is checked, and a foreign one is refused rather than
    /// followed.** The URL comes from a response, and the request built from it
    /// carries the credential header: following it to whatever host a header
    /// names is the same hole as following a redirect, which
    /// [`super::http::ReqwestTransport`] refuses for exactly this reason.
    /// Refusing loudly rather than stopping quietly is deliberate too, because
    /// the failure a silent stop produces is a short job list that looks like a
    /// short run.
    fn local_path(&self, url: &str) -> Result<String, ClientError> {
        let base_origin = origin_of(&self.base_url);
        let next_origin = origin_of(url);
        if next_origin.is_empty() || next_origin != base_origin {
            return Err(ClientError::Unsupported {
                message: format!(
                    "the next page of results is at {next_origin:?}, which is not this \
                     account's {base_origin:?}. bridgewatch will not send the token to a \
                     host a response header named; check base_url and any proxy in front \
                     of the API"
                ),
            });
        }
        let rest = &url[next_origin.len()..];
        // The prefix is stripped when it is there so the request log reads like
        // every other line. GitHub rewrites the path itself (to
        // `/repositories/<id>/...`), which is left exactly as it came.
        Ok(match rest.strip_prefix(self.api_path.as_str()) {
            Some(p) if !self.api_path.is_empty() => p.to_string(),
            _ => rest.to_string(),
        })
    }

    /// One `GET`, decoded, with the response metadata the caller may need.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<(T, ResponseMeta), ClientError> {
        let url = format!("{}{}{}", self.base_url, self.api_path, path);
        let request = HttpRequest {
            method: "GET",
            url,
            path: path.to_string(),
            headers: vec![
                (self.header_name.to_string(), self.header_value.clone()),
                ("Accept".to_string(), ACCEPT.to_string()),
                ("X-GitHub-Api-Version".to_string(), API_VERSION.to_string()),
            ],
        };

        let started = std::time::Instant::now();
        let result = self.transport.execute(request).await;
        let ms = started.elapsed().as_millis() as u64;

        match result {
            Ok(response) => {
                let error = status_error(response.status, path, &response);
                // Safe to log whole for the same reason GitLab's path is: the
                // credential travels only in a header, and every path here is a
                // fixed template with its variables percent-encoded.
                tracing::debug!(
                    method = "GET",
                    path,
                    status = response.status,
                    bytes = response.body.len(),
                    ms,
                    "request"
                );
                self.ring.record(RequestLog {
                    method: "GET".into(),
                    path: path.to_string(),
                    status: Some(response.status),
                    ms,
                    ratelimit_remaining: response.ratelimit_remaining,
                    ratelimit_reset: response.ratelimit_reset,
                    retry_after: response.retry_after,
                    error: error.as_ref().map(ToString::to_string),
                    at: chrono::Utc::now(),
                });
                if let Some(e) = error {
                    return Err(e);
                }
                let value =
                    serde_json::from_str::<T>(&response.body).map_err(|e| ClientError::Decode {
                        path: path.to_string(),
                        message: e.to_string(),
                    })?;
                Ok((
                    value,
                    ResponseMeta {
                        link: response.link,
                        oauth_scopes: response.oauth_scopes,
                    },
                ))
            }
            Err(e) => {
                tracing::debug!(method = "GET", path, error = %e, ms, "request failed");
                self.ring.record(RequestLog {
                    method: "GET".into(),
                    path: path.to_string(),
                    status: None,
                    ms,
                    ratelimit_remaining: None,
                    ratelimit_reset: None,
                    retry_after: e.retry_after(),
                    error: Some(e.to_string()),
                    at: chrono::Utc::now(),
                });
                Err(e)
            }
        }
    }
}

/// What a caller may need from the response beyond its body.
struct ResponseMeta {
    link: Option<String>,
    oauth_scopes: Option<String>,
}

/// The path segment for `/repos/{...}`.
///
/// ⛔ GitHub addresses a repository as `owner/repo` with the slash **left
/// alone**, which is the opposite of GitLab's percent-encoded
/// `group%2Fproject`. Each segment is still encoded individually, so a name
/// with an unusual character cannot break out of its segment.
///
/// A numeric project is refused with an explanation rather than sent: there is
/// no `/repos/<id>` endpoint, and a numeric id in a GitHub watch is nearly
/// always a GitLab project id left behind by moving a watch between accounts.
/// `validate` says the same thing about the file before it ever loads; this is
/// the second line of the same defence.
fn repo_segment(project: &ProjectRef) -> Result<String, ClientError> {
    match project {
        ProjectRef::Path(_) => Ok(project.url_segment_for(Provider::Github)),
        ProjectRef::Id(id) => Err(ClientError::Unsupported {
            message: format!(
                "GitHub addresses a repository as owner/repo, and this watch names the \
                 numeric project {id}. Write project = \"owner/repo\""
            ),
        }),
    }
}

/// The `rel="next"` URL of a `Link` header, or `None` when there is not one.
///
/// ⚠️ Parsed by relation and never by position: measured on two consecutive
/// pages of the same listing, page 1 answered `next, last` and page 2 answered
/// `prev, next, last, first`. Taking "the first URL" would have paged forwards
/// once and then backwards forever.
/// ⚠️ Scanned `<...>` by `<...>` rather than split on commas: a comma is the
/// separator BETWEEN links and is also legal inside a URL's query, so splitting
/// on it first can cut a link in half.
pub fn next_link(header: &str) -> Option<String> {
    let mut rest = header;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let close = after.find('>')?;
        let url = &after[..close];
        let tail = &after[close + 1..];
        // The parameters of this link run up to the next one. ⚠️ Split on the
        // comma as well as the semicolon: the last parameter before the next
        // link keeps the separator that followed it, so an exact comparison
        // against `rel="next"` would miss `rel="next",`.
        let end = tail.find('<').unwrap_or(tail.len());
        let is_next = tail[..end]
            .split([';', ','])
            .map(str::trim)
            .any(|p| p.eq_ignore_ascii_case("rel=\"next\"") || p.eq_ignore_ascii_case("rel=next"));
        if is_next {
            return Some(url.trim().to_string());
        }
        rest = &tail[end..];
    }
    None
}

/// `scheme://host[:port]` of a URL, or an empty string when it has no scheme.
fn origin_of(url: &str) -> &str {
    let Some(after_scheme) = url.find("://").map(|i| i + 3) else {
        return "";
    };
    match url[after_scheme..].find(['/', '?', '#']) {
        Some(end) => &url[..after_scheme + end],
        None => url,
    }
}

/// The provider-neutral surface, delegating to the inherent methods above.
#[async_trait::async_trait]
impl CiClient for GitHubClient {
    fn ring(&self) -> &RequestRing {
        GitHubClient::ring(self)
    }

    async fn list_pipelines(
        &self,
        project: &ProjectRef,
        query: &ListQuery,
    ) -> Result<Vec<Pipeline>, ClientError> {
        GitHubClient::list_pipelines(self, project, query).await
    }

    async fn get_pipeline(&self, project: &ProjectRef, id: u64) -> Result<Pipeline, ClientError> {
        GitHubClient::get_pipeline(self, project, id).await
    }

    async fn pipeline_jobs(&self, project: &ProjectRef, id: u64) -> Result<Vec<Job>, ClientError> {
        GitHubClient::pipeline_jobs(self, project, id).await
    }

    /// Empty, and no request: a workflow run has no trigger jobs. See the
    /// module documentation on the commit group.
    async fn pipeline_bridges(
        &self,
        _project: &ProjectRef,
        _id: u64,
    ) -> Result<Vec<Bridge>, ClientError> {
        Ok(Vec::new())
    }

    /// Empty, and no request. Children are reached through bridges, and there
    /// are none, so nothing can ask for this; answering with an error would
    /// make a future caller's mistake look like an outage.
    async fn child_jobs(
        &self,
        _child_project: &ProjectRef,
        _child_id: u64,
    ) -> Result<Vec<Job>, ClientError> {
        Ok(Vec::new())
    }

    async fn current_user(&self) -> Result<User, ClientError> {
        GitHubClient::current_user(self).await
    }

    async fn token_self(&self) -> Result<TokenInfo, ClientError> {
        GitHubClient::token_self(self).await
    }

    async fn list_projects(
        &self,
        search: Option<&str>,
        max_pages: u32,
    ) -> Result<(Vec<Project>, bool), ClientError> {
        GitHubClient::list_projects(self, search, max_pages).await
    }

    async fn project(&self, project: &ProjectRef) -> Result<Project, ClientError> {
        GitHubClient::project(self, project).await
    }
}

/// Map an HTTP status onto a [`ClientError`], or `None` when it is a success.
///
/// ⛔ **This is not [`super::gitlab::status_error`], and the difference is the
/// single highest-severity thing a naive port would have inherited.** GitLab
/// signals rate limiting with a 429 and nothing else, so it can read a 403 as
/// "not authorised". GitHub signals an exhausted PRIMARY limit with a **403 or
/// a 429 carrying `x-ratelimit-remaining: 0`**, and a SECONDARY limit with a
/// **403 carrying `retry-after`**. Reading either as [`ClientError::Auth`]
/// makes it fatal: the account parks itself with "check the token's scope" and
/// stays parked through a limit that would have cleared on its own.
///
/// So the signals decide, and only a 401, or a 403 with neither signal, which
/// is the shape of a token that genuinely cannot see the repository, is auth.
///
/// ⚠️ The signals are read from the RESPONSE HEADERS and never from
/// `GET /rate_limit`, which was measured reporting `used: 0` while a response
/// on the same token seconds earlier reported 138, on two different
/// fine-grained tokens on two machines. The headers are authoritative; the
/// endpoint is not, and it is not called.
pub fn status_error(status: u16, path: &str, response: &HttpResponse) -> Option<ClientError> {
    let exhausted = response.ratelimit_remaining == Some(0);
    let told_to_wait = response.retry_after.is_some();
    match status {
        200..=299 => None,
        401 => Some(ClientError::Auth { status }),
        403 if exhausted || told_to_wait => Some(ClientError::RateLimited {
            retry_after: response.retry_after,
            reset: response.ratelimit_reset,
        }),
        403 => Some(ClientError::Auth { status }),
        404 => Some(ClientError::NotFound {
            path: path.to_string(),
        }),
        // Never followed, for the same reason GitLab's are not: the credential
        // would travel to whatever host the redirect names.
        300..=399 => Some(ClientError::Redirect {
            status,
            path: path.to_string(),
        }),
        429 => Some(ClientError::RateLimited {
            retry_after: response.retry_after,
            reset: response.ratelimit_reset,
        }),
        500..=599 => Some(ClientError::Server { status }),
        other => Some(ClientError::Unexpected {
            status: other,
            path: path.to_string(),
        }),
    }
}
