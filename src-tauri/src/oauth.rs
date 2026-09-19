//! "Sign in with GitHub" / "Sign in with GitLab": the shell's half.
//!
//! Every decision is the core's (`bridgewatch_core::oauth`). What lives here
//! is what only the shell can do: keep a sign-in running in the background
//! while the webview shows its code, tell the webview how it is going, open the
//! verification page through the one door to the browser (`links`), and store
//! the result in the OS credential store.
//!
//! ⛔ No token, device code or refresh token crosses IPC in either direction.
//! The webview sees the user code (which it must show), the verification page,
//! and afterwards who signed in and until when.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bridgewatch_core::client::{ReqwestTransport, Transport};
use bridgewatch_core::config::{Account, OAuthSource, Provider};
use bridgewatch_core::oauth::{
    self, Availability, BuiltinClients, OAuthError, Progress, SignInStatus, SignedIn,
};
use bridgewatch_core::poll::PollNow;
use bridgewatch_core::token::SystemTokenProvider;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::state::AppState;

/// The event a sign-in's progress is emitted as, to the window that started
/// it.
pub const PROGRESS_EVENT: &str = "oauth-progress";

/// What to sign in to, and under which account name to keep it.
#[derive(Debug, Clone, Deserialize)]
pub struct SignInRequest {
    /// The `[accounts.*]` key the sign-in is stored for.
    pub account: String,
    /// Which provider. Absent is `gitlab`, as everywhere else.
    #[serde(default)]
    pub provider: Provider,
    /// Instance root, as the account's `base_url`.
    pub base_url: String,
    /// A client id of the user's own. Absent: the host's built-in one.
    #[serde(default)]
    pub client_id: Option<String>,
}

/// A started sign-in, for the webview to show.
#[derive(Debug, Clone, Serialize)]
pub struct StartedSignIn {
    /// Pass back to wait, cancel and open.
    pub id: String,
    /// What the user types. Shown large; never logged.
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    /// How long the code lasts, seconds.
    pub expires_in: u64,
    /// The host, for the button: `github.com`.
    pub host: String,
}

/// A sign-in that did not finish, in a shape the webview can choose words by.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SignInFailure {
    /// `expired`, `denied`, `cancelled`, `no_client_id`, `sign_in_again` or
    /// `error`.
    pub kind: String,
    /// One sentence. Never a token.
    pub message: String,
}

impl From<&OAuthError> for SignInFailure {
    fn from(e: &OAuthError) -> Self {
        Self {
            kind: e.kind().to_string(),
            message: e.to_string(),
        }
    }
}

fn failure(kind: &str, message: impl Into<String>) -> SignInFailure {
    SignInFailure {
        kind: kind.to_string(),
        message: message.into(),
    }
}

/// The progress event's payload.
#[derive(Debug, Clone, Serialize)]
struct ProgressEvent {
    id: String,
    #[serde(flatten)]
    progress: Progress,
}

type Outcome = Option<Result<SignedIn, SignInFailure>>;

struct Flow {
    /// The account being signed in, whose host the verification page is
    /// trusted on for one open.
    account: Account,
    verification_uri: String,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    result: tokio::sync::watch::Receiver<Outcome>,
}

/// The sign-ins in progress, managed by Tauri.
#[derive(Default)]
pub struct SignIns {
    flows: Mutex<HashMap<String, Flow>>,
}

impl SignIns {
    fn with_flows<R>(&self, f: impl FnOnce(&mut HashMap<String, Flow>) -> R) -> R {
        f(&mut self.flows.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A fresh, unguessable id for a flow.
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:016x}", hasher.finish())
}

/// The account a request signs in for: the running configuration's, when it
/// has one of that name on the same host (so its `api_path` is used), else
/// one built from the request with the provider's defaults.
///
/// ⛔ `Account::for_provider`, as in `wizard::account_for`: taking
/// `Account::default()` and setting the provider afterwards leaves GitLab's
/// `/api/v4` on a GitHub account.
pub fn account_for(request: &SignInRequest, running: Option<&bridgewatch_core::Config>) -> Account {
    let base_url = request.base_url.trim().trim_end_matches('/').to_string();
    if let Some(existing) = running.and_then(|c| c.accounts.get(&request.account))
        && existing.provider == request.provider
        && existing.base_url.trim().trim_end_matches('/') == base_url
    {
        return existing.clone();
    }
    let mut account = Account::for_provider(request.provider);
    if request.provider == Provider::Github {
        account.api_path = bridgewatch_core::wizard::github_api_path_for(&base_url);
    }
    account.base_url = base_url;
    account
}

fn source_of(request: &SignInRequest) -> OAuthSource {
    OAuthSource {
        client_id: request
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string),
    }
}

fn transport(account: &Account) -> Result<Arc<dyn Transport>, SignInFailure> {
    ReqwestTransport::new(Duration::from_secs(account.timeout_secs))
        .map(|t| Arc::new(t) as Arc<dyn Transport>)
        .map_err(|e| failure("error", e.to_string()))
}

/// Tell the poller something changed: a sign-in or a sign-out is a new answer
/// for that account's watches, and waiting out a backed-off interval would
/// leave "sign in again" on screen after the user did.
fn nudge(app: &AppHandle) {
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        state.request_poll(PollNow::Manual);
    }
}

/// Is "Sign in" available for this instance? Pure; reads no credential.
#[tauri::command]
pub fn oauth_availability(
    provider: Provider,
    base_url: String,
    client_id: Option<String>,
) -> Availability {
    oauth::availability(
        provider,
        &base_url,
        client_id.as_deref(),
        &BuiltinClients::shipped(),
    )
}

/// Start a sign-in: ask for a code, and keep polling in the background.
#[tauri::command]
pub async fn oauth_start(
    request: SignInRequest,
    window: WebviewWindow,
    app: AppHandle,
) -> Result<StartedSignIn, SignInFailure> {
    if request.account.trim().is_empty() {
        return Err(failure("error", "name the account before signing in"));
    }
    crate::wizard::check_base_url(&request.base_url).map_err(|f| failure("error", f.message))?;
    let running = app.state::<Arc<AppState>>().config();
    let account = account_for(&request, running.as_ref());
    let source = source_of(&request);
    let client_id = oauth::client_id_for(&account, &source, &BuiltinClients::shipped())
        .map_err(|e| SignInFailure::from(&e))?;
    let endpoints = oauth::Endpoints::for_account(&account);
    let transport = transport(&account)?;
    let device = oauth::start(transport.as_ref(), &endpoints, &client_id)
        .await
        .map_err(|e| SignInFailure::from(&e))?;

    let id = new_id();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let (result_tx, result_rx) = tokio::sync::watch::channel::<Outcome>(None);
    let started = StartedSignIn {
        id: id.clone(),
        user_code: device.user_code.clone(),
        verification_uri: device.verification_uri.clone(),
        expires_in: device.expires_in,
        host: endpoints.host.clone(),
    };
    app.state::<SignIns>().with_flows(|flows| {
        flows.insert(
            id.clone(),
            Flow {
                account: account.clone(),
                verification_uri: device.verification_uri.clone(),
                cancel: Some(cancel_tx),
                result: result_rx,
            },
        )
    });

    let label = window.label().to_string();
    let account_name = request.account.trim().to_string();
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let emit_app = task_app.clone();
        let emit_id = id.clone();
        let mut progress = move |p: Progress| {
            let _ = emit_app.emit_to(
                label.as_str(),
                PROGRESS_EVENT,
                ProgressEvent {
                    id: emit_id.clone(),
                    progress: p,
                },
            );
        };
        let cancelled = async {
            let _ = cancel_rx.await;
        };
        let outcome = match oauth::wait_for_token(
            transport.as_ref(),
            &endpoints,
            &client_id,
            &device,
            tokio::time::sleep,
            cancelled,
            &mut progress,
        )
        .await
        {
            Ok(reply) => {
                let store = SystemTokenProvider;
                oauth::complete_sign_in(
                    &account_name,
                    &account,
                    &client_id,
                    reply,
                    transport.clone(),
                    &store,
                    oauth::now(),
                )
                .await
                .map(|(_, signed_in)| signed_in)
            }
            Err(e) => Err(e),
        };
        let outcome = match outcome {
            Ok(signed_in) => {
                nudge(&task_app);
                Ok(signed_in)
            }
            Err(e) => {
                tracing::info!(account = %account_name, reason = e.kind(), "sign-in did not finish");
                Err(SignInFailure::from(&e))
            }
        };
        let _ = result_tx.send(Some(outcome));
    });

    Ok(started)
}

/// Wait for a started sign-in to finish.
#[tauri::command]
pub async fn oauth_wait(id: String, app: AppHandle) -> Result<SignedIn, SignInFailure> {
    let Some(mut result) = app
        .state::<SignIns>()
        .with_flows(|flows| flows.get(&id).map(|flow| flow.result.clone()))
    else {
        return Err(failure("error", "that sign-in is not running"));
    };
    let outcome = loop {
        if let Some(outcome) = result.borrow().clone() {
            break outcome;
        }
        if result.changed().await.is_err() {
            break Err(failure("error", "the sign-in stopped unexpectedly"));
        }
    };
    app.state::<SignIns>().with_flows(|flows| flows.remove(&id));
    outcome
}

/// Stop waiting for a sign-in. Stopping one that already finished is fine.
#[tauri::command]
pub fn oauth_cancel(id: String, app: AppHandle) {
    let cancel = app
        .state::<SignIns>()
        .with_flows(|flows| flows.get_mut(&id).and_then(|flow| flow.cancel.take()));
    if let Some(cancel) = cancel {
        let _ = cancel.send(());
    }
}

/// Open a sign-in's verification page, trusting its account's host for this
/// one open.
#[tauri::command]
pub fn oauth_open_verification(id: String, app: AppHandle) -> Result<(), String> {
    let (uri, account) = app
        .state::<SignIns>()
        .with_flows(|flows| {
            flows
                .get(&id)
                .map(|flow| (flow.verification_uri.clone(), flow.account.clone()))
        })
        .ok_or("that sign-in is not running")?;
    let config = app.state::<Arc<AppState>>().config();
    crate::links::open_trusting(&app, &uri, config.as_ref(), &[account])
}

/// Open the page that installs bridgewatch's GitHub App, for a github.com
/// account whose build knows the app.
#[tauri::command]
pub fn oauth_open_install(
    provider: Provider,
    base_url: String,
    app: AppHandle,
) -> Result<(), String> {
    let account = Account {
        base_url: base_url.trim().to_string(),
        ..Account::for_provider(provider)
    };
    let url = oauth::install_url(&account, &BuiltinClients::shipped())
        .ok_or("this build does not know where to install the app")?;
    let config = app.state::<Arc<AppState>>().config();
    crate::links::open_trusting(&app, &url, config.as_ref(), &[account])
}

/// Who an account is signed in as, and until when. Never a token; `None` when
/// it is not signed in.
#[tauri::command]
pub async fn oauth_status(account: String) -> Result<Option<SignInStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        oauth::store::load(&account, &SystemTokenProvider).map(|s| s.map(|set| set.status()))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Forget an account's sign-in. The provider still lists bridgewatch as an
/// authorised application until the user revokes it there; the tokens are
/// gone from this machine.
#[tauri::command]
pub async fn oauth_sign_out(account: String, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        oauth::store::delete(&account, &SystemTokenProvider)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    nudge(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(provider: Provider, base_url: &str) -> SignInRequest {
        SignInRequest {
            account: "a".into(),
            provider,
            base_url: base_url.into(),
            client_id: None,
        }
    }

    /// A GitHub sign-in's account is a GitHub account from the start, so its
    /// `/user` request after the sign-in goes to GitHub's path shape.
    #[test]
    fn a_github_sign_in_builds_a_github_account() {
        let account = account_for(&request(Provider::Github, "https://api.github.com"), None);
        assert_eq!(account.provider, Provider::Github);
        assert_eq!(account.api_path, "");
        let ghes = account_for(&request(Provider::Github, "https://ghe.acme.com/"), None);
        assert_eq!(ghes.api_path, "/api/v3");
        assert_eq!(ghes.base_url, "https://ghe.acme.com");
    }

    /// The running file's account is used when it IS this account, so a
    /// custom `api_path` survives; one on another host is not.
    #[test]
    fn the_configured_account_is_used_only_on_its_own_host() {
        let raw = "[accounts.a]\nbase_url = \"https://gitlab.example.com\"\napi_path = \"/proxy/api/v4\"\n";
        let config = bridgewatch_core::config::parse_str(raw, std::path::Path::new("x.toml"))
            .unwrap()
            .config;
        let same = account_for(
            &request(Provider::Gitlab, "https://gitlab.example.com"),
            Some(&config),
        );
        assert_eq!(same.api_path, "/proxy/api/v4");
        let other = account_for(
            &request(Provider::Gitlab, "https://gitlab.com"),
            Some(&config),
        );
        assert_eq!(other.api_path, "/api/v4");
    }

    /// A blank typed client id is no client id, so the built-in one (or the
    /// "register an application" sentence) decides.
    #[test]
    fn a_blank_client_id_is_none() {
        let mut r = request(Provider::Gitlab, "https://gitlab.com");
        r.client_id = Some("  ".into());
        assert_eq!(source_of(&r), OAuthSource { client_id: None });
        r.client_id = Some(" abc ".into());
        assert_eq!(source_of(&r).client_id.as_deref(), Some("abc"));
    }

    #[test]
    fn a_failure_carries_the_core_kind_and_no_more() {
        let f = SignInFailure::from(&OAuthError::Expired);
        assert_eq!(f.kind, "expired");
        let f = SignInFailure::from(&OAuthError::Denied {
            host: "github.com".into(),
        });
        assert_eq!(f.kind, "denied");
        assert!(f.message.contains("github.com"), "{}", f.message);
    }
}
