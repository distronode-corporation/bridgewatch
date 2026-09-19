//! The API clients, and the two seams that let everything above them be tested
//! without a network.
//!
//! [`Transport`] is the lower seam and abstracts HTTP: one method, no provider
//! in it, and [`FixtureTransport`] is what makes the whole verdict engine
//! testable from recorded bytes. [`CiClient`] is the upper seam and abstracts
//! the PROVIDER: the poller, the fixture recorder and the setup wizard hold one
//! of these and never name GitLab.

pub mod conditional;
pub mod fixture;
pub mod github;
pub mod gitlab;
pub mod http;
pub mod wire;

use std::sync::Arc;

pub use conditional::ConditionalTransport;
pub use fixture::FixtureTransport;
pub use github::GitHubClient;
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

    /// A LISTED pipeline's own jobs and its bridges: what the poller fetches
    /// for every row it refreshes.
    ///
    /// The default is [`Self::pipeline_jobs`] then [`Self::pipeline_bridges`],
    /// in that order, which is GitLab's answer and exactly the two requests the
    /// poller made before this method existed.
    ///
    /// ⛔ It takes the row and the query that listed it, not an id, because a
    /// provider may SYNTHESISE rows. GitHub's commit group is one: a group's id
    /// is its newest run's id, so an id alone cannot say whether the caller
    /// means that run (whose jobs are real) or the group (which has none of its
    /// own and whose bridges are its runs). The query can.
    async fn listed_detail(
        &self,
        project: &ProjectRef,
        row: &Pipeline,
        _query: &ListQuery,
    ) -> Result<(Vec<Job>, Vec<Bridge>), ClientError> {
        let jobs = self.pipeline_jobs(project, row.id).await?;
        let bridges = self.pipeline_bridges(project, row.id).await?;
        Ok((jobs, bridges))
    }

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
/// ⛔ The ONE place a provider is turned into a client, and the only place in
/// the tree that names both: the poller, the wizard and the CLI hold a
/// `dyn CiClient` and cannot tell which they have.
///
/// The `Result` remains although both arms now succeed. A client is built from
/// a credential and a transport, which is exactly the sort of thing that grows
/// a failure case (an App installation token has to be exchanged before it can
/// be used), and an infallible signature here would make adding one a change at
/// every call site.
///
/// ⚠️ GitHub's client is handed its transport wrapped in a
/// [`ConditionalTransport`], one per client and so one per account: GitHub does
/// not charge an authenticated 304 against the rate limit, which is what lets a
/// settled watch poll for free. GitLab's is not wrapped. gitlab.com answers
/// `If-None-Match` too, but a 304 is a status GitLab's classifier reads as a
/// redirect, its budget is 24 times larger, and no recorded fixture carries an
/// `ETag` that could prove a GitLab watch reads the same with it on as off.
/// Switching it on is this line and a 304 arm in `gitlab::status_error`, and it
/// deserves a packet of its own.
pub fn client_for(
    account: &Account,
    token: &Secret,
    transport: Arc<dyn Transport>,
    ring: RequestRing,
) -> Result<Arc<dyn CiClient>, ClientError> {
    match account.provider {
        Provider::Gitlab => Ok(Arc::new(GitLabClient::new(account, token, transport, ring))),
        Provider::Github => {
            let conditional: Arc<dyn Transport> = Arc::new(ConditionalTransport::new(transport));
            Ok(Arc::new(GitHubClient::new(
                account,
                token,
                conditional,
                ring,
            )))
        }
    }
}

/// The sentence [`ClientError::Auth`] displays.
///
/// ⛔ GitLab's wording is byte for byte what it has always been. It is what a
/// GitLab user has already seen and learned to act on, and only the GitHub arm
/// is new; a "tidy" of the shared half would change an error message for people
/// this packet is not about.
///
/// GitHub's names both credential kinds because the two look for different
/// things: a classic token has a scope list with `repo` on it, a fine-grained
/// token has repository permissions and no scope list, and `actions:read` is
/// not a classic scope anybody will find on the token page.
fn auth_message(status: &u16, provider: &Provider) -> String {
    match provider {
        Provider::Gitlab => format!(
            "not authorised ({status}): check the token's scope (read_api) and that it can see this project"
        ),
        Provider::Github => format!(
            "not authorised ({status}): check that the token can read Actions on this \
             repository (a classic token needs the repo scope for a private repository; a \
             fine-grained token needs Actions: read)"
        ),
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
    ///
    /// ⚠️ The advice differs per provider, so the provider travels with the
    /// error rather than being looked up by whoever prints it: `read_api` is a
    /// GitLab scope name that exists nowhere on GitHub, and a fine-grained
    /// GitHub token has no scope list to check at all.
    #[error("{}", auth_message(.status, .provider))]
    Auth {
        /// The status that was returned.
        status: u16,
        /// Which provider refused it.
        provider: Provider,
    },
    /// 404: the project, pipeline or job does not exist, or the token cannot
    /// see it, which GitLab deliberately does not distinguish.
    #[error("not found: {path}")]
    NotFound {
        /// The path that 404'd.
        path: String,
    },
    /// Rate limited: a 429, or on GitHub a 403 that carries a rate-limit
    /// signal. See [`github::status_error`].
    #[error("rate limited; retry after {}s", retry_after.map(|r| r.to_string()).unwrap_or_else(|| "?".into()))]
    RateLimited {
        /// `retry-after` in seconds, when the server supplied one.
        retry_after: Option<u64>,
        /// `x-ratelimit-reset`, a Unix timestamp, when the server supplied one.
        ///
        /// ⚠️ GitHub sends `retry-after` only for a SECONDARY limit; when the
        /// primary hourly budget is exhausted the recovery time is here and
        /// nowhere else, so a backoff that read only `retry_after` would fall
        /// back to doubling an interval against a window that is an hour long.
        /// [`crate::poll::PollPolicy::on_error`] derives one from the other.
        /// GitLab's classifier leaves this `None`: it sends `retry-after` on
        /// the 429s that matter, and reading its reset header here would change
        /// a backoff that is doing its job.
        reset: Option<u64>,
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
    /// The request cannot be made as asked, decided before anything is sent.
    ///
    /// ⚠️ This replaced `UnsupportedProvider`, which existed only while there
    /// was a provider with no client. Both of today's uses are GitHub's and
    /// both are decisions rather than responses: a repository addressed by a
    /// numeric id, which GitHub has no URL for, and a pagination `Link` naming
    /// another host, which must not be followed with the token attached.
    #[error("{message}")]
    Unsupported {
        /// What cannot be done, and what to write instead.
        message: String,
    },
}

impl ClientError {
    /// True when retrying sooner is pointless and the user has to do something.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            ClientError::Auth { .. } | ClientError::Unsupported { .. }
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
            ClientError::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// The Unix timestamp the rate limit resets at, when the server gave one.
    /// A caller uses it only where [`ClientError::retry_after`] answered
    /// nothing; see the field's documentation.
    pub fn rate_limit_reset(&self) -> Option<u64> {
        match self {
            ClientError::RateLimited { reset, .. } => *reset,
            _ => None,
        }
    }
}
