//! The token set a sign-in produces, and where it is kept.
//!
//! One JSON document per account, in the OS credential store under service
//! `bridgewatch:oauth:<account>` and user `oauth`. The user differs from the
//! `own` entry's (`token`), so no account name can make the two addresses
//! collide, and the configuration file never holds any of it: it says only
//! `token = { oauth = true }`.
//!
//! The document is written in ONE credential-store call, which is what makes
//! GitLab's refresh-token rotation survivable: the new access token and the new
//! refresh token land together or not at all (see [`super::session`]).

use serde::{Deserialize, Serialize};

use super::{OAuthError, host_of, origin_of};
use crate::config::Account;
use crate::token::{Secret, TokenError, TokenProvider};

/// The credential-store user every sign-in is kept under.
pub const STORE_USER: &str = "oauth";

/// The version written into every document, so a later layout can tell an
/// older one apart instead of misreading it.
pub const STORE_VERSION: u32 = 1;

/// The credential-store service for an account's sign-in.
pub fn service(account: &str) -> String {
    format!("bridgewatch:oauth:{account}")
}

/// Everything one sign-in holds.
///
/// ⛔ `Debug` is hand-written and prints neither token; a derived one would
/// have put both in any `{:?}`.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenSet {
    /// What goes in `Authorization: Bearer`.
    pub access_token: Secret,
    /// What renews it. `None` for a GitHub App whose tokens do not expire.
    pub refresh_token: Option<Secret>,
    /// When the access token expires, Unix seconds. `None` is "not said".
    pub expires_at: Option<u64>,
    /// When the refresh token expires, Unix seconds, where the provider says.
    pub refresh_expires_at: Option<u64>,
    /// The granted scopes. Empty for a GitHub App, which has permissions
    /// rather than scopes.
    pub scopes: Vec<String>,
    /// Who signed in, as the provider names them, when it could be asked.
    pub login: Option<String>,
    /// The application the tokens belong to. A refresh must use the same one.
    pub client_id: String,
    /// The instance the tokens were issued by. A token is never sent anywhere
    /// else: a stored set whose origin differs from the account's is ignored.
    pub base_url: String,
    /// When this set was obtained, Unix seconds.
    pub obtained_at: u64,
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &self.access_token)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .field("scopes", &self.scopes)
            .field("login", &self.login)
            .field("client_id", &self.client_id)
            .field("base_url", &self.base_url)
            .field("obtained_at", &self.obtained_at)
            .finish()
    }
}

impl TokenSet {
    /// True when the access token expires within `margin` seconds of `now`.
    /// A set that states no expiry never needs refreshing ahead of time.
    pub fn expires_within(&self, now: u64, margin: u64) -> bool {
        self.expires_at
            .is_some_and(|at| at <= now.saturating_add(margin))
    }

    /// Whether this set may be used for `account`, whose sign-in application
    /// is `client_id`: issued by the same instance, for the same application.
    ///
    /// ⛔ A set from another host is never sent: the configuration's
    /// `base_url` can change under a stored sign-in (by hand, or by a hostile
    /// edit), and the token would otherwise go wherever it now points. The
    /// poller's session, `token::resolve`, `auth status` and the app's
    /// `oauth_status` all ask this one question, so none of them can come to
    /// call a sign-in usable that another refuses.
    pub fn fits(&self, account: &Account, client_id: &str) -> bool {
        self.fit(account, client_id) == Fit::Fits
    }

    /// [`Self::fits`], with the reason when it does not.
    pub fn fit(&self, account: &Account, client_id: &str) -> Fit {
        self.fit_for(&account.base_url, client_id)
    }

    /// [`Self::fit`] against a `base_url` and a client id directly, for the
    /// session, which keeps those rather than the whole account.
    pub(crate) fn fit_for(&self, base_url: &str, client_id: &str) -> Fit {
        let (issued, expected) = (origin_of(&self.base_url), origin_of(base_url));
        if issued != expected {
            // The host alone reads best; the whole origin only when the hosts
            // agree and the scheme or port is what differs.
            let (mut issued, mut expected) = (label(&self.base_url), label(base_url));
            if issued == expected {
                issued = origin_of(&self.base_url).unwrap_or(issued);
                expected = origin_of(base_url).unwrap_or(expected);
            }
            return Fit::OtherInstance { issued, expected };
        }
        if self.client_id != client_id {
            return Fit::OtherApplication;
        }
        Fit::Fits
    }

    /// What a UI or `auth status` may show. No token in it.
    pub fn status(&self) -> SignInStatus {
        SignInStatus {
            login: self.login.clone(),
            expires_at: self.expires_at,
            refresh_expires_at: self.refresh_expires_at,
            scopes: self.scopes.clone(),
            obtained_at: self.obtained_at,
            can_refresh: self.refresh_token.is_some(),
        }
    }

    /// The document written to the credential store.
    pub fn to_json(&self) -> String {
        let doc = Stored {
            version: STORE_VERSION,
            access_token: self.access_token.expose().to_string(),
            refresh_token: self.refresh_token.as_ref().map(|s| s.expose().to_string()),
            expires_at: self.expires_at,
            refresh_expires_at: self.refresh_expires_at,
            scopes: self.scopes.clone(),
            login: self.login.clone(),
            client_id: self.client_id.clone(),
            base_url: self.base_url.clone(),
            obtained_at: self.obtained_at,
        };
        // Every field is a string, a number or a list of strings, so this
        // cannot fail; the fallback exists only so a panic cannot either.
        serde_json::to_string(&doc).unwrap_or_default()
    }

    /// Read a stored document.
    ///
    /// ⛔ The error never quotes the document or serde's rendering of it, which
    /// can include a value: it names the line and column and nothing else.
    pub fn from_json(raw: &str) -> Result<Self, OAuthError> {
        let doc: Stored = serde_json::from_str(raw).map_err(|e| {
            OAuthError::Store(format!(
                "the stored sign-in is not readable ({:?} at line {} column {}); sign in again",
                e.classify(),
                e.line(),
                e.column()
            ))
        })?;
        if doc.version > STORE_VERSION {
            return Err(OAuthError::Store(format!(
                "the stored sign-in was written by a newer bridgewatch (layout {}); sign in again \
                 or update",
                doc.version
            )));
        }
        Ok(Self {
            access_token: Secret::new(doc.access_token),
            refresh_token: doc.refresh_token.map(Secret::new),
            expires_at: doc.expires_at,
            refresh_expires_at: doc.refresh_expires_at,
            scopes: doc.scopes,
            login: doc.login,
            client_id: doc.client_id,
            base_url: doc.base_url,
            obtained_at: doc.obtained_at,
        })
    }
}

/// Whether a stored sign-in may be used for an account ([`TokenSet::fit`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fit {
    /// Issued by the account's instance, for its application.
    Fits,
    /// Issued by another instance than the account's `base_url` names.
    OtherInstance {
        /// The instance that issued it, as a host (`gitlab.example.com`).
        issued: String,
        /// The account's instance, the same way.
        expected: String,
    },
    /// Issued by the right instance, for another application than the
    /// account's client id.
    OtherApplication,
}

impl Fit {
    /// Why the sign-in is not used, as the middle of a sentence:
    /// "signed in for gitlab.com, not gitlab.example.com". `None` when it fits.
    pub fn mismatch(&self) -> Option<String> {
        match self {
            Fit::Fits => None,
            Fit::OtherInstance { issued, expected } => {
                Some(format!("signed in for {issued}, not {expected}"))
            }
            Fit::OtherApplication => {
                Some("signed in with another application than this account's client id".to_string())
            }
        }
    }
}

/// A URL's host for a sentence.
///
/// ⛔ Never the URL itself when it has no host [`origin_of`] accepts: that is
/// how `https://user:token@host` is refused, and echoing it would print the
/// credential.
fn label(url: &str) -> String {
    host_of(url).unwrap_or_else(|| "an address that is not an http(s) URL".to_string())
}

/// What a sign-in may say about itself in public: who, until when, with
/// what. Never a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInStatus {
    /// Who signed in.
    pub login: Option<String>,
    /// When the access token expires, Unix seconds.
    pub expires_at: Option<u64>,
    /// When the refresh token expires, Unix seconds.
    pub refresh_expires_at: Option<u64>,
    /// The granted scopes.
    pub scopes: Vec<String>,
    /// When the sign-in happened or was last refreshed, Unix seconds.
    pub obtained_at: u64,
    /// Whether it renews itself. A set without a refresh token lasts until
    /// its access token does.
    pub can_refresh: bool,
}

/// The document's field names, which are the stored layout.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    refresh_expires_at: Option<u64>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    login: Option<String>,
    client_id: String,
    base_url: String,
    #[serde(default)]
    obtained_at: u64,
}

/// Read an account's sign-in. `Ok(None)` when there is none.
pub fn load(account: &str, store: &dyn TokenProvider) -> Result<Option<TokenSet>, OAuthError> {
    match store.keyring_get(&service(account), STORE_USER) {
        Ok(raw) => TokenSet::from_json(&raw).map(Some),
        Err(TokenError::NoEntry { .. }) => Ok(None),
        Err(e) => Err(OAuthError::Store(e.to_string())),
    }
}

/// Write an account's sign-in, replacing whatever was there, in one call.
pub fn save(account: &str, set: &TokenSet, store: &dyn TokenProvider) -> Result<(), OAuthError> {
    store
        .keyring_set(&service(account), STORE_USER, &set.to_json())
        .map_err(|e| OAuthError::Store(e.to_string()))
}

/// Forget an account's sign-in. Forgetting nothing is not an error.
pub fn delete(account: &str, store: &dyn TokenProvider) -> Result<(), OAuthError> {
    store
        .keyring_delete(&service(account), STORE_USER)
        .map_err(|e| OAuthError::Store(e.to_string()))
}

/// Move a sign-in to another account name: the wizard signs in before the
/// user has settled what to call the account.
///
/// The new entry is written before the old one is removed, so a failure in
/// between leaves two copies rather than none.
pub fn rename(from: &str, to: &str, store: &dyn TokenProvider) -> Result<bool, OAuthError> {
    if from == to {
        return Ok(false);
    }
    let Some(set) = load(from, store)? else {
        return Ok(false);
    };
    save(to, &set, store)?;
    delete(from, store)?;
    Ok(true)
}
