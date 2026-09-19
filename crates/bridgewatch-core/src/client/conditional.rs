//! Conditional requests: remember each response's `ETag`, send it back as
//! `If-None-Match`, and answer a `304 Not Modified` with the body it confirms.
//!
//! # Why this exists
//!
//! GitHub does not charge an authenticated `304` against the primary rate limit
//! (measured twice: `x-ratelimit-used` stayed flat across consecutive 304s and
//! moved by one per 200). A settled watch asks the same questions every tick and
//! gets the same answers, so with validators it polls at no cost and only a real
//! change spends budget. Without them, three busy watches exhaust an hour's
//! 5,000 requests.
//!
//! # Where it sits, and why there
//!
//! It is a [`Transport`] that wraps another one, built by
//! [`super::client_for`] for each GitHub client. Three properties follow from
//! that placement rather than from code that has to keep them true:
//!
//! * **One cache per client, so one per account.** Two accounts can never share
//!   a validator or a body, even for the same URL, because they never share an
//!   instance. A new token means a new client (the credential is rendered into
//!   the client at construction), so a rotated token starts cold too.
//! * **Nothing above the transport can tell a replayed body from a fresh one**,
//!   which is the point: every request still reaches the server, and a 304 is
//!   the SERVER saying the stored bytes are current. There is therefore no
//!   second validity rule to keep in step with
//!   [`crate::poll::cache::PipelineCache`]'s `(status, updated_at)`: that cache
//!   sees exactly the bytes a 200 would have carried, and decides as it always
//!   has.
//! * **Each page is its own entry.** A paginated listing is a sequence of
//!   requests for different URLs, so a 304 on page 1 replays page 1's stored
//!   `Link` header and the client goes on to ask for page 2 conditionally in
//!   its own right. A 304 does not have to repeat `Link` (RFC 9110 lists the
//!   headers it must repeat, and `Link` is not one), so the stored one is what
//!   keeps pages 2..n reachable.
//!
//! The status a 304 is served with stays **304**, so the request log, and the
//! debug pane that draws it, shows the saving rather than disguising it as a
//! 200. The GitHub client's classifier reads a 304 as success for that reason.
//!
//! # What it deliberately does not do
//!
//! * It persists nothing. The entries live exactly as long as the client, in
//!   memory, and are dropped with it.
//! * It never answers without asking. There is no freshness lifetime here and
//!   `Cache-Control: max-age` is ignored: a monitor that served a minute-old
//!   answer without asking would be a monitor that is a minute late.
//! * GitLab does not get it in this build, although gitlab.com also answers
//!   `If-None-Match` with a 304. See [`super::client_for`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::ClientError;
use super::http::{HttpRequest, HttpResponse, Transport};

/// How many responses one client keeps by default.
///
/// A watch touches a handful of URLs per tick (its list, and the jobs pages of
/// whichever runs are live), so this holds the working set of dozens of watches
/// on one account. Least recently used goes first, and every tick touches the
/// entries a settled watch depends on, so they are the last to go.
pub const DEFAULT_MAX_ENTRIES: usize = 256;

/// How many bytes of stored bodies one client keeps by default.
///
/// A page of 100 jobs with their steps is the largest body bridgewatch asks
/// for and runs to a few hundred kilobytes, so this holds dozens of them while
/// staying small for a tray process. A single body larger than the whole
/// budget is not stored at all.
pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The request header a stored validator is sent back in.
const IF_NONE_MATCH: &str = "If-None-Match";

/// A [`Transport`] that makes every `GET` conditional once it has a validator
/// for that exact URL.
///
/// ⛔ `Debug` is hand-written so that it prints sizes and never a stored body:
/// a body is a repository's CI history, and a `{:?}` of a client is exactly the
/// kind of thing that ends up pasted into an issue.
pub struct ConditionalTransport {
    inner: Arc<dyn Transport>,
    store: Mutex<Store>,
}

impl std::fmt::Debug for ConditionalTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (entries, bytes) = self
            .store
            .lock()
            .map(|s| (s.entries.len(), s.bytes))
            .unwrap_or((0, 0));
        f.debug_struct("ConditionalTransport")
            .field("inner", &self.inner)
            .field("entries", &entries)
            .field("bytes", &bytes)
            .finish()
    }
}

/// What a 200 said, kept so a later 304 can say it again.
///
/// Held behind an `Arc` so a request can take a snapshot of it when it SENDS
/// the validator. The 304 is then answered from that snapshot, whatever
/// happened to the store while the request was in flight: another request on
/// the same client may have evicted it, and a 304 with nothing to serve is the
/// one outcome this type is built never to produce.
#[derive(Debug)]
struct Stored {
    etag: String,
    body: String,
    /// The pagination header that came with this body. See the module docs.
    link: Option<String>,
    /// GitLab's pagination header, kept for the same reason as `link`.
    next_page: Option<String>,
    /// Kept because a 304 need not repeat it, and `token_self` reads it.
    oauth_scopes: Option<String>,
}

#[derive(Debug)]
struct Slot {
    stored: Arc<Stored>,
    /// The value of [`Store::clock`] when this entry was last stored or served.
    used: u64,
}

#[derive(Debug)]
struct Store {
    entries: HashMap<String, Slot>,
    /// The sum of every stored body's length.
    bytes: usize,
    /// A counter, not a time: all eviction needs is an order.
    clock: u64,
    max_entries: usize,
    max_bytes: usize,
}

impl Store {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn lookup(&mut self, key: &str) -> Option<Arc<Stored>> {
        let now = self.tick();
        let slot = self.entries.get_mut(key)?;
        slot.used = now;
        Some(slot.stored.clone())
    }

    fn remove(&mut self, key: &str) {
        if let Some(slot) = self.entries.remove(key) {
            self.bytes -= slot.stored.body.len();
        }
    }

    fn put(&mut self, key: String, stored: Arc<Stored>) {
        self.remove(&key);
        let size = stored.body.len();
        // A body bigger than the whole budget would evict everything and then
        // itself; not storing it costs one full response per poll, which is
        // what happened before this cache existed.
        if self.max_entries == 0 || size > self.max_bytes {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.max_entries || self.bytes + size > self.max_bytes)
        {
            self.evict_one();
        }
        let used = self.tick();
        self.bytes += size;
        self.entries.insert(key, Slot { stored, used });
    }

    /// Drop the least recently used entry. A linear scan: the store holds a few
    /// hundred entries at most and this runs only when one is added past the
    /// bound.
    fn evict_one(&mut self) {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, slot)| slot.used)
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            self.remove(&key);
        }
    }
}

impl ConditionalTransport {
    /// Wrap `inner` with the default bounds.
    pub fn new(inner: Arc<dyn Transport>) -> Self {
        Self::with_bounds(inner, DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }

    /// Wrap `inner`, keeping at most `max_entries` responses and `max_bytes` of
    /// their bodies. Either bound at zero stores nothing, which makes every
    /// request unconditional.
    pub fn with_bounds(inner: Arc<dyn Transport>, max_entries: usize, max_bytes: usize) -> Self {
        Self {
            inner,
            store: Mutex::new(Store {
                entries: HashMap::new(),
                bytes: 0,
                clock: 0,
                max_entries,
                max_bytes,
            }),
        }
    }

    /// How many responses are stored.
    pub fn len(&self) -> usize {
        self.store.lock().map(|s| s.entries.len()).unwrap_or(0)
    }

    /// True when nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The bytes of stored bodies.
    pub fn bytes(&self) -> usize {
        self.store.lock().map(|s| s.bytes).unwrap_or(0)
    }

    fn with_store<T>(&self, f: impl FnOnce(&mut Store) -> T) -> Option<T> {
        // A poisoned lock means a panic elsewhere mid-update; the store is an
        // optimisation, so the request goes out unconditionally rather than the
        // panic spreading.
        self.store.lock().ok().map(|mut s| f(&mut s))
    }
}

/// The key a response is stored under.
///
/// Method and the FULL URL, query included: `?page=2` is a different resource
/// from `?page=1`, and so is `?event=push` from `?event=schedule`. Nothing names
/// the account because the store already belongs to exactly one. The request
/// headers that could select a different representation (`Accept`, the API
/// version, the credential) are constants of the client that owns this store.
fn key_of(request: &HttpRequest) -> String {
    format!("{} {}", request.method, request.url)
}

#[async_trait::async_trait]
impl Transport for ConditionalTransport {
    async fn execute(&self, mut request: HttpRequest) -> Result<HttpResponse, ClientError> {
        // Only a GET is safe to replay, and it is the only method bridgewatch
        // issues; anything else passes through untouched.
        if request.method != "GET" {
            return self.inner.execute(request).await;
        }
        let key = key_of(&request);
        let sent = self.with_store(|s| s.lookup(&key)).flatten();
        if let Some(stored) = &sent {
            // ⛔ Byte for byte, `W/` prefix and quotes included. A validator the
            // server does not recognise is a cache miss it will not report,
            // which would cost a full request every poll and look like nothing.
            request
                .headers
                .push((IF_NONE_MATCH.to_string(), stored.etag.clone()));
        }
        let path = request.path.clone();

        let response = self.inner.execute(request).await?;

        match response.status {
            304 => {
                let Some(stored) = sent else {
                    // Only reachable when a server (or a proxy in front of one)
                    // answers 304 to a request that carried no validator of
                    // ours. There is no body to serve, and retrying without a
                    // validator is the request that just produced this, so it
                    // is reported as what it is. Not fatal and not a backoff:
                    // the next tick asks again.
                    return Err(ClientError::Unexpected { status: 304, path });
                };
                // The 304's own validator is the current one if it sent one.
                let etag = response.etag.clone().unwrap_or_else(|| stored.etag.clone());
                let served = HttpResponse {
                    // Kept as 304 so the request log shows the saving.
                    status: 304,
                    body: stored.body.clone(),
                    next_page: stored.next_page.clone(),
                    // The rate-limit headers are the 304's own: they describe
                    // the budget NOW, which is what the backoff and the debug
                    // pane need, and the whole point is that they did not move.
                    ratelimit_remaining: response.ratelimit_remaining,
                    ratelimit_reset: response.ratelimit_reset,
                    retry_after: response.retry_after,
                    etag: Some(etag.clone()),
                    link: stored.link.clone(),
                    oauth_scopes: response
                        .oauth_scopes
                        .clone()
                        .or_else(|| stored.oauth_scopes.clone()),
                };
                // Re-stored rather than merely touched: the entry may have been
                // evicted while this request was in flight, and the server has
                // just confirmed it is current. The same allocation is reused
                // unless the 304 changed something worth keeping.
                let refreshed = if etag == stored.etag && served.oauth_scopes == stored.oauth_scopes
                {
                    stored
                } else {
                    Arc::new(Stored {
                        etag,
                        body: stored.body.clone(),
                        link: stored.link.clone(),
                        next_page: stored.next_page.clone(),
                        oauth_scopes: served.oauth_scopes.clone(),
                    })
                };
                self.with_store(|s| s.put(key, refreshed));
                Ok(served)
            }
            200 => {
                match response.etag.as_deref().filter(|e| !e.is_empty()) {
                    Some(etag) => {
                        let stored = Arc::new(Stored {
                            etag: etag.to_string(),
                            body: response.body.clone(),
                            link: response.link.clone(),
                            next_page: response.next_page.clone(),
                            oauth_scopes: response.oauth_scopes.clone(),
                        });
                        self.with_store(|s| s.put(key, stored));
                    }
                    // A 200 with no validator makes any stored one stale.
                    None => {
                        self.with_store(|s| s.remove(&key));
                    }
                }
                Ok(response)
            }
            // An error says nothing about the stored representation: a rate
            // limit or a 5xx now does not make the last body wrong, and the
            // next 200 replaces it anyway.
            _ => Ok(response),
        }
    }
}
