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
/// Used by [`HttpRequest`]'s and `Conn`'s `Debug`, the two types that hold the
/// rendered credential header (both clients hold theirs in a `Conn`). It is the same text
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
    /// HTTP method: `GET` for every read, `POST` for the OAuth device-flow
    /// and token endpoints ([`crate::oauth`]) and for the job actions (retry,
    /// play), which are the only writes bridgewatch makes.
    pub method: &'static str,
    /// Fully-qualified URL.
    pub url: String,
    /// Path and query, relative to the API root. This is what the request log
    /// shows, because it is the part a human can act on.
    pub path: String,
    /// Headers to send, credentials included. Never recorded: [`RequestLog`]
    /// has no field for them, and `Debug` prints [`REDACTED`] for every value.
    pub headers: Vec<(String, String)>,
    /// A form-encoded body, sent with a `POST`. `None` for every `GET`.
    ///
    /// ⛔ The OAuth token endpoints carry the device code and the refresh token
    /// HERE, which is why `Debug` never prints it and why no log line or
    /// [`RequestLog`] field reads it.
    pub body: Option<String>,
    /// Send this request exactly as built: no layer may add a credential or a
    /// validator to it, and nothing may cache what it returns.
    ///
    /// ⛔ Set for one kind of request only: the second leg of a job log, the
    /// short-lived signed URL a provider's log endpoint redirects to (see
    /// [`super::log`]). The signature in that URL IS the authorisation, the
    /// host is a storage service rather than the account's, and
    /// [`crate::oauth::OAuthTransport`] would otherwise put the account's
    /// bearer token on it like on every other request. The client builds it
    /// with no credential header; this flag is what stops a layer below from
    /// adding one.
    pub anonymous: bool,
    /// Keep only the last this-many bytes of the body, however long it is.
    ///
    /// ⚠️ A job log can run to tens of megabytes and only its end is wanted,
    /// so the transport never buffers more than about twice this: it reads
    /// the body in chunks and drops the front as it goes, and says in
    /// [`HttpResponse::truncated`] whether anything was dropped. `None` for
    /// every JSON request, whose body is read whole as it always was.
    pub tail_bytes: Option<usize>,
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
            .field("body", &self.body.as_ref().map(|_| REDACTED))
            .field("anonymous", &self.anonymous)
            .field("tail_bytes", &self.tail_bytes)
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
    /// `ratelimit-remaining` (GitLab), or `x-ratelimit-remaining` (GitHub) when
    /// the unprefixed header is absent.
    pub ratelimit_remaining: Option<u64>,
    /// `ratelimit-reset` (GitLab), or `x-ratelimit-reset` (GitHub) when the
    /// unprefixed header is absent. A Unix timestamp either way.
    pub ratelimit_reset: Option<u64>,
    /// `retry-after`, in seconds.
    pub retry_after: Option<u64>,
    /// `etag`, exactly as received.
    ///
    /// ⚠️ Kept byte for byte, weak `W/` prefix included, because the only thing
    /// it is ever used for is echoing back in `If-None-Match`, and a validator
    /// the server does not recognise is simply a cache miss it will not report.
    /// Read by [`super::conditional::ConditionalTransport`], which is the one
    /// thing that sends it back.
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
    /// `location`, exactly as received. Read only by a job log's one explicit
    /// redirect follow ([`super::log::follow`]); every other 3xx is refused as
    /// it always was.
    pub location: Option<String>,
    /// True when the body is not the whole resource: a `tail_bytes` request
    /// whose response was longer than the tail and had its front dropped, or
    /// a `206` whose `content-range` starts after byte 0. Always false for a
    /// request without `tail_bytes`.
    pub truncated: bool,
}

impl HttpResponse {
    /// A response with a status and a body and no headers at all, for test
    /// doubles and scripts.
    pub fn plain(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            next_page: None,
            ratelimit_remaining: None,
            ratelimit_reset: None,
            retry_after: None,
            etag: None,
            link: None,
            oauth_scopes: None,
            location: None,
            truncated: false,
        }
    }
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
    ///
    /// Inside [`in_order`] the entry is held back instead, and appended when
    /// that scope's turn comes: see [`in_order`] for why.
    pub fn record(&self, entry: RequestLog) {
        let mut entry = Some(entry);
        let held = HELD.try_with(|held| {
            if let Some(entry) = entry.take() {
                held.borrow_mut().push((self.clone(), entry));
            }
        });
        if held.is_ok() {
            return;
        }
        let Some(entry) = entry else { return };
        self.append(entry);
    }

    fn append(&self, entry: RequestLog) {
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

tokio::task_local! {
    /// What [`RequestRing::record`] was handed inside an [`in_order`] scope,
    /// with the ring it was meant for.
    static HELD: std::cell::RefCell<Held>;
}

/// Run `futures` concurrently, returning their outputs in the order given and
/// recording their requests in that order too.
///
/// ⛔ The order is the point, and not only of the outputs. The poller used to
/// await every request in turn, so the debug pane's ring read in the order the
/// tick was written: a watch's list, then each pipeline's jobs and bridges,
/// then its children. Run concurrently, the ring would record in COMPLETION
/// order, which is whichever response the network happened to deliver first,
/// and the same tick would read differently every time. So each future's
/// entries are held back while it runs and appended, whole and in turn, once
/// they have all finished. Nested calls compose: an inner scope hands its
/// entries to the enclosing one rather than to the ring, so a tick reads
/// exactly as the sequential poller wrote it however deep the concurrency
/// goes.
///
/// ⚠ This bounds nothing. How many requests are in flight at once is each
/// client's [`InFlight`], which is per account and so also covers two watches
/// on one account running side by side.
pub async fn in_order<F>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output>
where
    F: std::future::Future,
{
    futures_util::future::join_all(futures.into_iter().map(held_back))
        .await
        .into_iter()
        .map(|(output, held)| {
            release(held);
            output
        })
        .collect()
}

/// [`in_order`] for exactly two futures of different types.
pub async fn in_order2<A, B>(a: A, b: B) -> (A::Output, B::Output)
where
    A: std::future::Future,
    B: std::future::Future,
{
    let ((a, held_a), (b, held_b)) = tokio::join!(held_back(a), held_back(b));
    release(held_a);
    release(held_b);
    (a, b)
}

type Held = Vec<(RequestRing, RequestLog)>;

/// `future`, with every entry it records held back and handed over with its
/// output.
async fn held_back<F: std::future::Future>(future: F) -> (F::Output, Held) {
    HELD.scope(std::cell::RefCell::new(Vec::new()), async move {
        let output = future.await;
        (output, HELD.with(|held| held.take()))
    })
    .await
}

/// Record what a [`held_back`] future held, which goes to the enclosing scope
/// when there is one and to the ring when there is not.
fn release(held: Held) {
    for (ring, entry) in held {
        ring.record(entry);
    }
}

/// How many requests one client has in flight at once.
///
/// ⚠ Four, because the poller now fetches a tick's pipelines, their children
/// and its watches side by side, and gitlab.com's own guidance for API clients
/// is a handful of concurrent connections, not dozens. A deep bridge fan-out is
/// 20 to 30 requests a tick; at four at a time that is a few seconds of wall
/// time instead of one request after another for ten.
pub const MAX_IN_FLIGHT: usize = 4;

/// The bound on one client's concurrent requests. Clones share it.
///
/// ⛔ One per CLIENT, and the poller builds one client per account, so this is
/// the per-account limit whichever watch or pipeline is asking. The permit is
/// taken before a request's clock starts, so a request's `ms` in the log and
/// the ring is the request, not the queue in front of it.
#[derive(Debug, Clone)]
pub struct InFlight(Arc<tokio::sync::Semaphore>);

impl InFlight {
    /// A bound of [`MAX_IN_FLIGHT`].
    pub fn new() -> Self {
        Self(Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT)))
    }

    /// Wait for a slot. The slot is released when the permit is dropped.
    ///
    /// `None` only if the semaphore were closed, which nothing here ever does;
    /// the request then goes ahead unbounded rather than failing.
    pub async fn acquire(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        self.0.acquire().await.ok()
    }
}

impl Default for InFlight {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for RequestRing {
    fn default() -> Self {
        Self::new(50)
    }
}

/// What a client needs to talk to one account: where its API is, the rendered
/// credential header, and the transport, ring and in-flight bound every request
/// goes through. Both clients hold one; cloning is cheap, and clones share the
/// transport, the ring and the bound.
///
/// ⛔ `Debug` is hand-written, here and only here. The derived one printed
/// `header_value`, which is the token itself for a `PRIVATE-TOKEN` account: the
/// whole point of [`crate::token::Secret`] is that a credential cannot reach a
/// log line by accident, and a struct holding the already-rendered header value
/// undid that for anyone who wrote `{:?}` on a client. The clients derive their
/// own `Debug` over this one, so the rule is kept in one place.
#[derive(Clone)]
pub(crate) struct Conn {
    base_url: String,
    api_path: String,
    header_name: &'static str,
    header_value: String,
    transport: Arc<dyn Transport>,
    ring: RequestRing,
    /// At most [`MAX_IN_FLIGHT`] of this account's requests at a time.
    in_flight: InFlight,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("base_url", &self.base_url)
            .field("api_path", &self.api_path)
            .field("header_name", &self.header_name)
            .field("header_value", &REDACTED)
            .field("transport", &self.transport)
            .finish()
    }
}

impl Conn {
    /// The connection for an account and its already-resolved token.
    pub(crate) fn new(
        account: &crate::config::Account,
        token: &crate::token::Secret,
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
            in_flight: InFlight::new(),
        }
    }

    /// The account's `base_url`, without a trailing slash.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The account's `api_path`, which every request path is appended to.
    pub(crate) fn api_path(&self) -> &str {
        &self.api_path
    }

    /// The request ring every request is recorded in.
    pub(crate) fn ring(&self) -> &RequestRing {
        &self.ring
    }

    /// A request to `path` under this account's API, carrying the credential
    /// header and nothing else. A provider adds its own headers to it.
    pub(crate) fn request(&self, method: &'static str, path: &str) -> HttpRequest {
        HttpRequest {
            method,
            url: format!("{}{}{}", self.base_url, self.api_path, path),
            path: path.to_string(),
            headers: vec![(self.header_name.to_string(), self.header_value.clone())],
            body: None,
            anonymous: false,
            tail_bytes: None,
        }
    }

    /// Send one request the way every request is sent: through the account's
    /// in-flight bound, timed from when it got a slot, logged at `debug`, and
    /// recorded in the ring. `classify` decides which answers are errors.
    ///
    /// ⛔ `path` is logged whole, query and all, and that is safe by
    /// CONSTRUCTION rather than by filtering: the credential travels only in a
    /// header (see the module docs), every API path is built from a fixed
    /// template with its variables percent-encoded, and a log's signed redirect
    /// is recorded without its query ([`super::log::display_path`]).
    pub(crate) async fn send(
        &self,
        request: HttpRequest,
        classify: impl Fn(&HttpResponse) -> Option<ClientError>,
    ) -> Result<HttpResponse, ClientError> {
        let method = request.method;
        let path = request.path.clone();
        // Before the clock starts: `ms` is the request, not the queue.
        let _slot = self.in_flight.acquire().await;
        let started = std::time::Instant::now();
        let result = self.transport.execute(request).await;
        let ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(response) => {
                let error = classify(&response);
                // As a str, so the value is quoted: a path is free text and
                // its end must be visible in a line of `key=value` pairs.
                tracing::debug!(
                    method,
                    path = path.as_str(),
                    status = response.status,
                    bytes = response.body.len(),
                    truncated = response.truncated,
                    ms,
                    "request"
                );
                self.ring.record(RequestLog {
                    method: method.into(),
                    path: path.clone(),
                    status: Some(response.status),
                    ms,
                    ratelimit_remaining: response.ratelimit_remaining,
                    ratelimit_reset: response.ratelimit_reset,
                    retry_after: response.retry_after,
                    error: error.as_ref().map(ToString::to_string),
                    at: chrono::Utc::now(),
                });
                match error {
                    Some(e) => Err(e),
                    None => Ok(response),
                }
            }
            Err(e) => {
                tracing::debug!(method, path = path.as_str(), error = %e, ms, "request failed");
                self.ring.record(RequestLog {
                    method: method.into(),
                    path,
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

    /// [`Self::send`] a JSON read and decode its body, handing the response
    /// back too for the headers a caller pages or reads scopes from.
    pub(crate) async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        request: HttpRequest,
        classify: impl Fn(&HttpResponse) -> Option<ClientError>,
    ) -> Result<(T, HttpResponse), ClientError> {
        let path = request.path.clone();
        let response = self.send(request, classify).await?;
        let value = serde_json::from_str::<T>(&response.body).map_err(|e| ClientError::Decode {
            path,
            message: e.to_string(),
        })?;
        Ok((value, response))
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
        // The OAuth endpoints post a form and the job actions post nothing;
        // everything else is a GET exactly as before.
        let mut builder = match request.method {
            "POST" => self
                .client
                .post(&request.url)
                .body(request.body.clone().unwrap_or_default()),
            _ => self.client.get(&request.url),
        };
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
        // ⛔ GitHub spells these `x-ratelimit-*` and GitLab spells them without
        // the prefix. Reading only GitLab's spelling left both fields `None` on
        // every GitHub response, which silently disarmed the classifier that
        // tells an exhausted primary limit (a 403 with remaining 0) from a bad
        // token, and left the debug pane's `rl` column blank. The unprefixed
        // name is read FIRST so a GitLab response reads exactly as it always
        // has; GitLab documents no `x-ratelimit-*` header, so the fallback is
        // only ever reached on GitHub.
        let number = |names: [&str; 2]| -> Option<u64> {
            names
                .iter()
                .find_map(|name| header(name))
                .and_then(|v| v.parse().ok())
        };
        let ratelimit_remaining = number(["ratelimit-remaining", "x-ratelimit-remaining"]);
        let ratelimit_reset = number(["ratelimit-reset", "x-ratelimit-reset"]);
        let retry_after = header("retry-after").and_then(|v| v.parse().ok());
        // Taken verbatim: see the fields' docs. An empty header is not a value.
        let etag = header("etag").filter(|s| !s.is_empty());
        let link = header("link").filter(|s| !s.is_empty());
        // ⛔ NOT filtered on emptiness, unlike the two above: an empty
        // `x-oauth-scopes` is a classic token with no scopes, which is a
        // different answer from a fine-grained token that sends no header at
        // all. See the field's documentation.
        let oauth_scopes = header("x-oauth-scopes");
        let location = header("location").filter(|s| !s.is_empty());

        let (body, truncated) = match request.tail_bytes {
            None => (
                response
                    .text()
                    .await
                    .map_err(|e| ClientError::Transport(describe(&e)))?,
                false,
            ),
            Some(keep) => {
                let starts_late = status == 206
                    && header("content-range")
                        .as_deref()
                        .and_then(range_start)
                        .is_some_and(|start| start > 0);
                let mut tail = Tail::new(keep);
                let mut response = response;
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|e| ClientError::Transport(describe(&e)))?
                {
                    tail.push(&chunk);
                }
                let (bytes, dropped) = tail.finish();
                (
                    String::from_utf8_lossy(&bytes).into_owned(),
                    dropped || starts_late,
                )
            }
        };

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
            location,
            truncated,
        })
    }
}

/// The last `keep` bytes of a stream, held in at most about twice that.
///
/// Public so the bound can be tested without a network: the transport is the
/// only production caller.
#[derive(Debug)]
pub struct Tail {
    keep: usize,
    bytes: Vec<u8>,
    dropped: bool,
}

impl Tail {
    /// An empty tail that will keep the last `keep` bytes.
    pub fn new(keep: usize) -> Self {
        Self {
            keep: keep.max(1),
            bytes: Vec::new(),
            dropped: false,
        }
    }

    /// Append a chunk, dropping the front once more than twice `keep` is
    /// held. Twice rather than exactly `keep` so a stream of small chunks does
    /// not move the whole buffer on every one of them.
    pub fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > self.keep.saturating_mul(2) {
            let cut = self.bytes.len() - self.keep;
            self.bytes.drain(..cut);
            self.dropped = true;
        }
    }

    /// How many bytes are held right now. Never more than twice `keep` plus
    /// one chunk.
    pub fn held(&self) -> usize {
        self.bytes.len()
    }

    /// The last `keep` bytes, and whether anything before them was dropped.
    pub fn finish(mut self) -> (Vec<u8>, bool) {
        if self.bytes.len() > self.keep {
            let cut = self.bytes.len() - self.keep;
            self.bytes.drain(..cut);
            self.dropped = true;
        }
        (self.bytes, self.dropped)
    }
}

/// The first byte position of a `content-range: bytes <start>-<end>/<size>`.
fn range_start(header: &str) -> Option<u64> {
    header
        .trim()
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
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
