//! "Sign in with GitHub" and "Sign in with GitLab": the OAuth 2.0 device
//! authorization grant, the stored token set, and the credential that keeps
//! itself fresh.
//!
//! # Why the device grant
//!
//! It is the one OAuth flow a tray app can use without a client secret and
//! without a redirect listener: bridgewatch asks the provider for a short user
//! code, the user types it at the provider's own page in their browser, and
//! bridgewatch polls until the provider says yes (RFC 8628). Both providers
//! support it for a public client:
//!
//! - GitHub, a GitHub App with "Enable Device Flow" ticked:
//!   <https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app#using-the-device-flow-to-generate-a-user-access-token>
//! - GitLab, an application that is not confidential, scope `read_api`,
//!   generally available from GitLab 17.9:
//!   <https://docs.gitlab.com/api/oauth2/#device-authorization-grant-flow>
//!
//! # Where things live
//!
//! - [`clients`]: the built-in client ids, one constant each.
//! - [`device`]: starting a sign-in, polling it to completion, and refreshing.
//! - [`store`]: the token set, kept as JSON in the OS credential store under
//!   `bridgewatch:oauth:<account>`, never in `config.toml`.
//! - [`session`]: [`OAuthTransport`], which puts a fresh access token on every
//!   API request and refreshes once on a 401, so the clients above it never
//!   know a token can expire.
//!
//! ⛔ No token, device code or refresh token reaches a log line, an error
//! message or a `Debug` rendering. The user code is shown to the user by the
//! shell and the CLI, and is still never logged. `tests/oauth_log.rs` captures
//! the logs of a whole sign-in and refresh and looks for every one of them.

pub mod clients;
pub mod device;
pub mod session;
pub mod store;

pub use clients::BuiltinClients;
pub use device::{
    DeviceAuthorization, Progress, SignedIn, complete_sign_in, refresh, start, wait_for_token,
};
pub use session::{Clock, OAuthSession, OAuthTransport, system_clock, transport_for};
pub use store::{SignInStatus, TokenSet};

use serde::Serialize;

use crate::client::ClientError;
use crate::config::{Account, OAuthSource, Provider};

/// The scope bridgewatch asks GitLab for: the same `read_api` a personal
/// access token needs, and nothing more.
///
/// GitHub takes no scope for a GitHub App: what a user token can reach is the
/// intersection of the user's access and the app's permissions.
pub const GITLAB_SCOPE: &str = "read_api";

/// Where one account signs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Which provider.
    pub provider: Provider,
    /// Where to ask for a device code.
    pub device_code_url: String,
    /// Where to exchange it (and a refresh token) for a token set.
    pub token_url: String,
    /// The scope to ask for, when the provider takes one.
    pub scope: Option<&'static str>,
    /// The origin the verification page must be on: GitHub's web host, or the
    /// GitLab instance itself. A device-code answer naming any other host is
    /// refused rather than opened.
    pub origin: String,
    /// The host, for sentences: `github.com`, `gitlab.example.com`.
    pub host: String,
}

impl Endpoints {
    /// The endpoints for an account.
    ///
    /// GitHub's live on the WEB host, never the API host: `github.com`, or a
    /// GitHub Enterprise Server's own host (`<host>/login/device/code`), which
    /// [`crate::client::github::web_origin`] already derives for links. GitLab's
    /// live under the instance root, path prefix included.
    pub fn for_account(account: &Account) -> Self {
        match account.provider {
            Provider::Github => {
                let web = crate::client::github::web_origin(&account.base_url);
                let web = web.trim_end_matches('/').to_string();
                Self {
                    provider: Provider::Github,
                    device_code_url: format!("{web}/login/device/code"),
                    token_url: format!("{web}/login/oauth/access_token"),
                    scope: None,
                    host: host_of(&web).unwrap_or_else(|| web.clone()),
                    origin: origin_of(&web).unwrap_or_else(|| web.clone()),
                }
            }
            Provider::Gitlab => {
                let base = account.base_url.trim().trim_end_matches('/').to_string();
                Self {
                    provider: Provider::Gitlab,
                    device_code_url: format!("{base}/oauth/authorize_device"),
                    token_url: format!("{base}/oauth/token"),
                    scope: Some(GITLAB_SCOPE),
                    host: host_of(&base).unwrap_or_else(|| base.clone()),
                    origin: origin_of(&base).unwrap_or_else(|| base.clone()),
                }
            }
        }
    }

    /// The path part of a URL on these endpoints, for a log line.
    pub(crate) fn path_of(url: &str) -> &str {
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        rest.find('/').map(|i| &rest[i..]).unwrap_or("/")
    }
}

/// Everything signing in, refreshing or reading a stored sign-in can fail with.
///
/// ⛔ No variant holds a token, a device code or a refresh token, and no
/// message is built from a response body: a provider's `error` code is kept
/// only after [`sanitize_code`] reduces it to the characters an OAuth error
/// code is made of.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OAuthError {
    /// The device code expired before the user entered it.
    #[error("the code expired before it was entered; start the sign-in again")]
    Expired,
    /// The user pressed Cancel on the provider's page.
    #[error("the sign-in was cancelled on {host}")]
    Denied {
        /// Where.
        host: String,
    },
    /// bridgewatch stopped waiting because it was asked to.
    #[error("the sign-in was stopped")]
    Cancelled,
    /// There is no client id to sign in with. The message says what to write.
    #[error("{0}")]
    NoClientId(String),
    /// The provider refused the request with an OAuth error code, for a reason
    /// no retry will change (a disabled device flow, an unknown client id).
    #[error("{host} refused the sign-in ({code}){}", advice(.code))]
    Refused {
        /// Where.
        host: String,
        /// The OAuth `error` code, sanitised.
        code: String,
    },
    /// A stored sign-in can no longer be refreshed: it expired, was revoked,
    /// or was rotated by somebody else. The user has to sign in again.
    #[error("the sign-in has expired or was revoked ({code}); sign in again")]
    SignInAgain {
        /// The OAuth `error` code, sanitised, or a word of ours.
        code: String,
    },
    /// Nothing is stored for this account.
    #[error("not signed in")]
    NotSignedIn,
    /// The request did not complete, or the provider answered with a status
    /// that says nothing about the sign-in itself.
    #[error(transparent)]
    Client(ClientError),
    /// The answer was not the OAuth JSON it should have been.
    #[error("{host} answered the sign-in with something that is not OAuth: {message}")]
    Decode {
        /// Where.
        host: String,
        /// What was wrong, never the body.
        message: String,
    },
    /// The OS credential store could not read or write the sign-in.
    #[error("the credential store could not keep the sign-in: {0}")]
    Store(String),
}

impl OAuthError {
    /// A stable word for the UI to choose its sentence by.
    pub fn kind(&self) -> &'static str {
        match self {
            OAuthError::Expired => "expired",
            OAuthError::Denied { .. } => "denied",
            OAuthError::Cancelled => "cancelled",
            OAuthError::NoClientId(_) => "no_client_id",
            OAuthError::SignInAgain { .. } | OAuthError::NotSignedIn => "sign_in_again",
            OAuthError::Refused { .. }
            | OAuthError::Client(_)
            | OAuthError::Decode { .. }
            | OAuthError::Store(_) => "error",
        }
    }
}

/// One more sentence for the refusals a user can do something about.
fn advice(code: &str) -> &'static str {
    match code {
        "device_flow_disabled" => {
            ": the application does not have the device flow enabled. On GitHub, tick \
             \"Enable Device Flow\" in the app's settings"
        }
        "invalid_client" | "incorrect_client_credentials" | "unauthorized_client" => {
            ": the client id is not an application this server knows, or the application \
             is confidential. Check the client id; on GitLab, untick \"Confidential\""
        }
        "invalid_scope" => ": the application does not allow the read_api scope",
        "unsupported_grant_type" => {
            ": this server does not offer the device grant (GitLab needs 17.9 or later, or \
             the oauth2_device_grant_flow feature flag)"
        }
        _ => "",
    }
}

/// Reduce a provider's `error` code to what an OAuth error code is made of.
///
/// RFC 6749 limits the code to printable ASCII without `"` or `\`; this is
/// narrower still (letters, digits, `_`, `-`, `.`) and at most 64 characters,
/// because it is the one piece of a response body that reaches a message.
pub fn sanitize_code(code: &str) -> String {
    let clean: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(64)
        .collect();
    if clean.is_empty() {
        "unknown_error".to_string()
    } else {
        clean
    }
}

/// True when the account is on github.com or gitlab.com, the two hosts that
/// can have a built-in application.
pub fn is_hosted(account: &Account) -> bool {
    match account.provider {
        Provider::Github => {
            origin_of(&crate::client::github::web_origin(&account.base_url)).as_deref()
                == Some("https://github.com")
        }
        Provider::Gitlab => origin_of(&account.base_url).as_deref() == Some("https://gitlab.com"),
    }
}

/// The built-in client id for an account's host, if this build has one.
pub fn builtin_client_id(account: &Account, builtin: &BuiltinClients) -> Option<&'static str> {
    if !is_hosted(account) {
        return None;
    }
    match account.provider {
        Provider::Github => builtin.github_com,
        Provider::Gitlab => builtin.gitlab_com,
    }
    .filter(|id| !id.trim().is_empty())
}

/// The client id an `oauth` source signs in with: its own, else the host's
/// built-in one.
///
/// ⛔ A self-managed server never falls back to the built-in id. An
/// application registered on gitlab.com does not exist on
/// gitlab.example.com, and asking there with its id is an `invalid_client`
/// that reads like a typo in somebody else's configuration.
pub fn client_id_for(
    account: &Account,
    source: &OAuthSource,
    builtin: &BuiltinClients,
) -> Result<String, OAuthError> {
    if let Some(id) = source.client_id.as_deref() {
        let id = id.trim();
        if id.is_empty() {
            return Err(OAuthError::NoClientId(
                "client_id is empty: remove it to use the built-in application, or write \
                 the client id of your own"
                    .to_string(),
            ));
        }
        return Ok(id.to_string());
    }
    if let Some(id) = builtin_client_id(account, builtin) {
        return Ok(id.to_string());
    }
    Err(OAuthError::NoClientId(missing_client_id_message(account)))
}

/// Why there is no client id, and what to write.
pub fn missing_client_id_message(account: &Account) -> String {
    let host = Endpoints::for_account(account).host;
    if is_hosted(account) {
        format!(
            "this build of bridgewatch has no built-in sign-in application for {host} yet; \
             write token = {{ oauth = {{ client_id = \"...\" }} }} with an application of \
             your own, or use another token source"
        )
    } else {
        let what = match account.provider {
            Provider::Github => "a GitHub App on that server with \"Enable Device Flow\" ticked",
            Provider::Gitlab => {
                "an OAuth application on that instance, not confidential, with the read_api scope"
            }
        };
        format!(
            "signing in to {host} needs that server's own application ({what}): register \
             one and write token = {{ oauth = {{ client_id = \"...\" }} }}"
        )
    }
}

/// Where to install bridgewatch's GitHub App, for a github.com account whose
/// build knows the app's slug.
pub fn install_url(account: &Account, builtin: &BuiltinClients) -> Option<String> {
    if account.provider != Provider::Github || !is_hosted(account) {
        return None;
    }
    builtin
        .github_app_slug
        .filter(|s| !s.trim().is_empty())
        .map(|slug| format!("https://github.com/apps/{slug}/installations/new"))
}

/// What to add to a "not found" on a GitHub account that signs in.
///
/// A GitHub App's user token reaches only the repositories that BOTH the user
/// and one of the app's installations can see, so a repository the user can
/// open in a browser is a 404 here until the app is installed on the
/// organisation that owns it. Nothing in the 404 says so; this does.
pub fn not_found_hint(account: &Account, builtin: &BuiltinClients) -> Option<String> {
    if account.provider != Provider::Github
        || !matches!(account.token, crate::config::TokenSource::Oauth(_))
    {
        return None;
    }
    Some(match install_url(account, builtin) {
        Some(url) => format!(
            "signed in through the bridgewatch GitHub App, which sees only repositories on \
             accounts it is installed on; install it on the owner of this repository: {url}"
        ),
        None => "signed in through a GitHub App, which sees only repositories on accounts it \
                 is installed on; install the app on the owner of this repository"
            .to_string(),
    })
}

/// Whether to offer "Sign in" for an instance, as the wizard and Settings ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Availability {
    /// Sign-in can start: a client id is typed, or the host has a built-in one.
    pub available: bool,
    /// The host has a built-in application in this build.
    pub builtin: bool,
    /// The host is one that can only ever sign in with a typed client id
    /// (GitHub Enterprise Server, self-managed GitLab).
    pub needs_client_id: bool,
    /// The host, for the button: `github.com`.
    pub host: String,
    /// Where to install the GitHub App, when this build knows.
    pub install_url: Option<String>,
}

/// [`Availability`] for a provider, an instance and an optional typed client
/// id.
pub fn availability(
    provider: Provider,
    base_url: &str,
    client_id: Option<&str>,
    builtin: &BuiltinClients,
) -> Availability {
    let account = Account {
        base_url: base_url.trim().to_string(),
        ..Account::for_provider(provider)
    };
    let typed = client_id.is_some_and(|c| !c.trim().is_empty());
    let has_builtin = builtin_client_id(&account, builtin).is_some();
    Availability {
        available: typed || has_builtin,
        builtin: has_builtin,
        needs_client_id: !is_hosted(&account),
        host: Endpoints::for_account(&account).host,
        install_url: install_url(&account, builtin),
    }
}

/// Scheme, host and port of an http(s) URL, lowercased, default port
/// dropped: `https://GitHub.com:443/x` is `https://github.com`. `None` for
/// anything else, including a URL carrying credentials.
///
/// Written out rather than taken from a URL crate because the core has none of
/// its own, and every URL it compares is one bridgewatch built or one it is
/// about to refuse.
pub fn origin_of(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let authority = authority.to_ascii_lowercase();
    let default = if scheme == "https" { ":443" } else { ":80" };
    let authority = authority.strip_suffix(default).unwrap_or(&authority);
    Some(format!("{scheme}://{authority}"))
}

/// The host of an http(s) URL, for sentences.
pub fn host_of(url: &str) -> Option<String> {
    origin_of(url).and_then(|o| o.split_once("://").map(|(_, h)| h.to_string()))
}

/// The account as an OAuth token has to be sent: `Authorization: Bearer`.
///
/// ⛔ GitLab reads `PRIVATE-TOKEN` for personal, project and group access
/// tokens only; an OAuth access token there is a 401. A GitLab account's
/// header defaults to `PRIVATE-TOKEN`, so every client built for an `oauth`
/// account is built from this, and [`OAuthTransport`] replaces the header on
/// every request regardless.
pub fn bearer_account(account: &Account) -> Account {
    Account {
        header: crate::config::AuthHeader::AuthorizationBearer,
        ..account.clone()
    }
}

/// Now, in whole seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
