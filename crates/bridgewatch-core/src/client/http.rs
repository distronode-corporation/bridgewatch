//! The transport seam.
//!
//! Every GitLab call goes through [`Transport`]. The real implementation is
//! [`ReqwestTransport`]; the tests drive [`crate::client::fixture::FixtureTransport`],
//! so the whole verdict engine is exercised with no network at all.
//!
//! Every request is recorded in a bounded [`RequestRing`] that the GUI's debug
//! pane reads. Nothing recorded there can carry the token, by construction
//! rather than by filtering: a [`RequestLog`] has no field for headers at all,
//! and the credential travels only in a header, never in the path or query.
//! [`REDACTED`] is what the hand-written `Debug` impls print in place of a
//! header value, which is the one route a credential had into a log line.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::ClientError;

/// What a `Debug` rendering prints in place of a header value.
///
/// Used by [`HttpRequest`]'s and [`super::GitLabClient`]'s `Debug`, the two
/// types that hold the rendered credential header. It is the same text
/// [`crate::token::Secret`] prints, so a redaction reads the same wherever it
/// appears.
pub const REDACTED: &str = "<redacted>";

/// A request about to be executed.
///
/// ⛔ `Debug` is hand-written because the derived one printed `headers`
/// verbatim, credential and all: one `{:?}` in a `tracing` line, a panic
/// message or an `unwrap` on a `Result<_, _>` holding one of these would have
/// put the token in a log the user then pastes into an issue. See
/// [`REDACTED`].
#[derive(Clone)]
pub struct HttpRequest {
    /// HTTP method. bridgewatch only ever issues `GET`.
    pub method: &'static str,
    /// Fully-qualified URL.
    pub url: String,
    /// Path and query, relative to the API root. This is what the request log
    /// shows, because it is the part a human can act on.
    pub path: String,
    /// Headers to send, credentials included. Never recorded: [`RequestLog`]
    /// has no field for them, and `Debug` prints [`REDACTED`] for every value.
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("path", &self.path)
            // Header NAMES are worth seeing — which credential spelling was
            // sent is exactly what a 401 investigation needs — and no value is.
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| (name.as_str(), REDACTED))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// A response, decoded far enough to route on.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body.
    pub body: String,
    /// `x-next-page`, when GitLab said there is one.
    pub next_page: Option<String>,
    /// `ratelimit-remaining`.
    pub ratelimit_remaining: Option<u64>,
    /// `ratelimit-reset`, a Unix timestamp.
    pub ratelimit_reset: Option<u64>,
    /// `retry-after`, in seconds.
    pub retry_after: Option<u64>,
    /// `etag`, exactly as received.
    ///
    /// ⚠️ Kept byte for byte, weak `W/` prefix included, because the only thing
    /// it is ever used for is echoing back in `If-None-Match`, and a validator
    /// the server does not recognise is simply a cache miss it will not report.
    /// Nothing reads it yet.
    pub etag: Option<String>,
    /// `link`, exactly as received: the whole header, all relations, unparsed.
    ///
    /// GitHub pages with `Link: <...>; rel="next"` rather than GitLab's
    /// `x-next-page`, and rewrites `/repos/{owner}/{repo}/` to
    /// `/repositories/{id}/` in those URLs, so the next page has to be FOLLOWED
    /// rather than reconstructed. Read by
    /// [`super::github::GitHubClient`]; GitLab still pages on `next_page`.
    pub link: Option<String>,
    /// `x-oauth-scopes`, exactly as received: a comma-separated list, or an
    /// empty string for a credential that has none.
    ///
    /// ⛔ The presence of this header is the ONLY thing that distinguishes a
    /// GitHub classic token from a fine-grained one at runtime, and it is why a
    /// header field exists for it at all. GitHub has no token-introspection
    /// endpoint: a classic token gets its granted scopes echoed on every
    /// response, and a fine-grained token or an App installation token gets no
    /// header whatever. ⚠️ Absent and empty are therefore DIFFERENT answers,
    /// "there is nothing here to tell you" against "this token was granted no
    /// scopes", so this is `Option<String>` and never defaulted to `""`.
    pub oauth_scopes: Option<String>,
}

/// Executes HTTP requests. The one seam between the verdict engine and the
/// network.
#[async_trait::async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// Execute one request.
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError>;
}

/// One completed request, as shown in the debug pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLog {
    /// HTTP method.
    pub method: String,
    /// Path and query, relative to the API root. Never contains a credential.
    pub path: String,
    /// HTTP status, or `None` when the request never got one (a transport
    /// failure), which is itself worth seeing in the pane.
    pub status: Option<u16>,
    /// Wall time in milliseconds.
    pub ms: u64,
    /// `ratelimit-remaining` as reported by GitLab.
    pub ratelimit_remaining: Option<u64>,
    /// `ratelimit-reset` as reported by GitLab.
    pub ratelimit_reset: Option<u64>,
    /// `retry-after` as reported by GitLab.
    pub retry_after: Option<u64>,
    /// The error, when there was one.
    pub error: Option<String>,
    /// When the request was issued.
    pub at: chrono::DateTime<chrono::Utc>,
}

/// A bounded ring of recent requests, shared between the poller and the GUI.
#[derive(Debug, Clone)]
pub struct RequestRing {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    capacity: usize,
    entries: VecDeque<RequestLog>,
}

impl RequestRing {
    /// A ring holding at most `capacity` entries. A capacity of zero disables
    /// recording entirely.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                capacity,
                entries: VecDeque::new(),
            })),
        }
    }

    /// Append an entry, evicting the oldest when full.
    pub fn record(&self, entry: RequestLog) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.capacity == 0 {
            return;
        }
        while inner.entries.len() >= inner.capacity {
            inner.entries.pop_front();
        }
        inner.entries.push_back(entry);
    }

    /// A snapshot of the ring, oldest first.
    pub fn entries(&self) -> Vec<RequestLog> {
        self.inner
            .lock()
            .map(|i| i.entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many entries are currently held.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|i| i.entries.len()).unwrap_or(0)
    }

    /// True when nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Discard everything.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.clear();
        }
    }

    /// Change the ceiling, trimming immediately if it shrank.
    pub fn set_capacity(&self, capacity: usize) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.capacity = capacity;
            while inner.entries.len() > capacity {
                inner.entries.pop_front();
            }
        }
    }
}

impl Default for RequestRing {
    fn default() -> Self {
        Self::new(50)
    }
}

/// The production transport: `reqwest` over rustls, with no OpenSSL anywhere in
/// the tree.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Build a transport with a per-request timeout.
    ///
    /// ⛔ **Redirects are not followed, and a 3xx is an error.** reqwest's
    /// default policy follows up to ten hops and strips only `Authorization`,
    /// `Cookie`, `cookie2`, `Proxy-Authorization` and `WWW-Authenticate` on a
    /// cross-host hop — so the GitLab spelling of the credential,
    /// `PRIVATE-TOKEN`, travels to whatever host a redirect names, and only the
    /// `Authorization: Bearer` accounts were ever protected. A misconfigured
    /// `base_url`, a captive portal or a hostile instance is enough. Nothing
    /// bridgewatch asks for is behind a redirect, so refusing is free; the
    /// error names the status, and [`super::ClientError::Redirect`] says what to
    /// do about it.
    pub fn new(timeout: std::time::Duration) -> Result<Self, ClientError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("bridgewatch/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait::async_trait]
impl Transport for ReqwestTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        let mut builder = self.client.get(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let response = builder
            .send()
            .await
            .map_err(|e| ClientError::Transport(describe(&e)))?;

        let status = response.status().as_u16();
        let header = |name: &str| -> Option<String> {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let next_page = header("x-next-page").filter(|s| !s.is_empty());
        let ratelimit_remaining = header("ratelimit-remaining").and_then(|v| v.parse().ok());
        let ratelimit_reset = header("ratelimit-reset").and_then(|v| v.parse().ok());
        let retry_after = header("retry-after").and_then(|v| v.parse().ok());
        // Taken verbatim: see the fields' docs. An empty header is not a value.
        let etag = header("etag").filter(|s| !s.is_empty());
        let link = header("link").filter(|s| !s.is_empty());
        // ⛔ NOT filtered on emptiness, unlike the two above: an empty
        // `x-oauth-scopes` is a classic token with no scopes, which is a
        // different answer from a fine-grained token that sends no header at
        // all. See the field's documentation.
        let oauth_scopes = header("x-oauth-scopes");

        let body = response
            .text()
            .await
            .map_err(|e| ClientError::Transport(describe(&e)))?;

        Ok(HttpResponse {
            status,
            body,
            next_page,
            ratelimit_remaining,
            ratelimit_reset,
            retry_after,
            etag,
            link,
            oauth_scopes,
        })
    }
}

/// Render a reqwest error usefully.
///
/// ⛔ `reqwest::Error`'s own `Display` is frequently just `builder error` or
/// `error sending request`, with everything that identifies the fault in its
/// source chain. A token with a stray newline in it produces exactly
/// `builder error` on every request, forever, which is unactionable; the cause
/// says `failed to parse header value`. Walk the chain.
fn describe(error: &reqwest::Error) -> String {
    let mut parts = vec![strip_url(&error.to_string())];
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(cause) = source {
        let text = strip_url(&cause.to_string());
        if !parts.contains(&text) {
            parts.push(text);
        }
        source = cause.source();
    }
    parts.join(": ")
}

/// reqwest puts the full URL in some error strings. It never contains a token
/// here, but the request log is a place people paste from, so keep it short.
fn strip_url(message: &str) -> String {
    match message.find(" for url (") {
        Some(i) => message[..i].to_string(),
        None => message.to_string(),
    }
}
