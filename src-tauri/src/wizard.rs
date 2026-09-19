//! The first-run setup wizard's commands: thin wrappers over
//! `bridgewatch_core::wizard`, which holds every decision.
//!
//! What lives HERE is only what the core cannot do: turning the wizard's
//! `Connection` into a client (resolving a token, or taking a pasted one), the
//! confirmation gate in front of that, and the write, which goes through the
//! same `admit` + compare-and-swap path as every other write to the file.
//!
//! ⛔ The client is built by `client::client_for`, the core's one factory, and
//! never by naming a provider's type here. This built a `GitLabClient`
//! outright, so a github account would have had `/api/v4/...` paths and a
//! `PRIVATE-TOKEN` header sent to api.github.com on the wizard's very first
//! step, and the 401 would have read as a bad token.
//!
//! ⛔ Two rules carried over from `guard`:
//! - A `command` token source RUNS A PROGRAM, and a keyring or environment
//!   source sent to a host no account uses exports that credential. The live
//!   steps (1, 2 and 4) resolve a token on every call, so a connection that
//!   would do either is answered with a `ConfirmRequest` first. Once
//!   confirmed, that exact (base_url, token source) pair is remembered for the
//!   life of the process, because the wizard calls four commands with it.
//! - A pasted token (`secret`) is used for the request and, on save, stored
//!   in bridgewatch's own keyring entry. It is never logged, never echoed and
//!   never in the file (the core refuses a result that contains one).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bridgewatch_core::client::{CiClient, RequestRing, ReqwestTransport, client_for};
use bridgewatch_core::config::{Account, Config, ProjectRef, Provider, TokenSource};
use bridgewatch_core::token::{self, Secret, SystemTokenProvider, TokenProvider};
use bridgewatch_core::wizard::{
    self, CliTokenDetection, FailureKind, Identity, MarkerSuggestions, ProjectListing,
    ResolvedProject, StepIssue, SystemKeyringProbe, WizardAnswers, WizardError, WizardFailure,
    WizardStep,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::config::{DiagnosticView, Validation};
use crate::guard::{ConfirmRequest, Confirmations};
use crate::state::AppState;

/// The wizard window's label (declared in tauri.conf.json).
pub const WIZARD: &str = "wizard";

/// The account name used for a connection that does not name one. It never
/// reaches the file; it only keys the guard's comparison.
const UNNAMED: &str = "(setup wizard)";

/// How to reach the instance for the live steps. Mirrors `Connection` in
/// src/components/wizard/api.ts.
#[derive(Debug, Clone, Deserialize)]
pub struct Connection {
    /// Which provider the account talks to. Absent is `gitlab`, so a caller
    /// that predates the key means what it always did.
    #[serde(default)]
    pub provider: Provider,
    /// Instance root.
    pub base_url: String,
    /// Where the token comes from.
    pub token: TokenSource,
    /// A pasted token, for `{ own: true }` before it has been stored.
    #[serde(default)]
    pub secret: Option<String>,
    /// The id of a `ConfirmRequest` the user accepted.
    #[serde(default)]
    pub confirm: Option<String>,
    /// The account name the answers use, so `{ own: true }` finds that
    /// account's stored token and the guard compares like with like.
    #[serde(default)]
    pub account: Option<String>,
}

/// Either the answer, or the question that has to be answered first.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Confirmable<T> {
    /// The command ran.
    Ready(T),
    /// Nothing ran; confirm and repeat the call with `confirm: confirm.id`.
    NeedsConfirm {
        /// What to ask.
        confirm: ConfirmRequest,
    },
}

/// `previewConfig`'s answer.
#[derive(Debug, Serialize)]
pub struct WizardPreview {
    /// The complete config.toml that a save would write.
    pub toml: String,
    /// Load warnings, located in `toml`.
    pub warnings: Vec<DiagnosticView>,
    /// True when an existing file is being edited.
    pub edited_existing: bool,
    /// The id of the primary watch already in the file, when the new watch was
    /// added as secondary because of it. The review step says so.
    pub secondary_because: Option<String>,
}

/// `save`'s answer: the shell's `Validation`, plus answers the core refused
/// and where the file went.
#[derive(Debug, Serialize)]
pub struct WizardSaveResult {
    /// The write's validation (`confirm` / `conflict` as for any write).
    #[serde(flatten)]
    pub validation: Validation,
    /// Per-step problems; the wizard jumps to the first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<StepIssue>,
    /// The file written, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Per-process wizard state, managed by Tauri.
#[derive(Default)]
pub struct WizardSession {
    confirmations: Confirmations,
    /// Connections the user has confirmed, as their gate key.
    approved: Mutex<HashSet<String>>,
}

impl WizardSession {
    /// Let a connection through, or say what must be confirmed first.
    ///
    /// A pasted secret needs no confirmation: the token source is not read.
    pub fn gate(&self, conn: &Connection, running: Option<&Config>) -> Result<(), ConfirmRequest> {
        if has_secret(conn) {
            return Ok(());
        }
        let changes = connection_changes(conn, running);
        if changes.is_empty() {
            return Ok(());
        }
        let key = gate_key(conn);
        let mut approved = self.approved.lock().unwrap_or_else(|e| e.into_inner());
        if approved.contains(&key) {
            return Ok(());
        }
        if self.confirmations.redeem(conn.confirm.as_deref(), &key) {
            approved.insert(key);
            return Ok(());
        }
        Err(self.confirmations.issue(&key, changes))
    }
}

fn has_secret(conn: &Connection) -> bool {
    conn.secret.as_deref().is_some_and(|s| !s.trim().is_empty())
}

/// What a confirmation is bound to: the host and the token source, exactly.
fn gate_key(conn: &Connection) -> String {
    serde_json::to_string(&(conn.base_url.trim(), &conn.token)).unwrap_or_default()
}

/// The guard's sentences for using this connection, measured against what
/// is running: the same rules a write of such an account would meet.
pub fn connection_changes(conn: &Connection, running: Option<&Config>) -> Vec<String> {
    let mut candidate = Config::default();
    candidate.accounts.insert(
        conn.account.clone().unwrap_or_else(|| UNNAMED.to_string()),
        account_for(conn),
    );
    crate::guard::sensitive_changes(running, &candidate)
}

/// ⛔ `Account::for_provider`, never `Account::default()` with a provider
/// assigned afterwards: the three provider-dependent defaults (`api_path`,
/// `header` and `base_url`) are taken when the struct is built, so the obvious
/// spelling yields a GitHub account carrying `/api/v4` and `PRIVATE-TOKEN`.
/// `base_url` is overridden here because the wizard always has one.
///
/// ⛔ A GitHub `api_path` follows the host, exactly as the file the wizard
/// writes will say (`wizard::github_api_path_for`): GitHub Enterprise Server
/// serves its API under `/api/v3`, and the provider default of "" would send
/// the wizard's own requests to the web UI's `/user` page instead.
fn account_for(conn: &Connection) -> Account {
    let base_url = conn.base_url.trim().to_string();
    let mut account = Account {
        token: conn.token.clone(),
        ..Account::for_provider(conn.provider)
    };
    if conn.provider == Provider::Github {
        account.api_path = wizard::github_api_path_for(&base_url);
    }
    account.base_url = base_url;
    account
}

fn failure(kind: FailureKind, message: impl Into<String>) -> WizardFailure {
    WizardFailure {
        kind,
        message: message.into(),
        issues: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn account_issue(field: &str, message: String) -> WizardFailure {
    WizardFailure {
        kind: FailureKind::Answers,
        message: message.clone(),
        issues: vec![StepIssue {
            step: WizardStep::Account,
            field: field.into(),
            message,
        }],
        diagnostics: Vec::new(),
    }
}

/// The token for a connection: the pasted one, or the source resolved.
///
/// Token failures are reported against the account step's `token` field,
/// with the core's own message (which never contains a token).
pub fn secret_for(
    conn: &Connection,
    provider: &dyn TokenProvider,
) -> Result<Secret, WizardFailure> {
    if let Some(pasted) = conn
        .secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(Secret::new(pasted));
    }
    let account = conn.account.as_deref().unwrap_or(UNNAMED);
    token::resolve(&conn.token, account, provider)
        .map_err(|e| account_issue("token", format!("could not read the token: {e}")))
}

/// Check the base URL before anything is sent to it.
pub fn check_base_url(base_url: &str) -> Result<(), WizardFailure> {
    match url::Url::parse(base_url.trim()) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host_str().is_some() => Ok(()),
        _ => Err(account_issue(
            "base_url",
            "the instance address must start with https:// (or http://)".into(),
        )),
    }
}

/// Gate, resolve and build a client. `Ok(Err(request))` means "confirm first".
async fn connect(
    app: &AppHandle,
    conn: &Connection,
) -> Result<Result<Arc<dyn CiClient>, ConfirmRequest>, WizardFailure> {
    check_base_url(&conn.base_url)?;
    let running = app.state::<Arc<AppState>>().config();
    if let Err(request) = app.state::<WizardSession>().gate(conn, running.as_ref()) {
        return Ok(Err(request));
    }
    // Off the async threads: a keyring read can raise a Keychain prompt, and
    // a command source runs a program.
    let resolving = conn.clone();
    let secret =
        tauri::async_runtime::spawn_blocking(move || secret_for(&resolving, &SystemTokenProvider))
            .await
            .map_err(|e| failure(FailureKind::Network, e.to_string()))??;
    let account = account_for(conn);
    let transport = ReqwestTransport::new(Duration::from_secs(account.timeout_secs))
        .map_err(|e| failure(FailureKind::Network, e.to_string()))?;
    client_for(&account, &secret, Arc::new(transport), RequestRing::new(16))
        .map(Ok)
        .map_err(|e| failure(FailureKind::Network, e.to_string()))
}

/// Every command rejects with the core's `WizardFailure` JSON,
/// `{kind, message, issues, diagnostics}`, diagnostics in the core's
/// `Diagnostic` shape (the wizard UI maps them itself).
fn fail(e: WizardError) -> WizardFailure {
    e.to_failure()
}

/// Is the provider CLI's keyring item present for this instance? Existence
/// only; the token is never read.
///
/// ⛔ The probe cannot return a value: `KeyringProbe::exists` answers a bool,
/// the macOS branch runs `security find-generic-password` WITHOUT `-w` or `-g`
/// (which prints attributes and never the password) and the Linux branch
/// discards `secret-tool`'s stdout. So no credential enters this process, and
/// nothing that reaches the IPC payload, a log line or a test message could
/// carry one even by mistake.
#[tauri::command]
pub async fn wizard_detect_cli_token(base_url: String, provider: Provider) -> CliTokenDetection {
    tauri::async_runtime::spawn_blocking(move || {
        wizard::detect_cli_token(&SystemKeyringProbe, provider, &base_url)
    })
    .await
    .unwrap_or_else(|e| CliTokenDetection::Unavailable {
        reason: e.to_string(),
    })
}

/// Step 1: who is this token?
#[tauri::command]
pub async fn wizard_test_connection(
    connection: Connection,
    app: AppHandle,
) -> Result<Confirmable<Identity>, WizardFailure> {
    let client = match connect(&app, &connection).await? {
        Ok(c) => c,
        Err(confirm) => return Ok(Confirmable::NeedsConfirm { confirm }),
    };
    wizard::test_connection(client.as_ref(), connection.provider)
        .await
        .map(Confirmable::Ready)
        .map_err(fail)
}

/// Step 2: the projects this token can pick from.
#[tauri::command]
pub async fn wizard_list_projects(
    connection: Connection,
    search: Option<String>,
    app: AppHandle,
) -> Result<Confirmable<ProjectListing>, WizardFailure> {
    let client = match connect(&app, &connection).await? {
        Ok(c) => c,
        Err(confirm) => return Ok(Confirmable::NeedsConfirm { confirm }),
    };
    // The token's kind decides whether listing is possible at all (a project
    // token cannot usefully list), so ask who it is first.
    let identity = wizard::test_connection(client.as_ref(), connection.provider)
        .await
        .map_err(fail)?;
    let search = search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    wizard::list_projects(
        client.as_ref(),
        connection.provider,
        &identity.token,
        search,
    )
    .await
    .map(Confirmable::Ready)
    .map_err(fail)
}

/// Step 2: resolve a typed id, path or pasted project URL.
#[tauri::command]
pub async fn wizard_resolve_project(
    connection: Connection,
    id_or_path: String,
    app: AppHandle,
) -> Result<Confirmable<ResolvedProject>, WizardFailure> {
    let client = match connect(&app, &connection).await? {
        Ok(c) => c,
        Err(confirm) => return Ok(Confirmable::NeedsConfirm { confirm }),
    };
    wizard::resolve_project(client.as_ref(), connection.provider, &id_or_path)
        .await
        .map(Confirmable::Ready)
        .map_err(fail)
}

/// Step 4: deploy-marker suggestions from the ref's latest pipeline.
#[tauri::command]
pub async fn wizard_suggest_deploy_markers(
    connection: Connection,
    project: ProjectRef,
    ref_name: String,
    workflow: Option<String>,
    app: AppHandle,
) -> Result<Confirmable<MarkerSuggestions>, WizardFailure> {
    let client = match connect(&app, &connection).await? {
        Ok(c) => c,
        Err(confirm) => return Ok(Confirmable::NeedsConfirm { confirm }),
    };
    wizard::suggest_deploy_markers(client.as_ref(), &project, &ref_name, workflow.as_deref())
        .await
        .map(Confirmable::Ready)
        .map_err(fail)
}

/// The text a save would write, given what is on disk. Pure.
pub fn preview(answers: &WizardAnswers, existing: &str) -> Result<WizardPreview, WizardFailure> {
    let existing = (!existing.trim().is_empty()).then_some(existing);
    let built = wizard::build_config(answers, existing).map_err(fail)?;
    let warnings = built
        .warnings
        .iter()
        .map(|d| DiagnosticView::new(d, &built.toml))
        .collect();
    Ok(WizardPreview {
        toml: built.toml,
        warnings,
        edited_existing: built.edited_existing,
        secondary_because: built.secondary_because,
    })
}

/// Step 6: the config.toml the answers produce. Writes nothing.
#[tauri::command]
pub fn wizard_preview_config(
    answers: WizardAnswers,
    app: AppHandle,
) -> Result<WizardPreview, WizardFailure> {
    let state = app.state::<Arc<AppState>>();
    let existing = crate::commands::read_or_empty(&state.config_path)
        .map_err(|e| failure(FailureKind::ExistingConfig, e.to_string()))?;
    preview(&answers, &existing)
}

/// Step 6: write it.
///
/// Order: build (the core refuses bad answers and any token in the text),
/// admit (validation + the Lo13/M11 confirmation, the same gate as every
/// write), store a pasted token, compare-and-swap the file, reload. A
/// `confirm` answer wrote nothing; a `conflict` answer wrote nothing because
/// the file changed since it was read.
#[tauri::command]
pub async fn wizard_save(
    answers: WizardAnswers,
    secret: Option<String>,
    confirm: Option<String>,
    app: AppHandle,
) -> Result<WizardSaveResult, WizardFailure> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let path = state.config_path.clone();
    let base = crate::commands::read_or_empty(&path)
        .map_err(|e| failure(FailureKind::ExistingConfig, e.to_string()))?;

    let built = match preview(&answers, &base) {
        Ok(p) => p,
        Err(f) if f.kind == FailureKind::Answers => {
            return Ok(WizardSaveResult {
                validation: Validation {
                    ok: false,
                    ..Validation::default()
                },
                issues: f.issues,
                path: None,
            });
        }
        Err(f) => return Err(f),
    };

    let validation = match crate::commands::admit(
        &path,
        &base,
        &built.toml,
        state.config().as_ref(),
        &state.confirmations,
        confirm.as_deref(),
    ) {
        Ok(v) => v,
        Err(refused) => {
            return Ok(WizardSaveResult {
                validation: refused,
                issues: Vec::new(),
                path: None,
            });
        }
    };

    // Stored BEFORE the write, so the reload the write triggers finds it.
    if let (TokenSource::Own(true), Some(pasted)) = (&answers.token, secret.as_deref()) {
        let pasted = pasted.trim().to_string();
        if !pasted.is_empty() {
            let account = answers.account.clone();
            tauri::async_runtime::spawn_blocking(move || {
                token::set_own_token(&account, &Secret::new(pasted), &SystemTokenProvider)
            })
            .await
            .map_err(|e| failure(FailureKind::Network, e.to_string()))?
            .map_err(|e| account_issue("token", format!("could not store the token: {e}")))?;
        }
    }

    let validation = crate::commands::write_admitted(&app, &state, &base, &built.toml, validation);
    let written = validation.ok;
    let mut result = WizardSaveResult {
        validation,
        issues: Vec::new(),
        path: written.then(|| path.display().to_string()),
    };
    if written
        && let Some(wanted) = answers.launch_at_login
        && let Err(e) = crate::commands::set_login_item(&app, wanted)
    {
        result.validation.diagnostics.push(DiagnosticView {
            severity: "warning".into(),
            path: "ui.launch_at_login".into(),
            message: format!("saved, but the login item could not be changed: {e}"),
            line: None,
            col: None,
        });
    }
    if written {
        hide_wizard(&app);
    }
    Ok(result)
}

/// "I'll edit config.toml": leave the wizard, writing nothing, and open
/// Settings on the text editor. On a first launch the file does not exist;
/// the text tab reads it as empty and its Save creates it, as an explicit act.
#[tauri::command]
pub fn wizard_skip(app: AppHandle) {
    hide_wizard(&app);
    crate::windows::show_settings(&app, Some("text"));
}

/// Open the wizard window (tray menu, Settings).
#[tauri::command]
pub fn open_wizard(app: AppHandle) {
    show_wizard(&app);
}

/// Show and focus the wizard window.
pub fn show_wizard(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WIZARD) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_wizard(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WIZARD) {
        let _ = window.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridgewatch_core::token::TokenError;
    use std::path::Path;

    fn conn(base: &str, token: TokenSource) -> Connection {
        Connection {
            provider: Provider::Gitlab,
            base_url: base.into(),
            token,
            secret: None,
            confirm: None,
            account: None,
        }
    }

    fn running(raw: &str) -> Config {
        bridgewatch_core::config::parse_str(raw, Path::new("x.toml"))
            .unwrap()
            .config
    }

    #[test]
    fn a_command_source_runs_only_after_confirmation_and_then_stays_approved() {
        // Lo13 for the wizard: step 1 resolves the token, and a command
        // source runs a program. No call may run it unconfirmed.
        let s = WizardSession::default();
        let mut c = conn(
            "https://gitlab.com",
            TokenSource::Command(vec!["pass".into(), "gitlab".into()]),
        );
        let request = s.gate(&c, None).expect_err("ran unconfirmed");
        assert!(request.changes[0].contains("`pass gitlab`"), "{request:?}");
        // A wrong id is refused (and spends the request).
        c.confirm = Some("nope".into());
        assert!(s.gate(&c, None).is_err());
        let request = s.gate(&c, None).unwrap_err();
        c.confirm = Some(request.id);
        assert!(s.gate(&c, None).is_ok());
        // The four live steps reuse the connection: no second question.
        c.confirm = None;
        assert!(s.gate(&c, None).is_ok());
        // A different program is a different question.
        c.token = TokenSource::Command(vec!["sh".into()]);
        assert!(s.gate(&c, None).is_err());
    }

    #[test]
    fn a_stored_credential_is_sent_to_a_new_host_only_after_confirmation() {
        // M11 for the wizard: a keyring entry shipped to a host no account uses.
        let s = WizardSession::default();
        let glab = TokenSource::Keyring {
            service: "glab:gitlab.com:token".into(),
            user: String::new(),
        };
        let cfg =
            running("[accounts.gl]\nbase_url = \"https://gitlab.com\"\ntoken = { own = true }\n");
        assert!(
            s.gate(&conn("https://gitlab.com", glab.clone()), Some(&cfg))
                .is_ok()
        );
        assert!(
            s.gate(&conn("https://evil.example", glab.clone()), Some(&cfg))
                .is_err()
        );
        // A pasted token reads no stored credential, so nothing to confirm.
        let mut pasted = conn("https://evil.example", glab);
        pasted.secret = Some("typed".into());
        assert!(s.gate(&pasted, Some(&cfg)).is_ok());
        // `own` is the account's own entry: exempt, as in `guard`.
        assert!(
            s.gate(&conn("https://new.example", TokenSource::Own(true)), None)
                .is_ok()
        );
    }

    struct NoStore;
    impl TokenProvider for NoStore {
        fn keyring_get(&self, _: &str, _: &str) -> Result<String, TokenError> {
            Err(TokenError::NotConfigured("x".into()))
        }
        fn keyring_set(&self, _: &str, _: &str, _: &str) -> Result<(), TokenError> {
            unreachable!()
        }
        fn keyring_delete(&self, _: &str, _: &str) -> Result<(), TokenError> {
            unreachable!()
        }
        fn env(&self, name: &str) -> Option<String> {
            (name == "GL").then(|| "from-env".into())
        }
        fn run(&self, _: &[String]) -> Result<String, TokenError> {
            unreachable!("a command source ran")
        }
    }

    #[test]
    fn a_pasted_token_wins_and_a_missing_one_is_a_token_issue() {
        let mut c = conn("https://gitlab.com", TokenSource::Own(true));
        c.secret = Some("  pasted  ".into());
        assert_eq!(secret_for(&c, &NoStore).unwrap().expose(), "pasted");
        // Even over a command source: the pasted token is what was asked for.
        c.token = TokenSource::Command(vec!["x".into()]);
        assert_eq!(secret_for(&c, &NoStore).unwrap().expose(), "pasted");

        let env = conn("https://gitlab.com", TokenSource::Env("GL".into()));
        assert_eq!(secret_for(&env, &NoStore).unwrap().expose(), "from-env");

        let f = secret_for(
            &conn("https://gitlab.com", TokenSource::Own(true)),
            &NoStore,
        )
        .expect_err("no token");
        assert_eq!(f.kind, FailureKind::Answers);
        assert_eq!(f.issues[0].field, "token");
        assert_eq!(f.issues[0].step, WizardStep::Account);
    }

    #[test]
    fn only_an_http_or_https_instance_is_contacted() {
        assert!(check_base_url("https://gitlab.com").is_ok());
        assert!(check_base_url("http://gitlab.internal:8080").is_ok());
        for bad in ["gitlab.com", "file:///etc", "ftp://x", ""] {
            let f = check_base_url(bad).expect_err(bad);
            assert_eq!(f.issues[0].field, "base_url");
        }
    }

    fn answers() -> WizardAnswers {
        WizardAnswers {
            account: "gitlab".into(),
            base_url: "https://gitlab.com".into(),
            token: TokenSource::Own(true),
            project: Some(ProjectRef::Id(42)),
            watch_id: "app-main".into(),
            ref_name: "main".into(),
            deploy_markers: vec!["deploy".into()],
            ..WizardAnswers::default()
        }
    }

    #[test]
    fn the_preview_is_a_config_the_shell_would_admit() {
        let p = preview(&answers(), "").unwrap();
        assert!(!p.edited_existing);
        assert!(
            Validation::of(&p.toml, Path::new("x.toml")).ok,
            "{}",
            p.toml
        );
        // The same text passes the write gate with no question (own token,
        // no command): a first-run save is one click.
        let c = Confirmations::default();
        assert!(crate::commands::admit(Path::new("x.toml"), "", &p.toml, None, &c, None).is_ok());
    }

    /// The Tauri wizard hands `preview` the file on disk, so a second run over
    /// a config with a primary adds a secondary watch and the preview says
    /// which primary it deferred to; the `primary` answer overrides it.
    #[test]
    fn a_second_wizard_run_over_a_primary_adds_a_secondary_and_says_so() {
        let first = preview(&answers(), "").unwrap().toml;
        let mut second = answers();
        second.project = Some(ProjectRef::Id(7));
        second.watch_id = "other-main".into();

        let p = preview(&second, &first).unwrap();
        assert_eq!(p.secondary_because.as_deref(), Some("app-main"));
        assert!(
            p.warnings
                .iter()
                .all(|w| !w.message.contains("are primary"))
        );

        second.primary = true;
        let p = preview(&second, &first).unwrap();
        assert_eq!(p.secondary_because, None);
        assert!(
            p.warnings
                .iter()
                .any(|w| w.message.contains("2 watches are primary"))
        );
    }

    #[test]
    fn bad_answers_come_back_as_step_issues_not_a_write() {
        let mut a = answers();
        a.watch_id = String::new();
        let f = preview(&a, "").expect_err("built from bad answers");
        assert_eq!(f.kind, FailureKind::Answers);
        assert!(
            f.issues.iter().any(|i| i.field == "watch_id"),
            "{:?}",
            f.issues
        );
    }

    #[test]
    fn a_command_source_in_the_answers_is_parked_at_the_write_gate() {
        // The connection's confirmation does not carry over to the FILE: a
        // command source written there runs on every reload, so the save asks
        // again, through the same gate as every other write.
        let mut a = answers();
        a.token = TokenSource::Command(vec!["pass".into(), "gl".into()]);
        let p = preview(&a, "").unwrap();
        let c = Confirmations::default();
        let refused = crate::commands::admit(Path::new("x.toml"), "", &p.toml, None, &c, None)
            .expect_err("written unconfirmed");
        assert!(refused.confirm.is_some());
    }

    /// Records the URL, the path and the header NAMES of every request. ⛔ No
    /// value is ever kept: the token is the one thing a test fixture must not be
    /// able to print, and a name is what a 401 investigation actually needs.
    #[derive(Debug, Default)]
    struct SpyTransport {
        seen: Mutex<Vec<(String, String, Vec<String>)>>,
    }

    // Written out in the form `#[async_trait]` expands to, because this crate
    // does not depend on `async-trait` and adding it for one test double would
    // change Cargo.lock.
    impl bridgewatch_core::client::Transport for SpyTransport {
        fn execute<'life0, 'async_trait>(
            &'life0 self,
            request: bridgewatch_core::client::HttpRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            bridgewatch_core::client::HttpResponse,
                            bridgewatch_core::client::ClientError,
                        >,
                    > + Send
                    + 'async_trait,
            >,
        >
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            self.seen.lock().unwrap().push((
                request.url.clone(),
                request.path.clone(),
                request.headers.iter().map(|(n, _)| n.clone()).collect(),
            ));
            Box::pin(async move {
                Ok(bridgewatch_core::client::HttpResponse {
                    status: 200,
                    body: r#"{"id":1,"login":"octo","type":"User"}"#.to_string(),
                    next_page: None,
                    ratelimit_remaining: None,
                    ratelimit_reset: None,
                    retry_after: None,
                    etag: None,
                    link: None,
                    oauth_scopes: None,
                })
            })
        }
    }

    /// ⛔ The composition `connect` performs, minus the `AppHandle` it cannot
    /// have here: a `Connection` becomes an `Account`, and the account becomes
    /// a client through the core's ONE factory. It built a `GitLabClient`
    /// outright, so this same connection would have sent `/api/v4/user` with a
    /// `PRIVATE-TOKEN` header to api.github.com, and the 401 would have read as
    /// a bad token rather than as the wrong client.
    #[tokio::test]
    async fn a_github_connection_builds_a_github_client() {
        let conn = Connection {
            provider: Provider::Github,
            base_url: "https://api.github.com".into(),
            token: TokenSource::Own(true),
            secret: None,
            confirm: None,
            account: None,
        };
        let account = account_for(&conn);
        assert_eq!(account.provider, Provider::Github);
        assert_eq!(
            account.api_path, "",
            "GitHub's paths start /repos, unprefixed"
        );
        assert_eq!(
            account.header,
            bridgewatch_core::config::AuthHeader::AuthorizationBearer,
            "GitHub reads a credential from Authorization and nowhere else"
        );

        let spy = Arc::new(SpyTransport::default());
        let client = bridgewatch_core::client::client_for(
            &account,
            &Secret::new("not-a-real-token"),
            spy.clone(),
            bridgewatch_core::client::RequestRing::new(4),
        )
        .unwrap();
        let identity = bridgewatch_core::wizard::test_connection(client.as_ref(), conn.provider)
            .await
            .unwrap();
        assert_eq!(identity.username, "octo");

        let seen = spy.seen.lock().unwrap().clone();
        assert!(!seen.is_empty(), "nothing was sent");
        for (url, path, headers) in &seen {
            assert!(
                url.starts_with("https://api.github.com/"),
                "the request left for another host: {url}"
            );
            assert!(!url.contains("/api/v4"), "a GitLab API path: {url}");
            assert!(!path.contains("/api/v4"), "a GitLab API path: {path}");
            assert!(headers.iter().any(|h| h == "Authorization"), "{headers:?}");
            assert!(
                !headers.iter().any(|h| h == "PRIVATE-TOKEN"),
                "GitLab's header reached GitHub: {headers:?}"
            );
        }
        assert_eq!(seen[0].1, "/user");

        // GitHub Enterprise Server: the API is a path on the web host, and the
        // wizard's own requests must use it, as the written file will.
        let ghes = account_for(&Connection {
            base_url: "https://ghe.acme.com".into(),
            ..conn.clone()
        });
        assert_eq!(ghes.base_url, "https://ghe.acme.com");
        assert_eq!(ghes.api_path, "/api/v3");

        // ...and a gitlab connection is unchanged.
        let gl = conn_gitlab();
        let account = account_for(&gl);
        assert_eq!(account.provider, Provider::Gitlab);
        assert_eq!(account.api_path, "/api/v4");
        assert_eq!(
            account.header,
            bridgewatch_core::config::AuthHeader::PrivateToken
        );
    }

    fn conn_gitlab() -> Connection {
        conn("https://gitlab.com", TokenSource::Own(true))
    }

    #[test]
    fn the_wire_shapes_are_the_ones_api_ts_reads() {
        let ready: Confirmable<u32> = Confirmable::Ready(7);
        assert_eq!(serde_json::to_value(&ready).unwrap(), serde_json::json!(7));
        let ask: Confirmable<u32> = Confirmable::NeedsConfirm {
            confirm: ConfirmRequest {
                id: "i".into(),
                changes: vec!["c".into()],
            },
        };
        assert_eq!(
            serde_json::to_value(&ask).unwrap(),
            serde_json::json!({"confirm": {"id": "i", "changes": ["c"]}})
        );
        let saved = WizardSaveResult {
            validation: Validation {
                ok: true,
                ..Validation::default()
            },
            issues: Vec::new(),
            path: Some("/c.toml".into()),
        };
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            serde_json::json!({"ok": true, "diagnostics": [], "path": "/c.toml"})
        );
        let c: Connection = serde_json::from_value(serde_json::json!({
            "base_url": "https://gitlab.com", "token": {"own": true}
        }))
        .unwrap();
        assert!(c.secret.is_none() && c.confirm.is_none() && c.account.is_none());
        assert_eq!(c.provider, Provider::Gitlab, "absent is gitlab");
        let c: Connection = serde_json::from_value(serde_json::json!({
            "provider": "github", "base_url": "https://api.github.com", "token": {"own": true}
        }))
        .unwrap();
        assert_eq!(c.provider, Provider::Github);
    }

    /// A stand-in for the OS credential store. It answers presence only, which
    /// is all `KeyringProbe` can express, and records what it was asked.
    #[derive(Default)]
    struct FakeProbe {
        present: Vec<(String, String)>,
        asked: Mutex<Vec<(String, String)>>,
    }

    impl wizard::KeyringProbe for FakeProbe {
        fn exists(&self, service: &str, user: &str) -> Result<bool, String> {
            self.asked
                .lock()
                .unwrap()
                .push((service.to_string(), user.to_string()));
            Ok(self.present.iter().any(|(s, u)| s == service && u == user))
        }
    }

    /// What `wizard_detect_cli_token` hands the webview, per provider: the
    /// source to write, never a value. The gh preset is the EMPTY-user item,
    /// gh's active-account slot, looked up at the WEB host.
    #[test]
    fn detection_reaches_the_webview_as_a_source_and_never_a_value() {
        let probe = FakeProbe {
            present: vec![
                ("glab:gitlab.com:token".into(), String::new()),
                ("gh:github.com".into(), String::new()),
            ],
            ..FakeProbe::default()
        };
        let gitlab = wizard::detect_cli_token(&probe, Provider::Gitlab, "https://gitlab.com");
        assert_eq!(
            serde_json::to_value(&gitlab).unwrap(),
            serde_json::json!({
                "status": "found",
                "source": {"keyring": {"service": "glab:gitlab.com:token", "user": ""}}
            })
        );
        let github = wizard::detect_cli_token(&probe, Provider::Github, "https://api.github.com");
        assert_eq!(
            serde_json::to_value(&github).unwrap(),
            serde_json::json!({
                "status": "found",
                "source": {"keyring": {"service": "gh:github.com", "user": ""}}
            })
        );
        assert_eq!(
            probe.asked.lock().unwrap().as_slice(),
            [
                ("glab:gitlab.com:token".to_string(), String::new()),
                ("gh:github.com".to_string(), String::new()),
            ]
        );
        let missing = wizard::detect_cli_token(&probe, Provider::Github, "https://ghe.acme.com");
        assert_eq!(
            serde_json::to_value(&missing).unwrap(),
            serde_json::json!({"status": "not_found", "service": "gh:ghe.acme.com"})
        );
    }
}
