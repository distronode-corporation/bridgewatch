//! The device authorization grant, and refreshing what it produced.
//!
//! Three requests, all a form `POST` with `Accept: application/json`, all
//! through the ordinary [`Transport`] so the tests script them:
//!
//! 1. [`start`]: ask for a device code and a user code.
//! 2. [`wait_for_token`]: poll the token endpoint at the interval the server
//!    asked for until the user approves, refuses, or the code expires.
//! 3. [`refresh`]: trade a refresh token for a new set.
//!
//! ⚠️ The two providers answer a pending poll differently and the code reads
//! the BODY rather than the status for that reason. GitHub answers `200` with
//! `{"error": "authorization_pending"}`; GitLab answers `400` with the same
//! body, as RFC 8628 section 3.5 says to.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::store::TokenSet;
use super::{Endpoints, OAuthError, sanitize_code, store};
use crate::client::{ClientError, HttpRequest, HttpResponse, RequestRing, Transport, client_for};
use crate::config::Account;
use crate::token::{Secret, TokenProvider};

/// The grant type both providers take for a device-code poll (RFC 8628 3.4).
pub const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The interval RFC 8628 3.2 says to use when the server names none.
pub const DEFAULT_INTERVAL_SECS: u64 = 5;

/// What `slow_down` adds to the interval (RFC 8628 3.5; GitHub's docs say the
/// same five seconds).
pub const SLOW_DOWN_SECS: u64 = 5;

/// A started sign-in: what to show the user, and the device code to poll with.
///
/// ⛔ `Debug` prints neither code. The device code is a bearer credential for
/// the next fifteen minutes; the user code is shown to the user on purpose but
/// is still not something a log line needs.
#[derive(Clone)]
pub struct DeviceAuthorization {
    device_code: Secret,
    /// What the user types at the verification page.
    pub user_code: String,
    /// The page to type it at, e.g. `https://github.com/login/device`.
    pub verification_uri: String,
    /// The same page with the code already filled in, where the provider
    /// offers one (GitLab does). Not opened by bridgewatch: the user compares
    /// the code they see with the one bridgewatch shows, which is the point.
    pub verification_uri_complete: Option<String>,
    /// How long the codes are good for, seconds.
    pub expires_in: u64,
    /// How long to wait between polls, seconds.
    pub interval: u64,
}

impl std::fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("device_code", &self.device_code)
            .field("user_code", &"<redacted>")
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// Where a wait has got to, for a UI that wants to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Progress {
    /// Asked, and the user has not answered yet.
    Waiting {
        /// How many polls so far.
        polls: u32,
    },
    /// The server asked bridgewatch to poll less often.
    SlowedDown {
        /// The new interval, seconds.
        interval: u64,
    },
}

/// A completed sign-in: who, and until when. No token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedIn {
    /// Who signed in, when `/user` answered.
    pub login: Option<String>,
    /// The granted scopes.
    pub scopes: Vec<String>,
    /// When the access token expires, Unix seconds.
    pub expires_at: Option<u64>,
}

/// The token endpoint's answer, success and failure in one shape.
#[derive(Deserialize)]
struct TokenReply {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    refresh_token_expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct DeviceReply {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
}

/// Form-encode `pairs`.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// One form `POST` to an OAuth endpoint.
///
/// ⛔ Logged by PATH only, at debug: the body holds the device code or the
/// refresh token, and the response holds the tokens.
async fn post(
    transport: &dyn Transport,
    url: &str,
    body: String,
) -> Result<HttpResponse, ClientError> {
    let path = Endpoints::path_of(url).to_string();
    let request = HttpRequest {
        method: "POST",
        url: url.to_string(),
        path: path.clone(),
        headers: vec![
            ("Accept".to_string(), "application/json".to_string()),
            (
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ],
        body: Some(body),
    };
    let result = transport.execute(request).await;
    match &result {
        Ok(response) => tracing::debug!(
            method = "POST",
            path,
            status = response.status,
            "oauth request"
        ),
        Err(e) => tracing::debug!(method = "POST", path, error = %e, "oauth request failed"),
    }
    result
}

/// Decode a JSON body without ever quoting it.
fn decode<T: serde::de::DeserializeOwned>(body: &str, host: &str) -> Result<T, OAuthError> {
    serde_json::from_str(body).map_err(|e| OAuthError::Decode {
        host: host.to_string(),
        message: format!(
            "{:?} at line {} column {}",
            e.classify(),
            e.line(),
            e.column()
        ),
    })
}

/// A status that says nothing about the sign-in itself: the network, the
/// server, a rate limit.
fn transport_status(response: &HttpResponse, path: &str) -> Option<ClientError> {
    match response.status {
        429 => Some(ClientError::RateLimited {
            retry_after: response.retry_after,
            reset: response.ratelimit_reset,
        }),
        500..=599 => Some(ClientError::Server {
            status: response.status,
        }),
        300..=399 => Some(ClientError::Redirect {
            status: response.status,
            path: path.to_string(),
        }),
        _ => None,
    }
}

/// Step 1: ask for a device code.
pub async fn start(
    transport: &dyn Transport,
    endpoints: &Endpoints,
    client_id: &str,
) -> Result<DeviceAuthorization, OAuthError> {
    let mut pairs = vec![("client_id", client_id)];
    if let Some(scope) = endpoints.scope {
        pairs.push(("scope", scope));
    }
    let response = post(transport, &endpoints.device_code_url, form(&pairs))
        .await
        .map_err(OAuthError::Client)?;
    if let Some(e) = transport_status(&response, Endpoints::path_of(&endpoints.device_code_url)) {
        return Err(OAuthError::Client(e));
    }
    if !(200..300).contains(&response.status) {
        return Err(refusal(&response, endpoints));
    }
    let reply: DeviceReply = decode(&response.body, &endpoints.host)?;
    // ⛔ The verification page is opened in the user's browser, so it has to be
    // on the host the user chose. A server naming any other host is refused
    // here rather than trusted there.
    if super::origin_of(&reply.verification_uri).as_deref() != Some(endpoints.origin.as_str()) {
        return Err(OAuthError::Decode {
            host: endpoints.host.clone(),
            message: format!(
                "the verification page is not on {}, so it is not opened",
                endpoints.host
            ),
        });
    }
    let complete = reply
        .verification_uri_complete
        .filter(|u| super::origin_of(u).as_deref() == Some(endpoints.origin.as_str()));
    Ok(DeviceAuthorization {
        device_code: Secret::new(reply.device_code),
        user_code: reply.user_code,
        verification_uri: reply.verification_uri,
        verification_uri_complete: complete,
        expires_in: reply.expires_in,
        interval: reply.interval.unwrap_or(DEFAULT_INTERVAL_SECS).max(1),
    })
}

/// A non-2xx answer carrying an OAuth error code, or none.
fn refusal(response: &HttpResponse, endpoints: &Endpoints) -> OAuthError {
    let code = serde_json::from_str::<TokenReply>(&response.body)
        .ok()
        .and_then(|r| r.error)
        .map(|c| sanitize_code(&c))
        .unwrap_or_else(|| format!("HTTP {}", response.status));
    OAuthError::Refused {
        host: endpoints.host.clone(),
        code,
    }
}

/// What one poll of the token endpoint said.
enum Poll {
    Pending,
    SlowDown(Option<u64>),
    Done(TokenReply),
}

async fn poll_once(
    transport: &dyn Transport,
    endpoints: &Endpoints,
    client_id: &str,
    device: &DeviceAuthorization,
) -> Result<Poll, OAuthError> {
    let body = form(&[
        ("client_id", client_id),
        ("device_code", device.device_code.expose()),
        ("grant_type", DEVICE_GRANT),
    ]);
    let response = post(transport, &endpoints.token_url, body)
        .await
        .map_err(OAuthError::Client)?;
    if let Some(e) = transport_status(&response, Endpoints::path_of(&endpoints.token_url)) {
        return Err(OAuthError::Client(e));
    }
    let reply: TokenReply = match decode(&response.body, &endpoints.host) {
        Ok(reply) => reply,
        Err(_) if !(200..300).contains(&response.status) => {
            return Err(refusal(&response, endpoints));
        }
        Err(e) => return Err(e),
    };
    match reply.error.as_deref().map(sanitize_code).as_deref() {
        None if reply.access_token.is_some() => Ok(Poll::Done(reply)),
        None => Err(refusal(&response, endpoints)),
        Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => Ok(Poll::SlowDown(reply.interval)),
        Some("expired_token") => Err(OAuthError::Expired),
        Some("access_denied") => Err(OAuthError::Denied {
            host: endpoints.host.clone(),
        }),
        Some(code) => Err(OAuthError::Refused {
            host: endpoints.host.clone(),
            code: code.to_string(),
        }),
    }
}

/// Step 2: poll until the user answers, the code expires, or `cancel`
/// completes.
///
/// `sleep` waits between polls (the tests pass one that records the interval
/// and returns at once), and the deadline is counted in the intervals slept,
/// so it is exact under a scripted clock and within one request's latency of
/// the server's own under the real one. The server says `expired_token` itself
/// in any case; the deadline is what stops a server that never does.
///
/// ⚠️ A transport failure during the wait is not the end of it: a laptop
/// that drops Wi-Fi for one poll should not lose a sign-in the user is halfway
/// through. Three in a row is.
pub async fn wait_for_token<S, F, C>(
    transport: &dyn Transport,
    endpoints: &Endpoints,
    client_id: &str,
    device: &DeviceAuthorization,
    sleep: S,
    cancel: C,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<TokenReplyView, OAuthError>
where
    S: Fn(Duration) -> F,
    F: Future<Output = ()>,
    C: Future<Output = ()>,
{
    let mut cancel = std::pin::pin!(cancel);
    let mut interval = device.interval.max(1);
    let mut waited: u64 = 0;
    let mut polls: u32 = 0;
    let mut transient: u32 = 0;
    loop {
        if waited >= device.expires_in {
            return Err(OAuthError::Expired);
        }
        tokio::select! {
            biased;
            _ = &mut cancel => return Err(OAuthError::Cancelled),
            _ = sleep(Duration::from_secs(interval)) => {}
        }
        waited = waited.saturating_add(interval);
        polls += 1;
        let outcome = tokio::select! {
            biased;
            _ = &mut cancel => return Err(OAuthError::Cancelled),
            outcome = poll_once(transport, endpoints, client_id, device) => outcome,
        };
        match outcome {
            Ok(Poll::Done(reply)) => return Ok(TokenReplyView(reply)),
            Ok(Poll::Pending) => {
                transient = 0;
                progress(Progress::Waiting { polls });
            }
            Ok(Poll::SlowDown(asked)) => {
                transient = 0;
                interval = interval
                    .saturating_add(SLOW_DOWN_SECS)
                    .max(asked.unwrap_or(0));
                progress(Progress::SlowedDown { interval });
            }
            Err(OAuthError::Client(e)) if transient < 2 => {
                transient += 1;
                tracing::debug!(error = %e, "sign-in poll failed; trying again");
                if e.should_back_off() {
                    interval = interval.saturating_add(SLOW_DOWN_SECS);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// A successful token answer, opaque outside this module so its tokens can only
/// leave through [`TokenSet`].
pub struct TokenReplyView(TokenReply);

impl std::fmt::Debug for TokenReplyView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenReplyView(<redacted>)")
    }
}

impl TokenReplyView {
    /// Turn the answer into a token set for `account`, obtained at `now`.
    ///
    /// `previous_refresh` is kept when the answer carries no refresh token of
    /// its own: a provider that does not rotate may simply not repeat it.
    pub fn into_set(
        self,
        account: &Account,
        client_id: &str,
        now: u64,
        login: Option<String>,
        previous_refresh: Option<Secret>,
    ) -> TokenSet {
        let reply = self.0;
        TokenSet {
            access_token: Secret::new(reply.access_token.unwrap_or_default()),
            refresh_token: reply
                .refresh_token
                .filter(|r| !r.is_empty())
                .map(Secret::new)
                .or(previous_refresh),
            expires_at: reply.expires_in.map(|s| now.saturating_add(s)),
            refresh_expires_at: reply
                .refresh_token_expires_in
                .map(|s| now.saturating_add(s)),
            // Space-separated per RFC 6749; GitHub has been seen to use commas.
            scopes: reply
                .scope
                .unwrap_or_default()
                .split([' ', ','])
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            login,
            client_id: client_id.to_string(),
            base_url: account.base_url.trim().trim_end_matches('/').to_string(),
            obtained_at: now,
        }
    }
}

/// Trade a refresh token for a new set.
///
/// ⚠️ No client secret, on either provider. GitHub documents a device-flow
/// app's refresh with `client_id`, `grant_type` and `refresh_token` only.
/// GitLab's documentation shows `client_secret` on a refresh and does not
/// say whether a NON-confidential application may leave it out. Measured on
/// gitlab.com 2026-09-19 with bridgewatch's own application: a refresh with
/// `client_id` alone answers 200 with a new pair, the old access token then
/// answers 401, and replaying the old refresh token answers `invalid_grant`.
/// A refusal here lands on [`OAuthError::SignInAgain`], i.e. "sign in
/// again", rather than anything that loops.
/// <https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/refreshing-user-access-tokens>
/// <https://docs.gitlab.com/api/oauth2/#authorization-code-flow>
pub async fn refresh(
    transport: &dyn Transport,
    endpoints: &Endpoints,
    client_id: &str,
    refresh_token: &Secret,
) -> Result<TokenReplyView, OAuthError> {
    let body = form(&[
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
    ]);
    let response = post(transport, &endpoints.token_url, body)
        .await
        .map_err(OAuthError::Client)?;
    if let Some(e) = transport_status(&response, Endpoints::path_of(&endpoints.token_url)) {
        return Err(OAuthError::Client(e));
    }
    let reply: Option<TokenReply> = serde_json::from_str(&response.body).ok();
    match reply {
        Some(reply)
            if reply.error.is_none()
                && reply.access_token.is_some()
                && (200..300).contains(&response.status) =>
        {
            Ok(TokenReplyView(reply))
        }
        // Every other answer, an OAuth error code or a bare 4xx, means the
        // refresh token will not work again, whatever the code says:
        // `invalid_grant` (GitLab, and RFC 6749), `bad_refresh_token` (GitHub),
        // a revoked application. Retrying it would be the silent loop.
        Some(reply) => Err(OAuthError::SignInAgain {
            code: reply
                .error
                .map(|c| sanitize_code(&c))
                .unwrap_or_else(|| format!("HTTP {}", response.status)),
        }),
        None if (400..500).contains(&response.status) => Err(OAuthError::SignInAgain {
            code: format!("HTTP {}", response.status),
        }),
        None => Err(OAuthError::Decode {
            host: endpoints.host.clone(),
            message: "the refresh answer is not JSON".to_string(),
        }),
    }
}

/// Finish a sign-in: ask who it is, and store the set for `account_name`.
///
/// The `/user` request goes through `api_transport`, the ordinary client path.
/// A failure there does not lose the sign-in: the set is stored without a
/// login, and the poller's first request says whether the token works.
pub async fn complete_sign_in(
    account_name: &str,
    account: &Account,
    client_id: &str,
    reply: TokenReplyView,
    api_transport: Arc<dyn Transport>,
    store_provider: &dyn TokenProvider,
    now: u64,
) -> Result<(TokenSet, SignedIn), OAuthError> {
    let mut set = reply.into_set(account, client_id, now, None, None);
    if let Ok(client) = client_for(
        &super::bearer_account(account),
        &set.access_token,
        api_transport,
        RequestRing::new(1),
    ) {
        match client.current_user().await {
            Ok(user) => set.login = Some(user.username),
            Err(e) => {
                tracing::warn!(account = account_name, error = %e, "signed in, but could not ask who")
            }
        }
    }
    store::save(account_name, &set, store_provider)?;
    tracing::info!(account = account_name, "signed in");
    let signed_in = SignedIn {
        login: set.login.clone(),
        scopes: set.scopes.clone(),
        expires_at: set.expires_at,
    };
    Ok((set, signed_in))
}
