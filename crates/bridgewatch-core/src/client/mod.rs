//! The API clients, and the two seams that let everything above them be tested
//! without a network.
//!
//! [`Transport`] is the lower seam and abstracts HTTP: one method, no provider
//! in it, and [`FixtureTransport`] is what makes the whole verdict engine
//! testable from recorded bytes. [`CiClient`] is the upper seam and abstracts
//! the PROVIDER: the poller, the fixture recorder and the setup wizard hold one
//! of these and never name GitLab.

pub mod fixture;
pub mod gitlab;
pub mod http;
pub mod wire;

use std::sync::Arc;

pub use fixture::FixtureTransport;
pub use gitlab::{GitLabClient, ListQuery};
pub use http::{HttpRequest, HttpResponse, RequestLog, RequestRing, ReqwestTransport, Transport};

use crate::config::{Account, ProjectRef, Provider};
use crate::model::{Bridge, Job, Pipeline, Project, TokenInfo, User};
use crate::token::Secret;

/// One CI provider, as everything above the client sees it.
///
/// The methods are exactly what the poller, the fixture recorder and the setup
/// wizard ask for, and the types they hand back are [`crate::model`]'s: a
/// provider's own JSON is decoded and converted inside its client, so nothing
/// above this trait has ever seen a wire type.
///
/// ⚠️ [`ListQuery`] is still spelled in GitLab's vocabulary (`order_by` is a
/// GitLab parameter name). It carries no GitLab semantics a second provider
/// cannot answer: a ref, an optional source, a page size, and a preference for
/// newest-first. A client that has no equivalent of a field ignores it.
#[async_trait::async_trait]
pub trait CiClient: Send + Sync + std::fmt::Debug {
    /// The request ring this client records into, for the debug pane.
    fn ring(&self) -> &RequestRing;

    /// Recent pipelines of a project, newest first.
    async fn list_pipelines(
        &self,
        project: &ProjectRef,
        query: &ListQuery,
    ) -> Result<Vec<Pipeline>, ClientError>;

    /// One pipeline by id.
    async fn get_pipeline(&self, project: &ProjectRef, id: u64) -> Result<Pipeline, ClientError>;

    /// Every job of a pipeline, all pages.
    async fn pipeline_jobs(&self, project: &ProjectRef, id: u64) -> Result<Vec<Job>, ClientError>;

    /// Every bridge (trigger job) of a pipeline, all pages. A provider with no
    /// such concept answers with an empty list rather than an error.
    async fn pipeline_bridges(
        &self,
        project: &ProjectRef,
        id: u64,
    ) -> Result<Vec<Bridge>, ClientError>;

    /// The jobs of a child pipeline, which may live in another project.
    async fn child_jobs(
        &self,
        child_project: &ProjectRef,
        child_id: u64,
    ) -> Result<Vec<Job>, ClientError>;

    /// Who the token authenticates as. The wizard's "test connection".
    async fn current_user(&self) -> Result<User, ClientError>;

    /// The token's own name, scopes and expiry, where the provider exposes
    /// them. A failure here is an answer ("kind unknown"), not an outage.
    async fn token_self(&self) -> Result<TokenInfo, ClientError>;

    /// Projects this token can pick from, and whether the listing was cut
    /// short.
    async fn list_projects(
        &self,
        search: Option<&str>,
        max_pages: u32,
    ) -> Result<(Vec<Project>, bool), ClientError>;

    /// One project by id or path.
    async fn project(&self, project: &ProjectRef) -> Result<Project, ClientError>;
}

/// Build the client an account's `provider` asks for.
///
/// ⛔ The ONE place a provider is turned into a client. Phase 2 of GitHub
/// support adds an arm here and changes nothing else: the poller, the wizard
/// and the CLI all hold a `dyn CiClient` and cannot tell the difference.
///
/// ⛔ An unimplemented provider is refused rather than approximated. Falling
/// through to a GitLab client pointed at `https://api.github.com` would issue
/// `/api/v4/projects/...` requests against GitHub, collect 404s, and present as
/// a wrong project id.
pub fn client_for(
    account: &Account,
    token: &Secret,
    transport: Arc<dyn Transport>,
    ring: RequestRing,
) -> Result<Arc<dyn CiClient>, ClientError> {
    match account.provider {
        Provider::Gitlab => Ok(Arc::new(GitLabClient::new(account, token, transport, ring))),
        Provider::Github => Err(ClientError::UnsupportedProvider {
            provider: Provider::Github.as_str(),
        }),
    }
}

/// Everything a GitLab call can fail with.
///
/// The variants are the ones the poller reacts to differently: `Auth` is worth
/// telling the user about once and not retrying at speed, `RateLimited` drives
/// the backoff, and `Transport` is usually somebody's Wi-Fi.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    /// 401 or 403: the token is missing, wrong, or lacks the scope.
    #[error(
        "not authorised ({status}): check the token's scope (read_api) and that it can see this project"
    )]
    Auth {
        /// The status that was returned.
        status: u16,
    },
    /// 404: the project, pipeline or job does not exist, or the token cannot
    /// see it, which GitLab deliberately does not distinguish.
    #[error("not found: {path}")]
    NotFound {
        /// The path that 404'd.
        path: String,
    },
    /// 429: rate limited.
    #[error("rate limited; retry after {}s", retry_after.map(|r| r.to_string()).unwrap_or_else(|| "?".into()))]
    RateLimited {
        /// `retry-after` in seconds, when GitLab supplied one.
        retry_after: Option<u64>,
    },
    /// 5xx.
    #[error("server error ({status})")]
    Server {
        /// The status that was returned.
        status: u16,
    },
    /// 3xx: the server wants us somewhere else, and we will not go.
    ///
    /// Following it would send the `PRIVATE-TOKEN` header to whatever host the
    /// redirect names, which is the one thing this client must never do.
    #[error(
        "{path} answered {status}, a redirect. bridgewatch does not follow redirects, \
         because the token header would travel to the redirect's target; check base_url \
         and api_path (an instance served under a path prefix, or http:// where the \
         server redirects to https://, is the usual cause)"
    )]
    Redirect {
        /// The 3xx status that was returned.
        status: u16,
        /// The path that redirected.
        path: String,
    },
    /// The request never completed: DNS, TLS, timeout, connection reset.
    #[error("transport error: {0}")]
    Transport(String),
    /// The response was not the JSON we expected.
    #[error("could not decode {path}: {message}")]
    Decode {
        /// The path whose body failed to decode.
        path: String,
        /// The decoder's complaint.
        message: String,
    },
    /// An unexpected status that is none of the above.
    #[error("unexpected status {status} for {path}")]
    Unexpected {
        /// The status that was returned.
        status: u16,
        /// The path that returned it.
        path: String,
    },
    /// The account names a provider this build has no client for. Raised by
    /// [`client_for`] before any request is made, never by a response.
    #[error(
        "{provider} accounts are not supported by this build of bridgewatch yet; \
         remove the account or set provider = \"gitlab\""
    )]
    UnsupportedProvider {
        /// The provider named in the configuration.
        provider: &'static str,
    },
}

impl ClientError {
    /// True when retrying sooner is pointless and the user has to do something.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            ClientError::Auth { .. } | ClientError::UnsupportedProvider { .. }
        )
    }

    /// True when the poller should back off rather than retry at pace.
    pub fn should_back_off(&self) -> bool {
        matches!(
            self,
            ClientError::RateLimited { .. }
                | ClientError::Server { .. }
                | ClientError::Transport(_)
        )
    }

    /// `retry-after`, when the server gave one.
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            ClientError::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}
