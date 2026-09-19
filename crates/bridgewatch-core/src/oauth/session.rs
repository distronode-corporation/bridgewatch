//! A signed-in account's credential at runtime: fresh on every request.
//!
//! [`OAuthTransport`] sits between an ordinary client and the real transport.
//! Before each request it asks the [`OAuthSession`] for an access token, which
//! refreshes it first when it expires within [`REFRESH_MARGIN_SECS`]; after a
//! 401 it refreshes ONCE and retries ONCE. The GitLab and GitHub clients above
//! it are the same clients a personal access token uses, and cannot tell.
//!
//! # A refresh that cannot work is said once, plainly
//!
//! When a refresh is refused (the refresh token expired, was revoked, or was
//! already spent), the session remembers it and answers every later request
//! with [`ClientError::SignInAgain`] WITHOUT asking the server again, until the
//! credential store holds a different sign-in. That error is fatal to the
//! poller's backoff, and its message says to sign in again and where.
//!
//! # GitLab rotates, so the new pair is stored before it is used
//!
//! A GitLab refresh invalidates the old access token AND the old refresh token
//! the moment it answers. A process that received the new pair and died
//! before writing it down would leave the store holding a spent refresh token.
//! So a refreshed set is written to the credential store, in one call, BEFORE
//! it is handed to anything that might send it; `tests/oauth.rs` checks the
//! store from inside the very request that uses the new token. If the write
//! itself fails, the set is still used for this run (throwing away the only
//! valid pair would strand the user immediately) and the write is retried on
//! every later request until it succeeds.
//!
//! A second process (the CLI next to the tray) can spend the same refresh
//! token. So before refreshing, and again after a refusal, the session reads
//! the store: a set somebody else stored is adopted rather than refreshed over.

use std::sync::Arc;

use super::store::{self, TokenSet};
use super::{BuiltinClients, Endpoints, OAuthError, client_id_for, origin_of};
use crate::client::{ClientError, HttpRequest, HttpResponse, Transport};
use crate::config::{Account, OAuthSource, Provider};
use crate::token::{Secret, TokenProvider};

/// How long before expiry a token is refreshed. A GitLab token lasts about two
/// hours and the slowest poll is a few minutes, so five minutes means a poll
/// never sends a token that expires in flight.
pub const REFRESH_MARGIN_SECS: u64 = 300;

/// How recently a token must have been obtained for a 401 on it to be taken
/// as the endpoint's answer rather than as the token running out.
///
/// ⚠️ Not every 401 is an expired token. GitLab's
/// `/personal_access_tokens/self`, which the wizard's "Test connection" asks,
/// refuses EVERY OAuth token with a 401, and on GitLab a refresh rotates the
/// pair: refreshing on that answer would spend a refresh token each time
/// somebody pressed the button, and prove nothing. A token obtained this
/// recently has not expired, so its 401 is passed through untouched.
pub const FRESH_SECS: u64 = 60;

/// Seconds since the Unix epoch. Injected so the tests can move time.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The wall clock.
pub fn system_clock() -> Clock {
    Arc::new(super::now)
}

/// One account's live sign-in.
pub struct OAuthSession {
    account: String,
    provider: Provider,
    base_origin: Option<String>,
    endpoints: Endpoints,
    client_id: String,
    transport: Arc<dyn Transport>,
    store: Arc<dyn TokenProvider>,
    clock: Clock,
    state: tokio::sync::Mutex<State>,
}

#[derive(Default)]
struct State {
    current: Option<TokenSet>,
    /// The store has been read at least once.
    loaded: bool,
    /// `current` is newer than what the store holds, because writing it failed.
    unsaved: bool,
    /// A refresh of `current` was refused; say so without asking again.
    dead: bool,
}

impl std::fmt::Debug for OAuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthSession")
            .field("account", &self.account)
            .field("provider", &self.provider)
            .field("endpoints", &self.endpoints)
            .finish_non_exhaustive()
    }
}

impl OAuthSession {
    /// A session for `account_name`, which reads and writes its sign-in in
    /// `store` and refreshes through `transport`.
    ///
    /// Nothing is read yet: the store is first read by the first request, on
    /// the poll task, not while the poller is being built.
    pub fn new(
        account_name: &str,
        account: &Account,
        source: &OAuthSource,
        builtin: &BuiltinClients,
        transport: Arc<dyn Transport>,
        store: Arc<dyn TokenProvider>,
    ) -> Result<Self, OAuthError> {
        Ok(Self {
            account: account_name.to_string(),
            provider: account.provider,
            base_origin: origin_of(&account.base_url),
            endpoints: Endpoints::for_account(account),
            client_id: client_id_for(account, source, builtin)?,
            transport,
            store,
            clock: system_clock(),
            state: tokio::sync::Mutex::new(State::default()),
        })
    }

    /// Replace the clock, for tests.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    fn sign_in_again(&self, never_signed_in: bool) -> ClientError {
        ClientError::SignInAgain {
            account: self.account.clone(),
            provider: self.provider,
            never_signed_in,
        }
    }

    /// Whether a stored set may be used for this account: issued by the same
    /// instance, for the same application.
    ///
    /// ⛔ A set from another host is never sent: the configuration's
    /// `base_url` can change under a stored sign-in (by hand, or by a hostile
    /// edit), and the token would otherwise go wherever it now points.
    fn fits(&self, set: &TokenSet) -> bool {
        origin_of(&set.base_url) == self.base_origin && set.client_id == self.client_id
    }

    /// Read the store; adopt what is there if it differs from what is held.
    /// Returns whether anything new was adopted.
    fn adopt_stored(&self, state: &mut State) -> bool {
        state.loaded = true;
        match store::load(&self.account, self.store.as_ref()) {
            Ok(Some(set)) if self.fits(&set) => {
                if state.current.as_ref() == Some(&set) {
                    return false;
                }
                if state.unsaved {
                    // What is held is newer than the store and could not be
                    // written; the store's copy is the spent one.
                    return false;
                }
                state.current = Some(set);
                state.dead = false;
                true
            }
            Ok(Some(_)) => {
                tracing::warn!(
                    account = %self.account,
                    "the stored sign-in is for another instance or application; sign in again"
                );
                false
            }
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(account = %self.account, error = %e, "could not read the stored sign-in");
                false
            }
        }
    }

    /// Retry a write that failed earlier.
    fn flush_unsaved(&self, state: &mut State) {
        if !state.unsaved {
            return;
        }
        if let Some(set) = &state.current
            && store::save(&self.account, set, self.store.as_ref()).is_ok()
        {
            state.unsaved = false;
            tracing::info!(account = %self.account, "saved the refreshed sign-in");
        }
    }

    /// The access token to send now, refreshed first when it is about to
    /// expire.
    pub async fn access_token(&self) -> Result<Secret, ClientError> {
        let mut state = self.state.lock().await;
        self.flush_unsaved(&mut state);
        if !state.loaded || state.current.is_none() || state.dead {
            self.adopt_stored(&mut state);
        }
        if state.dead {
            return Err(self.sign_in_again(false));
        }
        let Some(set) = &state.current else {
            return Err(self.sign_in_again(true));
        };
        if set.expires_within((self.clock)(), REFRESH_MARGIN_SECS) {
            return self.refresh_locked(&mut state, false).await;
        }
        Ok(set.access_token.clone())
    }

    /// The server refused `rejected`: refresh once and hand back what to retry
    /// with. When somebody already replaced `rejected`, that replacement is the
    /// answer and no refresh is made. `None` means "do not retry": the token
    /// is too new to have run out, so the refusal is the endpoint's own (see
    /// [`FRESH_SECS`]).
    pub async fn after_rejection(&self, rejected: &Secret) -> Result<Option<Secret>, ClientError> {
        let mut state = self.state.lock().await;
        if let Some(set) = &state.current {
            if &set.access_token != rejected {
                return Ok(Some(set.access_token.clone()));
            }
            let now = (self.clock)();
            if now < set.obtained_at.saturating_add(FRESH_SECS) && !set.expires_within(now, 0) {
                return Ok(None);
            }
        }
        if state.dead {
            return Err(self.sign_in_again(false));
        }
        self.refresh_locked(&mut state, true).await.map(Some)
    }

    async fn refresh_locked(&self, state: &mut State, forced: bool) -> Result<Secret, ClientError> {
        // Somebody else may have refreshed, and on GitLab that spent our copy.
        let before = state.current.as_ref().map(|s| s.access_token.clone());
        self.adopt_stored(state);
        if let Some(set) = &state.current {
            let replaced = before.as_ref() != Some(&set.access_token);
            if replaced && !set.expires_within((self.clock)(), REFRESH_MARGIN_SECS) {
                return Ok(set.access_token.clone());
            }
        }
        let Some(current) = state.current.clone() else {
            return Err(self.sign_in_again(true));
        };
        let Some(refresh_token) = current.refresh_token.clone() else {
            // A token that does not expire and still got a 401 was revoked.
            // One that expires and cannot be renewed has run out.
            if forced || current.expires_within((self.clock)(), 0) {
                state.dead = true;
                return Err(self.sign_in_again(false));
            }
            return Ok(current.access_token.clone());
        };

        match super::device::refresh(
            self.transport.as_ref(),
            &self.endpoints,
            &self.client_id,
            &refresh_token,
        )
        .await
        {
            Ok(reply) => {
                let now = (self.clock)();
                let account = Account {
                    base_url: current.base_url.clone(),
                    ..Account::for_provider(self.provider)
                };
                let set = reply.into_set(
                    &account,
                    &self.client_id,
                    now,
                    current.login.clone(),
                    Some(refresh_token),
                );
                // ⛔ Written BEFORE it is returned: see the module docs.
                match store::save(&self.account, &set, self.store.as_ref()) {
                    Ok(()) => state.unsaved = false,
                    Err(e) => {
                        state.unsaved = true;
                        tracing::warn!(
                            account = %self.account,
                            error = %e,
                            "could not save the refreshed sign-in; it is used for this run and \
                             saving is retried on every request"
                        );
                    }
                }
                tracing::debug!(account = %self.account, "sign-in refreshed");
                let token = set.access_token.clone();
                state.current = Some(set);
                state.dead = false;
                Ok(token)
            }
            Err(OAuthError::Client(e)) => {
                // The network, not the sign-in. A token that is still good is
                // better than an error; one that is not gets the error, and the
                // poller backs off as it would for any failed request.
                if !forced && !current.expires_within((self.clock)(), 0) {
                    tracing::debug!(account = %self.account, error = %e, "refresh failed; the token is still valid");
                    return Ok(current.access_token.clone());
                }
                Err(e)
            }
            Err(e) => {
                // One more look: a second process may have refreshed between
                // our read and our request, which is exactly the refusal a
                // spent refresh token gets.
                if self.adopt_stored(state)
                    && let Some(set) = &state.current
                    && !set.expires_within((self.clock)(), 0)
                {
                    return Ok(set.access_token.clone());
                }
                tracing::warn!(account = %self.account, error = %e, "the sign-in could not be refreshed; sign in again");
                state.dead = true;
                Err(self.sign_in_again(false))
            }
        }
    }
}

/// Puts the session's access token on every request, and refreshes once on a
/// 401.
pub struct OAuthTransport {
    inner: Arc<dyn Transport>,
    session: Arc<OAuthSession>,
}

impl std::fmt::Debug for OAuthTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTransport")
            .field("inner", &self.inner)
            .field("session", &self.session)
            .finish()
    }
}

impl OAuthTransport {
    /// Wrap `inner`.
    pub fn new(inner: Arc<dyn Transport>, session: Arc<OAuthSession>) -> Self {
        Self { inner, session }
    }
}

/// `request` carrying `token` as a bearer credential, whatever credential
/// header the client rendered (an empty `PRIVATE-TOKEN` from a GitLab account's
/// default, or an empty bearer).
fn authorised(request: &HttpRequest, token: &Secret) -> HttpRequest {
    let mut out = request.clone();
    out.headers.retain(|(name, _)| {
        !name.eq_ignore_ascii_case("authorization") && !name.eq_ignore_ascii_case("private-token")
    });
    out.headers.push((
        "Authorization".to_string(),
        format!("Bearer {}", token.expose()),
    ));
    out
}

#[async_trait::async_trait]
impl Transport for OAuthTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        let token = self.session.access_token().await?;
        let first = self.inner.execute(authorised(&request, &token)).await?;
        if first.status != 401 {
            return Ok(first);
        }
        // Once. A 401 on the retry is the answer, and the client reads it as
        // the refusal it is; so is a 401 on a token too new to have expired.
        match self.session.after_rejection(&token).await? {
            Some(fresh) => self.inner.execute(authorised(&request, &fresh)).await,
            None => Ok(first),
        }
    }
}

/// The transport an `oauth` account's client is built on: `inner`, with the
/// account's sign-in on every request.
pub fn transport_for(
    account_name: &str,
    account: &Account,
    source: &OAuthSource,
    builtin: &BuiltinClients,
    inner: Arc<dyn Transport>,
    store: Arc<dyn TokenProvider>,
) -> Result<Arc<dyn Transport>, OAuthError> {
    let session = OAuthSession::new(account_name, account, source, builtin, inner.clone(), store)?;
    Ok(Arc::new(OAuthTransport::new(inner, Arc::new(session))))
}
