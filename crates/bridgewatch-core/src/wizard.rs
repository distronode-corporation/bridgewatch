//! The first-run setup wizard's logic, with no UI in it.
//!
//! Every step of the wizard is a function here, so the shell only renders and
//! the behaviour is tested against a scripted transport:
//!
//! 1. **Account.** [`test_connection`] names who a token authenticates as and
//!    what kind of token it is; [`detect_glab_token`] offers `glab`'s keyring
//!    item (existence only, never the value).
//! 2. **Project.** [`list_projects`], or for a token that cannot list,
//!    [`ProjectListing::TypeIdOrPath`]; [`resolve_project`] turns what was
//!    typed into an id, a path and a default branch.
//! 3. **Deploy detection.** [`suggest_deploy_markers`] ranks the latest
//!    pipeline's job names, parent and children, by how deploy-like they are.
//! 4. **Review.** [`build_config`] turns the answers into `config.toml`
//!    through the same [`ConfigEditor`] the settings window uses, editing an
//!    existing file in place rather than replacing it, and refuses to produce
//!    text that does not load or that carries a token.
//!
//! Nothing here writes a file or reads a credential: the shell shows the text
//! [`build_config`] returns and writes it only when the user says so, which is
//! what makes "skip" write nothing.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client::{ClientError, GitLabClient, ListQuery};
use crate::config::edit::{ConfigEditor, Edit, EditError, EditValue, quote_path_segment};
use crate::config::{
    self, Config, Diagnostic, KNOWN_PIPELINE_SOURCES, Pattern, ProjectRef, RefMatcher, Role,
    Severity, TokenSource, Watch,
};
use crate::model::{Job, Pipeline, User};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Which wizard step an answer belongs to, so the UI can send the user back to
/// the right page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardStep {
    /// Instance URL and token.
    Account,
    /// Which project.
    Project,
    /// Branch, sources and the optional secondary watches.
    Watch,
    /// Deploy markers.
    Deploy,
    /// Notifications, launch at login, poll speed.
    Preferences,
}

/// One answer that cannot be used as given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepIssue {
    /// The step to go back to.
    pub step: WizardStep,
    /// The field on that step.
    pub field: String,
    /// What is wrong and what to do.
    pub message: String,
}

/// Everything a wizard function can fail with.
#[derive(Debug, thiserror::Error)]
pub enum WizardError {
    /// 401 or 403. The message names the likely cause rather than the status.
    #[error(
        "GitLab refused the token ({status}). Check that it has not expired or been revoked, \
         and that it has the read_api scope"
    )]
    Unauthorized {
        /// The HTTP status.
        status: u16,
    },
    /// 404, which GitLab also answers when the token cannot see the thing.
    #[error("{what} was not found, or this token cannot see it")]
    NotFound {
        /// What was looked for, in words.
        what: String,
    },
    /// Any other client failure: network, rate limit, decode.
    #[error(transparent)]
    Client(ClientError),
    /// A typed value that cannot be used.
    #[error("{}", .0.message)]
    Input(StepIssue),
    /// The answers as a whole do not validate; every issue is listed.
    #[error("{} answer(s) need fixing: {}", .0.len(), .0.iter().map(|i| i.message.as_str()).collect::<Vec<_>>().join("; "))]
    Answers(Vec<StepIssue>),
    /// The existing config could not be edited (usually: it is not TOML).
    #[error(transparent)]
    Edit(#[from] EditError),
    /// The text the wizard would write does not load. Nothing is written.
    #[error("the resulting config.toml would not load: {}", .0.iter().filter(|d| d.severity == Severity::Error).map(|d| format!("{}: {}", d.path, d.message)).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Diagnostic>),
    /// The resulting text contains something shaped like a GitLab token.
    /// bridgewatch never writes one into a config file.
    #[error(
        "the resulting config.toml contains something shaped like a GitLab token ({prefix}…); \
         a token belongs in the keyring, an environment variable or a command, never in the file"
    )]
    TokenInConfig {
        /// The token prefix that was found, e.g. `glpat-`. Never the token.
        prefix: &'static str,
    },
}

impl WizardError {
    /// The answers that need fixing, each naming its step, for errors that are
    /// about answers ([`WizardError::Input`] and [`WizardError::Answers`]).
    /// Empty for every other error.
    pub fn step_issues(&self) -> Vec<StepIssue> {
        match self {
            WizardError::Input(issue) => vec![issue.clone()],
            WizardError::Answers(issues) => issues.clone(),
            _ => Vec::new(),
        }
    }

    /// The error as the shell sends it over IPC. [`WizardError`] itself is not
    /// `Serialize` because it wraps [`ClientError`]; this carries what a UI
    /// needs and, like every message here, never a token.
    pub fn to_failure(&self) -> WizardFailure {
        let kind = match self {
            WizardError::Unauthorized { .. } => FailureKind::Unauthorized,
            WizardError::NotFound { .. } => FailureKind::NotFound,
            WizardError::Client(_) => FailureKind::Network,
            WizardError::Input(_) | WizardError::Answers(_) => FailureKind::Answers,
            WizardError::Edit(_) => FailureKind::ExistingConfig,
            WizardError::Invalid(_) => FailureKind::Invalid,
            WizardError::TokenInConfig { .. } => FailureKind::TokenInConfig,
        };
        WizardFailure {
            kind,
            message: self.to_string(),
            issues: self.step_issues(),
            diagnostics: match self {
                WizardError::Invalid(d) => d.clone(),
                _ => Vec::new(),
            },
        }
    }

    fn from_client(error: ClientError, what: impl Into<String>) -> Self {
        match error {
            ClientError::Auth { status } => WizardError::Unauthorized { status },
            ClientError::NotFound { .. } => WizardError::NotFound { what: what.into() },
            other => WizardError::Client(other),
        }
    }
}

/// What kind of failure a [`WizardFailure`] is, so the UI can pick a place to
/// send the user without parsing the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// 401/403: the token.
    Unauthorized,
    /// 404: the project or pipeline, or the token cannot see it.
    NotFound,
    /// Network, rate limit or an undecodable answer. Worth retrying.
    Network,
    /// One or more answers need fixing; see `issues`.
    Answers,
    /// The existing config.toml could not be edited (usually: not TOML).
    ExistingConfig,
    /// The config the wizard would write does not load; see `diagnostics`.
    Invalid,
    /// The result carried something shaped like a token and was refused.
    TokenInConfig,
}

/// A [`WizardError`] in the shape the shell sends over IPC.
#[derive(Debug, Clone, Serialize)]
pub struct WizardFailure {
    /// What went wrong, as a category.
    pub kind: FailureKind,
    /// One sentence for the user. Never contains a token.
    pub message: String,
    /// Per-step problems when `kind` is `answers`; empty otherwise.
    pub issues: Vec<StepIssue>,
    /// Load errors when `kind` is `invalid`; empty otherwise.
    pub diagnostics: Vec<Diagnostic>,
}

impl From<&WizardError> for WizardFailure {
    fn from(e: &WizardError) -> Self {
        e.to_failure()
    }
}

// ---------------------------------------------------------------------------
// Step 1: the connection
// ---------------------------------------------------------------------------

/// What kind of credential a token is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TokenKind {
    /// A personal access token of a human user. Can list their projects.
    Personal,
    /// A project access token. Its bot user belongs to exactly one project and
    /// cannot usefully list projects.
    Project {
        /// The project, read from the bot user's name when it follows GitLab's
        /// `project_<id>_bot_<hex>` convention.
        project_id: Option<u64>,
    },
    /// A group access token. Lists the group's projects.
    Group {
        /// The group, read from the bot user's name.
        group_id: Option<u64>,
    },
    /// A bot that is neither of the above, such as a service account.
    ServiceAccount,
    /// Not a token `/personal_access_tokens/self` recognises (an OAuth token, a
    /// job token, or an instance too old to have the endpoint).
    Unknown,
}

/// Who a token authenticates as. Never carries the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Login name.
    pub username: String,
    /// Display name.
    pub name: Option<String>,
    /// True for a bot user (project, group and service-account tokens).
    pub bot: bool,
    /// The kind of token.
    pub token: TokenKind,
    /// The token's scopes, when `/personal_access_tokens/self` answered.
    pub scopes: Vec<String>,
    /// The token's expiry date, `YYYY-MM-DD`, when it has one.
    pub expires_at: Option<String>,
    /// Things worth telling the user before they continue.
    pub warnings: Vec<String>,
}

impl Identity {
    /// Whether the project picker can list projects for this token.
    pub fn can_list_projects(&self) -> bool {
        !matches!(self.token, TokenKind::Project { .. })
    }
}

/// Classify a token from its user and whether `/personal_access_tokens/self`
/// recognised it.
///
/// Project and group access tokens authenticate as bot users named
/// `project_<id>_bot_<hex>` and `group_<id>_bot_<hex>`; that naming is the
/// only place the API says which one it is, so it is read here.
pub fn classify_token(user: &User, token_self_answered: bool) -> TokenKind {
    let id_after = |prefix: &str| -> Option<Option<u64>> {
        let rest = user.username.strip_prefix(prefix)?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let after = &rest[digits.len()..];
        after.starts_with("_bot").then(|| digits.parse().ok())
    };
    if user.bot {
        if let Some(id) = id_after("project_") {
            return TokenKind::Project { project_id: id };
        }
        if let Some(id) = id_after("group_") {
            return TokenKind::Group { group_id: id };
        }
        return TokenKind::ServiceAccount;
    }
    if token_self_answered {
        TokenKind::Personal
    } else {
        TokenKind::Unknown
    }
}

/// Step 1's "Test connection": who is this token, and what can it do?
///
/// `GET /user` must succeed; its failure is the answer (a 401 is a bad token).
/// `GET /personal_access_tokens/self` is best effort: it supplies scopes and
/// expiry where the instance and the credential support it.
pub async fn test_connection(client: &GitLabClient) -> Result<Identity, WizardError> {
    let user = client
        .current_user()
        .await
        .map_err(|e| WizardError::from_client(e, "the current user"))?;
    let info = client.token_self().await.ok();
    let token = classify_token(&user, info.is_some());

    let mut warnings = Vec::new();
    if let TokenKind::Project { project_id } = &token {
        warnings.push(match project_id {
            Some(id) => format!(
                "This is a project access token for project {id}. It can read only that \
                 project, so bridgewatch cannot list projects for it: type the project id \
                 ({id}) or its path."
            ),
            None => "This is a project access token. It can read only its own project, so \
                     bridgewatch cannot list projects for it: type the project id or path."
                .to_string(),
        });
    }
    let scopes = info.as_ref().map(|i| i.scopes.clone()).unwrap_or_default();
    if info.is_some() && !scopes.iter().any(|s| s == "read_api" || s == "api") {
        warnings.push(format!(
            "The token's scopes are [{}]. bridgewatch reads pipelines through the API, which \
             needs read_api (or api); expect 403s until it has one.",
            scopes.join(", ")
        ));
    }
    if info.as_ref().and_then(|i| i.active) == Some(false) {
        warnings.push("GitLab reports this token as inactive.".to_string());
    }

    Ok(Identity {
        username: user.username,
        name: user.name,
        bot: user.bot,
        token,
        scopes,
        expires_at: info.and_then(|i| i.expires_at),
        warnings,
    })
}

// ---------------------------------------------------------------------------
// Step 1: glab's keyring item
// ---------------------------------------------------------------------------

/// Asks the OS credential store whether an item EXISTS. It has no way to
/// return a value, by construction: the wizard only needs to know whether to
/// offer `glab`'s token, and reading it here would put a credential in memory
/// for no reason.
pub trait KeyringProbe: Send + Sync {
    /// Whether an item with this service and user exists. `Err` means the
    /// store could not be asked (no `secret-tool`, an unsupported platform).
    fn exists(&self, service: &str, user: &str) -> Result<bool, String>;
}

/// The real probe. macOS asks `security find-generic-password` WITHOUT `-w`
/// or `-g`, which prints attributes only and never the password; Linux asks
/// `secret-tool lookup` with stdout discarded, so the value never reaches this
/// process. Both are bounded to [`KEYRING_PROBE_TIMEOUT`].
///
/// ⚠️ Not exercised by the test suite, deliberately: the tests never touch a
/// real credential store. The Linux path is unverified on a real desktop.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemKeyringProbe;

/// How long a keyring probe may take before it counts as "could not ask".
pub const KEYRING_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl KeyringProbe for SystemKeyringProbe {
    fn exists(&self, service: &str, user: &str) -> Result<bool, String> {
        use std::process::{Command, Stdio};

        let (mut command, missing_code) = if cfg!(target_os = "macos") {
            let mut c = Command::new("security");
            c.args(["find-generic-password", "-s", service]);
            if !user.is_empty() {
                c.args(["-a", user]);
            }
            (c, 44)
        } else if cfg!(target_os = "linux") {
            let mut c = Command::new("secret-tool");
            c.args(["lookup", "service", service, "username", user]);
            (c, 1)
        } else {
            return Err("no keyring probe for this platform".to_string());
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not run the keyring tool: {e}"))?;
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return match status.code() {
                        Some(0) => Ok(true),
                        Some(c) if c == missing_code => Ok(false),
                        other => Err(format!("the keyring tool exited with {other:?}")),
                    };
                }
                Ok(None) if started.elapsed() > KEYRING_PROBE_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("the keyring tool did not answer in time".to_string());
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// The keyring service `glab` stores an instance's token under:
/// `glab:<host>:token`, with an empty user. `None` for a URL with no host.
///
/// Verified for gitlab.com (see [`crate::token`]); the self-managed form
/// follows glab's own naming and has not been checked against a live install.
pub fn glab_service_for(base_url: &str) -> Option<String> {
    let rest = base_url
        .trim()
        .strip_prefix("https://")
        .or_else(|| base_url.trim().strip_prefix("http://"))?;
    let host = rest.split('/').next().unwrap_or("").to_ascii_lowercase();
    (!host.is_empty()).then(|| format!("glab:{host}:token"))
}

/// What the wizard found when it looked for `glab`'s token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum GlabDetection {
    /// The item exists; offer this source.
    Found {
        /// The token source to write if the user accepts.
        source: TokenSource,
    },
    /// Nothing at that address.
    NotFound {
        /// The service that was asked about.
        service: String,
    },
    /// The store could not be asked. Not an error for the wizard: it just
    /// does not offer the option.
    Unavailable {
        /// Why.
        reason: String,
    },
}

/// Look for `glab`'s keyring item for this instance. Existence only.
pub fn detect_glab_token(probe: &dyn KeyringProbe, base_url: &str) -> GlabDetection {
    let Some(service) = glab_service_for(base_url) else {
        return GlabDetection::Unavailable {
            reason: format!("{base_url:?} has no host to look up"),
        };
    };
    match probe.exists(&service, "") {
        Ok(true) => GlabDetection::Found {
            source: TokenSource::Keyring {
                service,
                user: String::new(),
            },
        },
        Ok(false) => GlabDetection::NotFound { service },
        Err(reason) => GlabDetection::Unavailable { reason },
    }
}

// ---------------------------------------------------------------------------
// Step 2: the project
// ---------------------------------------------------------------------------

/// One project in the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    /// Numeric id.
    pub id: u64,
    /// `group/project`.
    pub path: String,
    /// Display name.
    pub name: Option<String>,
    /// Default branch, `None` for an empty repository.
    pub default_branch: Option<String>,
    /// Link.
    pub web_url: Option<String>,
}

impl From<crate::model::Project> for ProjectSummary {
    fn from(p: crate::model::Project) -> Self {
        Self {
            id: p.id,
            path: p.path_with_namespace,
            name: p.name,
            default_branch: p.default_branch,
            web_url: p.web_url,
        }
    }
}

/// What the project step can offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ProjectListing {
    /// Projects to pick from, most recently active first.
    Projects {
        /// The projects.
        projects: Vec<ProjectSummary>,
        /// True when there were more than were fetched; the UI should say
        /// "search to narrow it".
        truncated: bool,
    },
    /// This token cannot list projects: the user types an id or a path.
    TypeIdOrPath {
        /// One sentence saying why.
        reason: String,
        /// A project id worth pre-filling, when the token names one.
        suggestion: Option<u64>,
    },
}

/// How many pages of 100 the picker fetches before asking for a search.
pub const PROJECT_PAGES: u32 = 3;

/// Step 2: the projects this token can pick from.
///
/// A project access token is never asked (its bot user is a member of one
/// project and the answer would be at best that project, at worst empty); an
/// account whose listing is refused gets the same "type it" answer rather than
/// an error, because typing an id still works for it.
pub async fn list_projects(
    client: &GitLabClient,
    token: &TokenKind,
    search: Option<&str>,
) -> Result<ProjectListing, WizardError> {
    if let TokenKind::Project { project_id } = token {
        return Ok(ProjectListing::TypeIdOrPath {
            reason: "A project access token can read only its own project, so there is no \
                     list to pick from. Type the project id or path."
                .to_string(),
            suggestion: *project_id,
        });
    }
    match client.list_projects(search, PROJECT_PAGES).await {
        Ok((projects, truncated)) => Ok(ProjectListing::Projects {
            projects: projects.into_iter().map(Into::into).collect(),
            truncated,
        }),
        Err(ClientError::Auth { status: 403 }) | Err(ClientError::NotFound { .. }) => {
            Ok(ProjectListing::TypeIdOrPath {
                reason: "This token is not allowed to list projects. Type the project id or \
                         path."
                    .to_string(),
                suggestion: None,
            })
        }
        Err(e) => Err(WizardError::from_client(e, "the project list")),
    }
}

/// Turn what somebody typed into a [`ProjectRef`], without a request.
///
/// Accepts a numeric id, `group/project`, and a pasted project URL
/// (`https://gitlab.com/group/project`, with or without `.git`, a trailing
/// slash or a `/-/...` suffix).
pub fn parse_project_input(input: &str) -> Result<ProjectRef, WizardError> {
    let issue = |message: &str| {
        WizardError::Input(StepIssue {
            step: WizardStep::Project,
            field: "project".into(),
            message: message.to_string(),
        })
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(issue(
            "type a project id (e.g. 82468124) or path (group/project)",
        ));
    }
    if let Ok(id) = trimmed.parse::<u64>() {
        return Ok(ProjectRef::Id(id));
    }
    let mut path = trimmed;
    for scheme in ["https://", "http://"] {
        if let Some(rest) = path.strip_prefix(scheme) {
            path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
        }
    }
    if let Some((before, _)) = path.split_once("/-/") {
        path = before;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if !path.contains('/')
        || path.split('/').any(str::is_empty)
        || path.contains(char::is_whitespace)
    {
        return Err(issue(
            "that is not a project id or a group/project path (a URL of the project works too)",
        ));
    }
    Ok(ProjectRef::Path(path.to_string()))
}

/// A project, resolved against the instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProject {
    /// Numeric id. The wizard writes this, because it survives a rename.
    pub id: u64,
    /// `group/project`.
    pub path: String,
    /// The default branch, to pre-fill the watch's `ref`. `None` for an empty
    /// repository.
    pub default_branch: Option<String>,
    /// Link.
    pub web_url: Option<String>,
}

/// Step 2: resolve a typed id, path or URL to a real project.
pub async fn resolve_project(
    client: &GitLabClient,
    id_or_path: &str,
) -> Result<ResolvedProject, WizardError> {
    let project = parse_project_input(id_or_path)?;
    let found = client
        .project(&project)
        .await
        .map_err(|e| WizardError::from_client(e, format!("project {project}")))?;
    Ok(ResolvedProject {
        id: found.id,
        path: found.path_with_namespace,
        default_branch: found.default_branch,
        web_url: found.web_url,
    })
}

// ---------------------------------------------------------------------------
// Step 4: deploy markers
// ---------------------------------------------------------------------------

/// A job name worth offering as a deploy marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkerSuggestion {
    /// The job name, exactly as a marker would match it.
    pub name: String,
    /// Its stage.
    pub stage: Option<String>,
    /// Where it was found: `"parent"`, or the name of the trigger job whose
    /// child pipeline holds it.
    pub pipeline: String,
    /// Its status in that pipeline.
    pub status: String,
    /// Higher is more deploy-like. Only positive scores are returned.
    pub score: u32,
    /// Why it scored, in words, for the UI's tooltip.
    pub reasons: Vec<String>,
}

/// One pipeline's jobs, labelled with where they came from.
#[derive(Debug, Clone, Copy)]
pub struct MarkerScope<'a> {
    /// `"parent"` or the trigger job's name.
    pub label: &'a str,
    /// The jobs.
    pub jobs: &'a [Job],
}

fn words(s: &str) -> Vec<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Score one job name and stage. Pure, so the ranking is testable on its own.
fn score(name: &str, stage: Option<&str>, succeeded: bool) -> (u32, Vec<String>) {
    let mut total = 0;
    let mut reasons = Vec::new();
    let name_words = words(name);
    let stage_words = stage.map(words).unwrap_or_default();
    let has = |ws: &[String], keys: &[&str]| ws.iter().any(|w| keys.contains(&w.as_str()));

    const DEPLOY: &[&str] = &["deploy", "deployment", "deploys", "rollout"];
    const RELEASE: &[&str] = &["release", "publish", "ship", "promote"];
    const ENV: &[&str] = &["production", "prod", "live"];

    if has(&name_words, DEPLOY) {
        total += 4;
        reasons.push("the name says deploy".to_string());
    } else if has(&name_words, RELEASE) {
        total += 2;
        reasons.push("the name says release or publish".to_string());
    }
    if has(&stage_words, DEPLOY) {
        total += 3;
        reasons.push("it is in a deploy stage".to_string());
    } else if has(&stage_words, RELEASE) {
        total += 2;
        reasons.push("it is in a release stage".to_string());
    }
    if total > 0 && has(&name_words, ENV) {
        total += 1;
        reasons.push("it names production".to_string());
    }
    if total > 0 && succeeded {
        total += 1;
        reasons.push("it succeeded in the latest pipeline".to_string());
    }
    (total, reasons)
}

/// Rank every job in `scopes` by how deploy-like it looks, best first.
///
/// A name that appears in more than one pipeline is offered once, at its best
/// score. Ties keep pipeline order (parent first) and then job name, so the
/// list is stable across calls.
pub fn rank_deploy_markers(scopes: &[MarkerScope<'_>]) -> Vec<MarkerSuggestion> {
    let mut out: Vec<(usize, MarkerSuggestion)> = Vec::new();
    for (order, scope) in scopes.iter().enumerate() {
        for job in scope.jobs {
            let (score, reasons) = score(&job.name, job.stage.as_deref(), job.status.is_success());
            if score == 0 {
                continue;
            }
            let suggestion = MarkerSuggestion {
                name: job.name.clone(),
                stage: job.stage.clone(),
                pipeline: scope.label.to_string(),
                status: job.status.to_string(),
                score,
                reasons,
            };
            match out.iter_mut().find(|(_, s)| s.name == job.name) {
                Some(existing) if existing.1.score >= score => {}
                Some(existing) => *existing = (order, suggestion),
                None => out.push((order, suggestion)),
            }
        }
    }
    out.sort_by(|(ao, a), (bo, b)| {
        b.score
            .cmp(&a.score)
            .then(ao.cmp(bo))
            .then(a.name.cmp(&b.name))
    });
    out.into_iter().map(|(_, s)| s).collect()
}

/// The deploy step's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkerSuggestions {
    /// The pipeline the suggestions came from, `None` when the ref has none.
    pub pipeline_id: Option<u64>,
    /// Its link.
    pub pipeline_url: Option<String>,
    /// Ranked suggestions, best first. Empty is a real answer: offer "none".
    pub suggestions: Vec<MarkerSuggestion>,
    /// Trigger jobs whose child pipeline could not be read (a multi-project
    /// child the token cannot see, say). Their jobs are not in the list.
    pub unread_children: Vec<String>,
}

/// Pick the pipeline to learn from: the newest `push` one, else the newest
/// that is not a schedule (an hourly schedule is often not representative),
/// else the newest of any kind.
fn representative(rows: &[Pipeline]) -> Option<&Pipeline> {
    rows.iter()
        .find(|p| p.source.as_deref() == Some("push"))
        .or_else(|| {
            rows.iter()
                .find(|p| p.source.as_deref() != Some("schedule"))
        })
        .or_else(|| rows.first())
}

/// Step 4: suggest deploy markers from the latest pipeline on `ref_name`,
/// reading its jobs and, through its bridges, every child pipeline's jobs.
pub async fn suggest_deploy_markers(
    client: &GitLabClient,
    project: &ProjectRef,
    ref_name: &str,
) -> Result<MarkerSuggestions, WizardError> {
    let rows = client
        .list_pipelines(project, &ListQuery::exact(ref_name, None, 20))
        .await
        .map_err(|e| WizardError::from_client(e, format!("pipelines of {project}")))?;
    let Some(pipeline) = representative(&rows).cloned() else {
        return Ok(MarkerSuggestions {
            pipeline_id: None,
            pipeline_url: None,
            suggestions: Vec::new(),
            unread_children: Vec::new(),
        });
    };
    let what = || format!("pipeline {}", pipeline.id);
    let jobs = client
        .pipeline_jobs(project, pipeline.id)
        .await
        .map_err(|e| WizardError::from_client(e, what()))?;
    let bridges = client
        .pipeline_bridges(project, pipeline.id)
        .await
        .map_err(|e| WizardError::from_client(e, what()))?;

    let mut children: Vec<(String, Vec<Job>)> = Vec::new();
    let mut unread = Vec::new();
    for bridge in &bridges {
        let Some(down) = &bridge.downstream_pipeline else {
            continue;
        };
        let child_project = down
            .project_id
            .map(ProjectRef::Id)
            .unwrap_or_else(|| project.clone());
        match client.child_jobs(&child_project, down.id).await {
            Ok(js) => children.push((bridge.name.clone(), js)),
            Err(ClientError::Auth { .. }) | Err(ClientError::NotFound { .. }) => {
                unread.push(bridge.name.clone());
            }
            Err(e) => return Err(WizardError::Client(e)),
        }
    }

    let mut scopes = vec![MarkerScope {
        label: "parent",
        jobs: &jobs,
    }];
    scopes.extend(
        children
            .iter()
            .map(|(label, js)| MarkerScope { label, jobs: js }),
    );

    Ok(MarkerSuggestions {
        pipeline_id: Some(pipeline.id),
        pipeline_url: pipeline.web_url.clone(),
        suggestions: rank_deploy_markers(&scopes),
        unread_children: unread,
    })
}

// ---------------------------------------------------------------------------
// Step 6: the config
// ---------------------------------------------------------------------------

/// The notification switches the wizard asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyAnswers {
    /// A deploy marker succeeded.
    pub deployed: bool,
    /// A blocking failure appeared.
    pub blocking_failure: bool,
    /// A pipeline settled.
    pub finished: bool,
}

/// Every answer the wizard collects. Serialisable so the shell can pass it
/// over IPC unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WizardAnswers {
    /// The `[accounts.<name>]` key. [`suggest_account_name`] offers one.
    pub account: String,
    /// Instance root, e.g. `https://gitlab.com`.
    pub base_url: String,
    /// Where the token comes from. Never the token.
    pub token: TokenSource,
    /// The project, by id where the wizard resolved one.
    pub project: Option<ProjectRef>,
    /// The primary watch's id. [`suggest_watch_id`] offers one.
    pub watch_id: String,
    /// The branch to watch, usually the default branch.
    pub ref_name: String,
    /// Pipeline sources to accept. `["push"]` by default.
    pub sources: Vec<String>,
    /// Deploy markers; empty is "none".
    pub deploy_markers: Vec<String>,
    /// Add a secondary watch for scheduled pipelines on the same branch.
    pub schedule_watch: bool,
    /// Add a secondary watch for this ref glob, e.g. `pf/*`.
    pub preflight_ref: Option<String>,
    /// Notification switches for the primary watch. `None` leaves them alone.
    pub notify: Option<NotifyAnswers>,
    /// Register as a login item. `None` leaves it alone.
    pub launch_at_login: Option<bool>,
    /// Poll interval while live, seconds. `None` leaves the default.
    pub live_secs: Option<u64>,
}

impl Default for WizardAnswers {
    fn default() -> Self {
        Self {
            account: "gitlab".into(),
            base_url: "https://gitlab.com".into(),
            token: TokenSource::Own(true),
            project: None,
            watch_id: "main".into(),
            ref_name: "main".into(),
            sources: vec!["push".into()],
            deploy_markers: Vec::new(),
            schedule_watch: false,
            preflight_ref: None,
            notify: None,
            launch_at_login: None,
            live_secs: None,
        }
    }
}

/// An account key for an instance: `gitlab` for gitlab.com, else the host's
/// first label (`gitlab.example.com` → `gitlab`, `code.acme.io` → `code`).
pub fn suggest_account_name(base_url: &str) -> String {
    let host = glab_service_for(base_url)
        .and_then(|s| {
            s.strip_prefix("glab:")
                .and_then(|h| h.strip_suffix(":token"))
                .map(str::to_string)
        })
        .unwrap_or_default();
    let first = host.split(['.', ':']).next().unwrap_or("");
    let clean: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if clean.is_empty() {
        "gitlab".into()
    } else {
        clean
    }
}

/// A watch id for a project path and branch: `<project>-<ref>`, lowercased,
/// with anything that is not a letter, digit, `-` or `_` turned into `-`.
pub fn suggest_watch_id(project_path: &str, ref_name: &str) -> String {
    let last = project_path.rsplit('/').next().unwrap_or(project_path);
    let raw = format!("{last}-{ref_name}").to_ascii_lowercase();
    let mut out = String::new();
    for c in raw.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' {
            c
        } else {
            '-'
        };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    out.trim_matches('-').to_string()
}

/// Prefixes GitLab puts on its tokens. Anything carrying one in the config's
/// text is refused. `go-keyring-base64:` is glab's keyring envelope, which a
/// user could paste from `security find-generic-password -w`.
pub const TOKEN_PREFIXES: &[&str] = &[
    "glpat-",
    "glptt-",
    "gldt-",
    "glrt-",
    "glrtr-",
    "glcbt-",
    "glsoat-",
    "glffct-",
    "glimt-",
    "glagent-",
    "gloas-",
    "glft-",
    "glsa-",
    "go-keyring-base64:",
    "go-keyring-encoded:",
];

/// The first token prefix in `text`, if any.
pub fn find_token_prefix(text: &str) -> Option<&'static str> {
    TOKEN_PREFIXES.iter().copied().find(|p| text.contains(p))
}

/// Check every answer, returning every problem at once.
pub fn validate_answers(answers: &WizardAnswers) -> Vec<StepIssue> {
    let mut out = Vec::new();
    let mut issue = |step, field: &str, message: String| {
        out.push(StepIssue {
            step,
            field: field.to_string(),
            message,
        })
    };
    use WizardStep::*;

    if answers.account.trim().is_empty() {
        issue(
            Account,
            "account",
            "the account needs a name, e.g. gitlab".into(),
        );
    }
    let url = answers.base_url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || glab_service_for(url).is_none()
    {
        issue(
            Account,
            "base_url",
            format!("{url:?} is not an instance URL; use e.g. https://gitlab.com"),
        );
    }
    let token_text = match &answers.token {
        TokenSource::Env(v) => {
            if v.trim().is_empty() {
                issue(
                    Account,
                    "token",
                    "name the environment variable that holds the token".into(),
                );
            } else if !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                issue(
                    Account,
                    "token",
                    "an environment variable name is letters, digits and underscores; paste \
                     the token into the keyring option instead, never here"
                        .into(),
                );
            }
            v.clone()
        }
        TokenSource::Command(argv) => {
            if argv.is_empty() || argv[0].trim().is_empty() {
                issue(Account, "token", "the token command is empty".into());
            }
            argv.join(" ")
        }
        TokenSource::Keyring { service, user } => {
            if service.trim().is_empty() {
                issue(
                    Account,
                    "token",
                    "the keyring item needs a service name".into(),
                );
            }
            format!("{service} {user}")
        }
        TokenSource::Own(own) => {
            if !own {
                issue(Account, "token", "choose where the token comes from".into());
            }
            String::new()
        }
    };
    if let Some(prefix) = find_token_prefix(&token_text) {
        issue(
            Account,
            "token",
            format!(
                "that looks like a token itself ({prefix}…). The config only says WHERE the \
                 token is; store it in the keyring instead"
            ),
        );
    }

    if answers.project.is_none() {
        issue(Project, "project", "pick or type a project".into());
    }
    if let Some(ProjectRef::Path(p)) = &answers.project
        && p.trim().is_empty()
    {
        issue(Project, "project", "the project path is empty".into());
    }

    if answers.watch_id.trim().is_empty() {
        issue(Watch, "watch_id", "the watch needs an id".into());
    }
    if answers.ref_name.trim().is_empty() {
        issue(Watch, "ref_name", "name the branch to watch".into());
    } else if let Err(e) = RefMatcher::parse(&answers.ref_name) {
        issue(Watch, "ref_name", e.to_string());
    }
    for s in &answers.sources {
        if !KNOWN_PIPELINE_SOURCES.contains(&s.as_str()) {
            issue(
                Watch,
                "sources",
                format!("{s:?} is not a pipeline source GitLab sends; push is the usual one"),
            );
        }
    }
    if let Some(glob) = &answers.preflight_ref {
        if glob.trim().is_empty() {
            issue(
                Watch,
                "preflight_ref",
                "the preflight pattern is empty".into(),
            );
        } else if let Err(e) = RefMatcher::parse(glob) {
            issue(Watch, "preflight_ref", e.to_string());
        }
    }

    for m in &answers.deploy_markers {
        if m.trim().is_empty() {
            issue(Deploy, "deploy_markers", "a deploy marker is empty".into());
        } else if let Err(e) = Pattern::parse(m) {
            issue(Deploy, "deploy_markers", e.to_string());
        }
    }

    if let Some(live) = answers.live_secs
        && !(1..=60).contains(&live)
    {
        issue(
            Preferences,
            "live_secs",
            "the live poll interval is between 1 and 60 seconds (the idle one is 60)".into(),
        );
    }
    out
}

/// What [`build_config`] produced.
#[derive(Debug, Clone, Serialize)]
pub struct BuiltConfig {
    /// The complete `config.toml` to show for review and then write.
    pub toml: String,
    /// Warnings from loading it. Errors never get this far.
    pub warnings: Vec<Diagnostic>,
    /// True when an existing file was edited rather than a new one started.
    pub edited_existing: bool,
}

/// The opening comment of a file the wizard starts from nothing.
pub const NEW_FILE_HEADER: &str = "\
# bridgewatch configuration, written by the setup wizard.
#
# Edit freely: the settings window and the wizard edit this file in place and
# keep your comments. `bridgewatch config schema` prints every key.
";

fn s(v: &str) -> EditValue {
    EditValue::String(v.to_string())
}

fn strings(vs: &[String]) -> EditValue {
    EditValue::Array(vs.iter().map(|v| s(v)).collect())
}

fn project_value(p: &ProjectRef) -> EditValue {
    match p {
        ProjectRef::Id(id) => EditValue::Integer(*id as i64),
        ProjectRef::Path(path) => s(path),
    }
}

fn token_edits(prefix: &str, token: &TokenSource) -> Vec<Edit> {
    let set = |path: &str, value: EditValue| Edit::Set {
        path: format!("{prefix}.token.{path}"),
        value,
    };
    let mut edits = vec![Edit::Unset {
        path: format!("{prefix}.token"),
    }];
    match token {
        TokenSource::Keyring { service, user } => {
            edits.push(set("keyring.service", s(service)));
            edits.push(set("keyring.user", s(user)));
        }
        TokenSource::Env(v) => edits.push(set("env", s(v))),
        TokenSource::Command(argv) => edits.push(set("command", strings(argv))),
        TokenSource::Own(b) => edits.push(set("own", EditValue::Boolean(*b))),
    }
    edits
}

/// A minimal new watch, through serde so every default is the schema's own.
fn new_watch(
    id: &str,
    account: &str,
    project: &ProjectRef,
    ref_name: &str,
    sources: &[String],
    role: Role,
) -> Result<Watch, WizardError> {
    let value = serde_json::json!({
        "id": id,
        "account": account,
        "project": project,
        "ref": ref_name,
        "sources": sources,
        "role": role,
    });
    serde_json::from_value(value).map_err(|e| {
        WizardError::Edit(EditError::Serialize(format!(
            "could not build the watch: {e}"
        )))
    })
}

/// Keys of a new watch that are always written, even at their default,
/// because a reader needs them to know what the watch is.
const ALWAYS_WRITTEN: &[&str] = &["id", "account", "project", "ref", "role"];

/// Append `watch` through [`Edit::AddWatch`] (the settings window's path), then
/// remove every key and inline sub-key that equals the schema default.
///
/// `AddWatch` renders every field, notification templates included, which is
/// right for the settings pane's "add" and wrong for a file somebody reads the
/// day after the wizard wrote it: the defaults are the schema's, and writing
/// them out pins them against a later change of default. Removing them does
/// not change what the file means, which the final load proves.
fn add_minimal_watch(editor: &mut ConfigEditor, watch: &Watch) -> Result<(), WizardError> {
    let reference = new_watch(
        &watch.id,
        &watch.account,
        &watch.project,
        &watch.ref_pattern,
        &[],
        watch.role,
    )?;
    let to_json = |w: &Watch| {
        serde_json::to_value(w).map_err(|e| WizardError::Edit(EditError::Serialize(e.to_string())))
    };
    let (have, default) = (to_json(watch)?, to_json(&reference)?);
    editor.apply(&[Edit::AddWatch {
        watch: Box::new(watch.clone()),
    }])?;
    let index = editor
        .watch_index(&watch.id)
        .ok_or_else(|| EditError::NoSuchWatch(watch.id.clone()))?;

    let (Some(have), Some(default)) = (have.as_object(), default.as_object()) else {
        return Ok(());
    };
    let mut unsets = Vec::new();
    for (key, value) in have {
        if ALWAYS_WRITTEN.contains(&key.as_str()) {
            continue;
        }
        let base = format!("watches.{index}.{}", quote_path_segment(key));
        match (value, default.get(key)) {
            (v, Some(d)) if v == d => unsets.push(Edit::Unset { path: base }),
            (serde_json::Value::Object(sub), Some(serde_json::Value::Object(dsub))) => {
                for (k, v) in sub {
                    if dsub.get(k) == Some(v) {
                        unsets.push(Edit::Unset {
                            path: format!("{base}.{}", quote_path_segment(k)),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    editor.apply(&unsets)?;
    Ok(())
}

/// Step 6: the `config.toml` the answers describe.
///
/// With `existing` (the text of a config already on disk) it EDITS that text
/// through [`ConfigEditor`], the settings window's own path, so every comment
/// and every watch it does not touch survives byte for byte:
///
/// - the account named `answers.account` is updated, or added;
/// - the watch with id `answers.watch_id` is updated in place, or appended;
/// - the scheduled and preflight watches reuse an existing secondary watch
///   for the same project and source/ref when there is one, and are otherwise
///   appended as `<id>-schedule` / `<id>-preflight`;
/// - a value that already equals the answer is not rewritten at all, so
///   re-running the wizard with the same answers returns the file unchanged.
///
/// Nothing is ever removed. The result must load (errors refuse, warnings are
/// returned) and must not contain anything shaped like a token
/// ([`TOKEN_PREFIXES`]).
pub fn build_config(
    answers: &WizardAnswers,
    existing: Option<&str>,
) -> Result<BuiltConfig, WizardError> {
    let issues = validate_answers(answers);
    if !issues.is_empty() {
        return Err(WizardError::Answers(issues));
    }
    let project = answers.project.clone().expect("validated above");

    let mut editor = ConfigEditor::new(existing.unwrap_or(""))?;
    // What is already there, so equal values are left alone. A file that does
    // not load today is still edited; its problems surface in the final check.
    let current: Option<Config> = existing
        .and_then(|raw| config::parse_str(raw, Path::new("config.toml")).ok())
        .map(|l| l.config);

    let mut edits = Vec::new();
    let mut new_watches: Vec<Watch> = Vec::new();

    // --- account --------------------------------------------------------
    let acct = format!("accounts.{}", quote_path_segment(&answers.account));
    let current_account = current
        .as_ref()
        .and_then(|c| c.accounts.get(&answers.account));
    let base_url = answers.base_url.trim().trim_end_matches('/');
    if current_account.map(|a| a.base_url.as_str()) != Some(base_url) {
        edits.push(Edit::Set {
            path: format!("{acct}.base_url"),
            value: s(base_url),
        });
    }
    if current_account.map(|a| &a.token) != Some(&answers.token) {
        edits.extend(token_edits(&acct, &answers.token));
    }

    // --- primary watch --------------------------------------------------
    let existing_watch = current
        .as_ref()
        .and_then(|c| c.watches.iter().find(|w| w.id == answers.watch_id));
    match existing_watch {
        None => {
            let mut watch = new_watch(
                &answers.watch_id,
                &answers.account,
                &project,
                &answers.ref_name,
                &answers.sources,
                Role::Primary,
            )?;
            watch.deploy_markers = answers.deploy_markers.clone();
            if let Some(live) = answers.live_secs {
                watch.poll.live_secs = live;
            }
            if let Some(n) = answers.notify {
                watch.notify.deployed = n.deployed;
                watch.notify.blocking_failure = n.blocking_failure;
                watch.notify.finished = n.finished;
            }
            new_watches.push(watch);
        }
        Some(w) => {
            let mut set = |path: &str, value: EditValue| {
                edits.push(Edit::SetWatch {
                    id: answers.watch_id.clone(),
                    path: path.to_string(),
                    value,
                })
            };
            if w.account != answers.account {
                set("account", s(&answers.account));
            }
            if w.project != project {
                set("project", project_value(&project));
            }
            if w.ref_pattern != answers.ref_name {
                set("ref", s(&answers.ref_name));
            }
            if w.sources != answers.sources {
                set("sources", strings(&answers.sources));
            }
            if w.deploy_markers != answers.deploy_markers {
                set("deploy_markers", strings(&answers.deploy_markers));
            }
            if let Some(live) = answers.live_secs
                && w.poll.live_secs != live
            {
                set("poll.live_secs", EditValue::Integer(live as i64));
            }
            if let Some(n) = answers.notify {
                for (key, want, have) in [
                    ("notify.deployed", n.deployed, w.notify.deployed),
                    (
                        "notify.blocking_failure",
                        n.blocking_failure,
                        w.notify.blocking_failure,
                    ),
                    ("notify.finished", n.finished, w.notify.finished),
                ] {
                    if want != have {
                        set(key, EditValue::Boolean(want));
                    }
                }
            }
        }
    }

    // --- secondary watches ----------------------------------------------
    let watches: &[Watch] = current
        .as_ref()
        .map(|c| c.watches.as_slice())
        .unwrap_or(&[]);
    let taken = |id: &str| watches.iter().any(|w| w.id == id);
    if answers.schedule_watch {
        let covered = watches.iter().any(|w| {
            w.project == project && w.ref_pattern == answers.ref_name && w.sources == ["schedule"]
        });
        let id = format!("{}-schedule", answers.watch_id);
        if !covered && !taken(&id) {
            let mut watch = new_watch(
                &id,
                &answers.account,
                &project,
                &answers.ref_name,
                &["schedule".to_string()],
                Role::Secondary,
            )?;
            // Stays cheap until something breaks, as the shipped example does.
            watch.dive.only_when = Some("failed".into());
            new_watches.push(watch);
        }
    }
    if let Some(glob) = &answers.preflight_ref {
        let covered = watches
            .iter()
            .any(|w| w.project == project && &w.ref_pattern == glob);
        let id = format!("{}-preflight", answers.watch_id);
        if !covered && !taken(&id) {
            let mut watch = new_watch(&id, &answers.account, &project, glob, &[], Role::Secondary)?;
            // A row per preflight, no dive: one request a tick.
            watch.dive.bridges = String::new();
            new_watches.push(watch);
        }
    }

    // --- preferences ----------------------------------------------------
    if let Some(login) = answers.launch_at_login
        && current.as_ref().map(|c| c.ui.launch_at_login) != Some(login)
    {
        edits.push(Edit::Set {
            path: "ui.launch_at_login".into(),
            value: EditValue::Boolean(login),
        });
    }

    editor.apply(&edits)?;
    for watch in &new_watches {
        add_minimal_watch(&mut editor, watch)?;
    }
    let toml = match existing {
        Some(_) => editor.to_toml(),
        // A document holding only a comment keeps it as TRAILING decor, so
        // the header would print below everything the edits added. A new file
        // is therefore built empty and the header put on top here.
        None => format!("{NEW_FILE_HEADER}{}", editor.to_toml()),
    };

    if let Some(prefix) = find_token_prefix(&toml) {
        return Err(WizardError::TokenInConfig { prefix });
    }
    let loaded = match config::parse_str(&toml, Path::new("config.toml")) {
        Ok(l) => l,
        Err(config::ConfigError::Invalid { diagnostics, .. }) => {
            return Err(WizardError::Invalid(diagnostics));
        }
        Err(e) => {
            return Err(WizardError::Invalid(vec![Diagnostic {
                severity: Severity::Error,
                path: String::new(),
                message: e.to_string(),
                span: None,
            }]));
        }
    };
    Ok(BuiltConfig {
        toml,
        warnings: loaded.warnings,
        edited_existing: existing.is_some(),
    })
}
