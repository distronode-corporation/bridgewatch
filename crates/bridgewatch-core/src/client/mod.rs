//! The GitLab API client, and the seam that lets everything above it be tested
//! without a network.

pub mod fixture;
pub mod gitlab;
pub mod http;

pub use fixture::FixtureTransport;
pub use gitlab::{GitLabClient, ListQuery};
pub use http::{HttpRequest, HttpResponse, RequestLog, RequestRing, ReqwestTransport, Transport};

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
}

impl ClientError {
    /// True when retrying sooner is pointless and the user has to do something.
    pub fn is_fatal(&self) -> bool {
        matches!(self, ClientError::Auth { .. })
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
