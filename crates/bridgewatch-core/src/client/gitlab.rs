//! The GitLab endpoints bridgewatch uses, and nothing else.
//!
//! The poller's five are all `GET` under `{base_url}{api_path}/projects/{project}`.
//! The setup wizard adds four more, also `GET`: `/user`,
//! `/personal_access_tokens/self`, `/projects` and `/projects/{project}`.
//! Pagination follows `x-next-page`, which is the only paging header GitLab
//! guarantees for these collections.

use std::sync::Arc;

use serde::de::DeserializeOwned;

use super::ClientError;
use super::http::{HttpRequest, RequestLog, RequestRing, Transport};
use crate::config::{Account, ProjectRef};
use crate::model::{Bridge, Job, Pipeline, Project, TokenInfo, User};
use crate::token::Secret;

/// How to list pipelines.
///
/// `source` is only sent when exactly one is configured: GitLab's `source`
/// parameter takes a single value, so anything else is filtered client-side.
/// Likewise `ref` is only sent for an exact ref, because a glob or regex cannot
/// be expressed to the API.
#[derive(Debug, Clone, Default)]
pub struct ListQuery {
    /// Exact ref to ask for, when the watch's pattern is an exact one.
    pub ref_name: Option<String>,
    /// Single pipeline source to ask for.
    pub source: Option<String>,
    /// How many rows to request.
    pub per_page: u32,
    /// `id` for an exact ref (newest pipeline first, deterministically);
    /// `updated_at` when filtering client-side, so recently-touched pipelines
    /// on other refs are not missed.
    pub order_by: &'static str,
}

impl ListQuery {
    /// The cheap, exact form: one ref, optionally one source.
    pub fn exact(ref_name: impl Into<String>, source: Option<String>, per_page: u32) -> Self {
        Self {
            ref_name: Some(ref_name.into()),
            source,
            per_page,
            order_by: "id",
        }
    }

    /// The scanning form, for a glob or regex ref: recent pipelines across all
    /// refs, filtered client-side.
    pub fn scan(per_page: u32) -> Self {
        Self {
            ref_name: None,
            source: None,
            per_page,
            order_by: "updated_at",
        }
    }

    fn to_query(&self) -> String {
        let mut parts = Vec::new();
        if let Some(r) = &self.ref_name {
            parts.push(format!("ref={}", urlencoding::encode(r)));
        }
        if let Some(s) = &self.source {
            parts.push(format!("source={}", urlencoding::encode(s)));
        }
        parts.push(format!("order_by={}", self.order_by));
        parts.push("sort=desc".to_string());
        parts.push(format!("per_page={}", self.per_page.clamp(1, 100)));
        parts.join("&")
    }
}

/// A client for one GitLab account.
///
/// Cloning is cheap: the transport, the ring and the token are shared.
///
/// ⛔ `Debug` is hand-written. The derived one printed `header_value`, which is
/// the token itself for a `PRIVATE-TOKEN` account: the whole point of
/// [`Secret`] is that a credential cannot reach a log line by accident, and a
/// struct holding the already-rendered header value undid that for anyone who
/// wrote `{:?}` on a client.
#[derive(Clone)]
pub struct GitLabClient {
    base_url: String,
    api_path: String,
    header_name: &'static str,
    header_value: String,
    transport: Arc<dyn Transport>,
    ring: RequestRing,
}

impl std::fmt::Debug for GitLabClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitLabClient")
            .field("base_url", &self.base_url)
            .field("api_path", &self.api_path)
            .field("header_name", &self.header_name)
            .field("header_value", &super::http::REDACTED)
            .field("transport", &self.transport)
            .finish()
    }
}

impl GitLabClient {
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

    /// `GET /projects/{project}/pipelines`.
    pub async fn list_pipelines(
        &self,
        project: &ProjectRef,
        query: &ListQuery,
    ) -> Result<Vec<Pipeline>, ClientError> {
        let path = format!(
            "/projects/{}/pipelines?{}",
            project.url_segment(),
            query.to_query()
        );
        self.get_json(&path).await
    }

    /// `GET /projects/{project}/pipelines/{id}`.
    pub async fn get_pipeline(
        &self,
        project: &ProjectRef,
        id: u64,
    ) -> Result<Pipeline, ClientError> {
        let path = format!("/projects/{}/pipelines/{id}", project.url_segment());
        self.get_json(&path).await
    }

    /// `GET /projects/{project}/pipelines/{id}/jobs`, all pages.
    pub async fn pipeline_jobs(
        &self,
        project: &ProjectRef,
        id: u64,
    ) -> Result<Vec<Job>, ClientError> {
        let base = format!("/projects/{}/pipelines/{id}/jobs", project.url_segment());
        self.get_paginated(&base).await
    }

    /// `GET /projects/{project}/pipelines/{id}/bridges`, all pages.
    pub async fn pipeline_bridges(
        &self,
        project: &ProjectRef,
        id: u64,
    ) -> Result<Vec<Bridge>, ClientError> {
        let base = format!("/projects/{}/pipelines/{id}/bridges", project.url_segment());
        self.get_paginated(&base).await
    }

    /// The jobs of a child pipeline, which may live in another project.
    ///
    /// A multi-project trigger creates its child elsewhere; asking the parent's
    /// project for those jobs returns 404, which is why the downstream's own
    /// `project_id` is threaded through.
    pub async fn child_jobs(
        &self,
        child_project: &ProjectRef,
        child_id: u64,
    ) -> Result<Vec<Job>, ClientError> {
        self.pipeline_jobs(child_project, child_id).await
    }

    /// `GET /user`: who the token authenticates as.
    pub async fn current_user(&self) -> Result<User, ClientError> {
        self.get_json("/user").await
    }

    /// `GET /personal_access_tokens/self`: the token's own name, scopes and
    /// expiry. Older instances and non-PAT credentials (OAuth, job tokens)
    /// answer 401/403/404, which the wizard treats as "kind unknown".
    pub async fn token_self(&self) -> Result<TokenInfo, ClientError> {
        self.get_json("/personal_access_tokens/self").await
    }

    /// `GET /projects?membership=true`, most recently active first, following
    /// `x-next-page` for at most `max_pages` pages of 100.
    ///
    /// Returns the projects and whether the listing was cut short: a user in a
    /// large group can see thousands, and the wizard wants a search box, not a
    /// ten-second wait.
    pub async fn list_projects(
        &self,
        search: Option<&str>,
        max_pages: u32,
    ) -> Result<(Vec<Project>, bool), ClientError> {
        let mut query = String::from(
            "membership=true&simple=true&archived=false&order_by=last_activity_at&sort=desc",
        );
        if let Some(s) = search.map(str::trim).filter(|s| !s.is_empty()) {
            query.push_str(&format!("&search={}", urlencoding::encode(s)));
        }
        self.get_paginated_capped(&format!("/projects?{query}&"), max_pages.max(1))
            .await
    }

    /// `GET /projects/{project}`.
    pub async fn project(&self, project: &ProjectRef) -> Result<Project, ClientError> {
        self.get_json(&format!("/projects/{}", project.url_segment()))
            .await
    }

    /// Follow `x-next-page` from `prefix` (which ends in `?` or `&`) for at
    /// most `max_pages` pages. The flag is true when a next page existed and
    /// was not fetched.
    async fn get_paginated_capped<T: DeserializeOwned>(
        &self,
        prefix: &str,
        max_pages: u32,
    ) -> Result<(Vec<T>, bool), ClientError> {
        let mut out: Vec<T> = Vec::new();
        let mut page = 1u32;
        let mut fetched = 0u32;
        loop {
            let path = format!("{prefix}per_page=100&page={page}");
            let (items, next) = self.get_json_paged::<Vec<T>>(&path).await?;
            out.extend(items);
            fetched += 1;
            match next {
                Some(n) if n > page && fetched >= max_pages => return Ok((out, true)),
                Some(n) if n > page => page = n,
                _ => return Ok((out, false)),
            }
        }
    }

    async fn get_paginated<T: DeserializeOwned>(&self, base: &str) -> Result<Vec<T>, ClientError> {
        let mut out: Vec<T> = Vec::new();
        let mut page = 1u32;
        loop {
            let path = format!("{base}?per_page=100&page={page}");
            let (items, next) = self.get_json_paged::<Vec<T>>(&path).await?;
            out.extend(items);
            match next {
                // GitLab returns the next page number; trust it, but never loop
                // forever if an instance echoes the current page back.
                Some(n) if n > page => page = n,
                _ => break,
            }
        }
        Ok(out)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.get_json_paged::<T>(path).await.map(|(v, _)| v)
    }

    async fn get_json_paged<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<(T, Option<u32>), ClientError> {
        let url = format!("{}{}{}", self.base_url, self.api_path, path);
        let request = HttpRequest {
            method: "GET",
            url,
            path: path.to_string(),
            headers: vec![(self.header_name.to_string(), self.header_value.clone())],
        };

        let started = std::time::Instant::now();
        let result = self.transport.execute(request).await;
        let ms = started.elapsed().as_millis() as u64;

        match result {
            Ok(response) => {
                let error = status_error(response.status, path, response.retry_after);
                // ⛔ `path` is logged whole, query and all, and that is safe by
                // CONSTRUCTION rather than by filtering: the credential travels
                // only in a header (see the module docs on `client::http`), and
                // every path here is built from a fixed template with its one
                // variable percent-encoded. Redacting a query parameter that
                // cannot exist would suggest that one could.
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
                Ok((value, response.next_page.and_then(|p| p.parse().ok())))
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

/// Map an HTTP status onto a [`ClientError`], or `None` when it is a success.
pub fn status_error(status: u16, path: &str, retry_after: Option<u64>) -> Option<ClientError> {
    match status {
        200..=299 => None,
        401 | 403 => Some(ClientError::Auth { status }),
        404 => Some(ClientError::NotFound {
            path: path.to_string(),
        }),
        // ⛔ Before `Unexpected`, and never followed: the transport refuses
        // redirects so the credential header cannot reach another host, which
        // makes a 3xx a response bridgewatch has to explain rather than a
        // status it merely did not expect.
        300..=399 => Some(ClientError::Redirect {
            status,
            path: path.to_string(),
        }),
        429 => Some(ClientError::RateLimited { retry_after }),
        500..=599 => Some(ClientError::Server { status }),
        other => Some(ClientError::Unexpected {
            status: other,
            path: path.to_string(),
        }),
    }
}
