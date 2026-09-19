//! The typed shape of `config.toml`.
//!
//! Every optional key has a default, so a file containing nothing but one
//! account and one watch loads. The same structs generate the JSON Schema that
//! `bridgewatch config schema` prints and that the GUI's settings form is built
//! from, which is what keeps the file and the GUI honest about each other.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

/// The whole configuration file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// CI instances to talk to, GitLab or GitHub, keyed by the name watches
    /// refer to.
    pub accounts: BTreeMap<String, Account>,
    /// What to watch. Order is the display order.
    pub watches: Vec<Watch>,
    /// Tray icon appearance.
    pub icon: IconConfig,
    /// Optional replacement for the built-in icon-state rules.
    pub verdict: VerdictConfig,
    /// Window and presentation settings, consumed by the GUI shell.
    pub ui: UiConfig,
    /// Logging and request-ring settings.
    pub log: LogConfig,
}

impl Config {
    /// How many accounts are configured, for status lines. Counts the map's KEYS so
    /// that nothing printed flows from `Account`, whose `TokenSource` CodeQL's
    /// `rust/cleartext-logging` treats as sensitive even though it only names where a
    /// token lives.
    pub fn account_count(&self) -> usize {
        self.accounts.keys().count()
    }
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

/// Which CI provider an account talks to.
///
/// ⛔ The default is `gitlab` and must stay that way: every configuration file
/// written before this key existed has no `provider` line, and adding one is
/// not something bridgewatch may do to a user's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// GitLab CI, the provider bridgewatch was built for.
    #[default]
    Gitlab,
    /// GitHub Actions. A watch's `group` key decides what one row is: one
    /// workflow run (`run`, the default, narrowed by `workflow`), or every run
    /// one push started (`commit`).
    Github,
}

impl Provider {
    /// Whether this is GitLab, the default. A serde `skip_serializing_if`
    /// predicate, which is why it takes a reference.
    pub fn is_gitlab(&self) -> bool {
        *self == Provider::Gitlab
    }

    /// The config spelling, `gitlab` or `github`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Gitlab => "gitlab",
            Provider::Github => "github",
        }
    }

    /// The instance root an account of this provider talks to when the file
    /// does not say.
    pub fn default_base_url(&self) -> String {
        match self {
            Provider::Gitlab => "https://gitlab.com".to_string(),
            Provider::Github => "https://api.github.com".to_string(),
        }
    }

    /// The API prefix an account of this provider uses when the file does not
    /// say.
    ///
    /// ⚠️ GitHub's is EMPTY: its paths start `/repos/...` on an `api.` host,
    /// and only GitHub Enterprise Server inserts a prefix (`/api/v3`). That is
    /// why `base_url` and `api_path` stay two keys, since a hostname swap
    /// covers one enterprise shape and not the other.
    pub fn default_api_path(&self) -> String {
        match self {
            Provider::Gitlab => "/api/v4".to_string(),
            Provider::Github => String::new(),
        }
    }

    /// The header an account of this provider carries its token in when the
    /// file does not say.
    ///
    /// ⚠️ GitHub reads a credential from `Authorization` and nowhere else, so
    /// the GitLab default would make every GitHub account fail with a 401 until
    /// its owner found the key. It is still an ERROR to write `PRIVATE-TOKEN`
    /// on a GitHub account: defaulting is for the line nobody wrote, not for
    /// overruling one somebody did.
    pub fn default_header(&self) -> AuthHeader {
        match self {
            Provider::Gitlab => AuthHeader::PrivateToken,
            Provider::Github => AuthHeader::AuthorizationBearer,
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One instance and the credential used against it.
//
// ⛔ `Deserialize` is hand-written (see the impl below), because `base_url`,
// `api_path` and `header` default to something DIFFERENT per provider and a
// serde field default cannot see a sibling field. Said in a plain comment
// rather than a doc one: the doc comment becomes the key's description in the
// JSON Schema, which is what the settings pane shows a user.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// Which CI provider this account talks to: `gitlab` or `github`.
    #[serde(default)]
    pub provider: Provider,
    /// Instance root, without a trailing slash, e.g. `https://gitlab.com`.
    /// Defaults per provider: gitlab.com, or `https://api.github.com`. For
    /// GitHub Enterprise Server, the server's root with `api_path = "/api/v3"`.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// API prefix. Overridable for proxies that mount the API elsewhere.
    /// Defaults per provider: `/api/v4`, or empty for github.com. GitHub
    /// Enterprise Server needs `/api/v3`.
    #[serde(default = "default_api_path")]
    pub api_path: String,
    /// Where the token comes from. Never the token itself.
    #[serde(default)]
    pub token: TokenSource,
    /// Which header carries the token. Defaults per provider: `PRIVATE-TOKEN`,
    /// or `Authorization: Bearer` for GitHub, which reads no other header.
    #[serde(default)]
    pub header: AuthHeader,
    /// Per-request timeout.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// How far the poll interval is allowed to back off after rate limiting.
    #[serde(default)]
    pub rate_limit_backoff: RateLimitBackoff,
}

impl Account {
    /// An account for `provider`, with that provider's three defaults.
    ///
    /// ⚠️ The reason this exists rather than being left to struct-update
    /// syntax: `Account { provider: Provider::Github, ..Account::default() }`
    /// compiles, reads as though it said what it means, and produces a GitHub
    /// account pointed at `https://gitlab.com` with `/api/v4` and a
    /// `PRIVATE-TOKEN` header, because the defaults were taken before the
    /// provider was. Nothing in the file path can hit that (the hand-written
    /// `Deserialize` fills the three together), so it is anything building an
    /// account in code that needs this.
    pub fn for_provider(provider: Provider) -> Self {
        Self {
            base_url: provider.default_base_url(),
            api_path: provider.default_api_path(),
            header: provider.default_header(),
            provider,
            ..Account::default()
        }
    }
}

impl Default for Account {
    fn default() -> Self {
        let provider = Provider::default();
        Self {
            base_url: provider.default_base_url(),
            api_path: provider.default_api_path(),
            header: provider.default_header(),
            provider,
            token: TokenSource::default(),
            timeout_secs: default_timeout_secs(),
            rate_limit_backoff: RateLimitBackoff::default(),
        }
    }
}

/// Read an account, filling the three provider-dependent defaults afterwards.
///
/// ⛔ A serde field default is a function of nothing: it cannot look at
/// `provider`, which is why this is written out rather than derived. An
/// explicitly written `base_url` or `api_path` is never touched, so nothing a
/// user typed is reinterpreted because of a key somewhere else in the table.
///
/// The inner representation carries `deny_unknown_fields` and the same field
/// names in the same order, so the "unknown field, expected one of ..." message
/// `parse_lenient` keys on is the one serde would have produced anyway.
impl<'de> Deserialize<'de> for Account {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct AccountRepr {
            #[serde(default)]
            provider: Provider,
            #[serde(default)]
            base_url: Option<String>,
            #[serde(default)]
            api_path: Option<String>,
            #[serde(default)]
            token: TokenSource,
            #[serde(default)]
            header: Option<AuthHeader>,
            #[serde(default = "default_timeout_secs")]
            timeout_secs: u64,
            #[serde(default)]
            rate_limit_backoff: RateLimitBackoff,
        }

        let repr = AccountRepr::deserialize(d)?;
        Ok(Account {
            base_url: repr
                .base_url
                .unwrap_or_else(|| repr.provider.default_base_url()),
            api_path: repr
                .api_path
                .unwrap_or_else(|| repr.provider.default_api_path()),
            header: repr
                .header
                .unwrap_or_else(|| repr.provider.default_header()),
            provider: repr.provider,
            token: repr.token,
            timeout_secs: repr.timeout_secs,
            rate_limit_backoff: repr.rate_limit_backoff,
        })
    }
}

fn default_base_url() -> String {
    Provider::Gitlab.default_base_url()
}
fn default_api_path() -> String {
    Provider::Gitlab.default_api_path()
}
fn default_timeout_secs() -> u64 {
    15
}

/// The header spelling used to authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum AuthHeader {
    /// `PRIVATE-TOKEN: <token>`, the GitLab personal/project access token form.
    #[default]
    #[serde(rename = "PRIVATE-TOKEN")]
    PrivateToken,
    /// `Authorization: Bearer <token>`, for GitLab OAuth and CI job tokens, and
    /// for every GitHub token.
    #[serde(rename = "Authorization: Bearer")]
    AuthorizationBearer,
}

impl AuthHeader {
    /// The header name to set.
    pub fn header_name(&self) -> &'static str {
        match self {
            AuthHeader::PrivateToken => "PRIVATE-TOKEN",
            AuthHeader::AuthorizationBearer => "Authorization",
        }
    }

    /// The header value for a given token.
    pub fn header_value(&self, token: &str) -> String {
        match self {
            AuthHeader::PrivateToken => token.to_string(),
            AuthHeader::AuthorizationBearer => format!("Bearer {token}"),
        }
    }
}

/// How far the backoff is allowed to grow after repeated rate limiting or
/// server errors.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimitBackoff {
    /// Ceiling for the doubled interval, in seconds.
    #[serde(default = "default_backoff_max")]
    pub max_secs: u64,
}

impl Default for RateLimitBackoff {
    fn default() -> Self {
        Self {
            max_secs: default_backoff_max(),
        }
    }
}

fn default_backoff_max() -> u64 {
    300
}

// ---------------------------------------------------------------------------
// Token sources
// ---------------------------------------------------------------------------

/// Where a token is read from.
///
/// A literal token string is rejected at parse time with an explanatory error:
/// bridgewatch never wants a credential sitting in a config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    /// An entry in the OS credential store, addressed exactly as another tool
    /// wrote it. `user` may be empty: `glab` stores its token with an empty
    /// account on the macOS Keychain and an empty `username` attribute on the
    /// Linux Secret Service, and `gh` keeps its active account's token under
    /// `gh:<host>` the same way.
    Keyring {
        /// The credential store's service / collection name.
        service: String,
        /// The account / username attribute. The empty string is valid.
        #[serde(default)]
        user: String,
    },
    /// An environment variable holding the token.
    Env(String),
    /// A program to run; the first line of its stdout, trimmed, is the token.
    /// A non-zero exit is an error.
    Command(Vec<String>),
    /// bridgewatch's own credential-store entry, `bridgewatch:<account>`,
    /// written by the GUI or by `set_own_token`.
    Own(bool),
    /// Sign in with GitHub or GitLab (the OAuth device flow). `oauth = true`
    /// uses bridgewatch's own application on github.com or gitlab.com;
    /// `oauth = { client_id = "..." }` names your own, which GitHub Enterprise
    /// Server and self-managed GitLab need. The tokens are kept in the OS
    /// credential store, never in this file.
    Oauth(OAuthSource),
}

/// Which OAuth application an `oauth` token source signs in through.
///
/// Written `oauth = true` for the built-in application of github.com or
/// gitlab.com, and `oauth = { client_id = "..." }` for any other.
//
// ⚠️ Serialize, Deserialize and JsonSchema are hand-written because the two
// spellings are one value: `true` is "no client id of my own". A plain derive
// would make the file say `oauth = {}`, which the settings editor cannot write
// (an edit sets a leaf, and an empty table has none).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthSource {
    /// The OAuth application's client id. `None` is the provider's built-in one.
    pub client_id: Option<String>,
}

impl Serialize for OAuthSource {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        match &self.client_id {
            None => s.serialize_bool(true),
            Some(id) => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry("client_id", id)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for OAuthSource {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{Error as _, Visitor};

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Repr {
            #[serde(default)]
            client_id: Option<String>,
        }

        struct V;

        impl<'de> Visitor<'de> for V {
            type Value = OAuthSource;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("oauth = true, or oauth = { client_id = \"...\" }")
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                if v {
                    Ok(OAuthSource::default())
                } else {
                    Err(E::custom(
                        "oauth = false selects no source; use oauth = true or another form",
                    ))
                }
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                let repr = Repr::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map_err(A::Error::custom)?;
                Ok(OAuthSource {
                    client_id: repr.client_id,
                })
            }
        }

        d.deserialize_any(V)
    }
}

impl JsonSchema for OAuthSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "OAuthSource".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Which OAuth application to sign in through: true for bridgewatch's own on github.com or gitlab.com, or { client_id = \"...\" } for your own (required for GitHub Enterprise Server and self-managed GitLab).",
            "anyOf": [
                { "type": "boolean", "const": true },
                {
                    "type": "object",
                    "properties": {
                        "client_id": {
                            "description": "The OAuth application's client id. Not a secret.",
                            "type": "string"
                        }
                    },
                    "additionalProperties": false
                }
            ]
        })
    }
}

impl Default for TokenSource {
    fn default() -> Self {
        TokenSource::Own(true)
    }
}

impl<'de> Deserialize<'de> for TokenSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error as _, Visitor};

        /// The keyring variant's own fields, the one source with more than a
        /// single value.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct KeyringRepr {
            service: String,
            #[serde(default)]
            user: String,
        }

        struct V;

        impl<'de> Visitor<'de> for V {
            type Value = TokenSource;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a token source table: { keyring = { service = \"..\", user = \"\" } }, \
                     { env = \"VAR\" }, { command = [\"prog\", \"arg\"] }, { own = true } or \
                     { oauth = true }",
                )
            }

            fn visit_str<E: serde::de::Error>(self, _v: &str) -> Result<Self::Value, E> {
                Err(E::custom(
                    "a token may not be written literally in the config file. \
                     Use one of: token = { keyring = { service = \"glab:gitlab.com:token\", user = \"\" } }, \
                     token = { env = \"BRIDGEWATCH_TOKEN_GITLAB\" }, \
                     token = { command = [\"pass\", \"gitlab/pat\"] }, or token = { own = true }",
                ))
            }

            /// ⛔ The keys are read one at a time rather than through serde's
            /// enum machinery, because that machinery took the FIRST key and
            /// dropped the rest in silence: `token = { env = "T", own = true }`
            /// loaded as the env source, and a user who thought they had moved
            /// their token into bridgewatch's own keychain entry was still
            /// reading the variable. Two sources in one table is not something
            /// to guess at — it is the one key in the file that decides which
            /// credential is sent.
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let Some(key) = map.next_key::<String>()? else {
                    return Err(A::Error::custom(
                        "token = { } names no source; use keyring, env, command, own or oauth",
                    ));
                };
                let source = match key.as_str() {
                    "keyring" => {
                        let k: KeyringRepr = map.next_value()?;
                        TokenSource::Keyring {
                            service: k.service,
                            user: k.user,
                        }
                    }
                    "env" => TokenSource::Env(map.next_value()?),
                    "command" => TokenSource::Command(map.next_value()?),
                    "own" => TokenSource::Own(map.next_value()?),
                    "oauth" => TokenSource::Oauth(map.next_value()?),
                    other => {
                        return Err(A::Error::custom(format!(
                            "unknown token source {other:?}; use keyring, env, command, own or oauth"
                        )));
                    }
                };
                if let Some(extra) = map.next_key::<String>()? {
                    return Err(A::Error::custom(format!(
                        "a token source names exactly one of keyring, env, command, own or oauth; \
                         this one names both {key:?} and {extra:?}, and bridgewatch will not \
                         guess which credential you meant"
                    )));
                }
                Ok(source)
            }
        }

        deserializer.deserialize_any(V).map_err(|e| {
            // `deserialize_any` on a non-string, non-map value produces serde's
            // own wording; leave it, it already names the offending type.
            D::Error::custom(e.to_string())
        })
    }
}

// ---------------------------------------------------------------------------
// Watches
// ---------------------------------------------------------------------------

/// One thing to watch: a project, a ref pattern and a set of pipeline sources.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    /// Stable identifier, used in notifications, `--watch` and the GUI.
    pub id: String,
    /// Which `[accounts.*]` entry to use.
    pub account: String,
    /// GitLab: a numeric project id, or a `group/path` which is URL-encoded
    /// for you. GitHub: `owner/repo`, which is the only form GitHub accepts.
    pub project: ProjectRef,
    /// Ref pattern: exact (`main`), glob (`pf/*`) or regex (`re:^release/.*$`).
    #[serde(rename = "ref", default = "default_ref")]
    pub ref_pattern: String,
    /// GitHub only: the one workflow this watch follows, as its file name
    /// (`ci.yml`) or its numeric id. Absent or empty is every workflow. Leave
    /// it out with `group = "commit"`, which needs every workflow's runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// GitHub only: what one row is. `run` (the default) is one workflow run;
    /// `commit` folds every run of one push (same commit, event and branch)
    /// into one row, each run appearing as a bridge, so `dive` selects
    /// workflow names.
    // Skipped when it is the default, like `workflow`: a watch the settings
    // window or the wizard writes back must not gain a line nobody asked for.
    #[serde(default, skip_serializing_if = "GroupMode::is_run")]
    pub group: GroupMode,
    /// GitHub only, and only with `group = "commit"`: runs of one commit,
    /// event and branch created within this many seconds of the group's
    /// newest run are one row. Absent is 90.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_out_secs: Option<u64>,
    /// GitHub only, and only with `group = "commit"`: workflow FILE names
    /// (`ci.yml`, or the whole `.github/workflows/ci.yml` path), not display
    /// names, that must have a run in every commit group this watch shows.
    /// Once a group has settled and its `fan_out_secs` window has passed, an
    /// expected workflow with no run in it is a dead bridge, the way a GitLab
    /// trigger job that created no child is. Until then it is a pending job.
    /// It applies to every group the watch shows, whatever its event, so list
    /// only workflows that run on EVERY event in `sources`, and none whose
    /// `paths` or `branches` filter can legitimately skip a push.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expect: Vec<String>,
    /// Pipeline sources (GitLab) or workflow events (GitHub) to accept. Empty
    /// means all of them.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Whether this watch drives the tray icon.
    #[serde(default)]
    pub role: Role,
    /// How many pipelines to list.
    #[serde(default)]
    pub show: ShowConfig,
    /// Poll intervals for this watch.
    #[serde(default)]
    pub poll: PollConfig,
    /// Which trigger jobs to walk into, and how deep.
    #[serde(default)]
    pub dive: DiveConfig,
    /// Job names (or `re:` patterns) whose success means "this deployed".
    /// With more than one, CONFIG ORDER decides: the earliest entry here with
    /// a successful job is the deploy that gets reported, whichever ran first.
    #[serde(default)]
    pub deploy_markers: Vec<String>,
    /// What a failed sibling bridge does to a watch that otherwise deployed.
    #[serde(default)]
    pub sibling_failure: FailurePolicy,
    /// What a blocking failure *after* a successful deploy marker does.
    #[serde(default)]
    pub post_deploy_failure: FailurePolicy,
    /// Per-job class overrides. Keys are matched in file order, first wins.
    #[serde(default)]
    pub jobs: JobOverrides,
    /// Which events raise a notification, and how they read.
    #[serde(default)]
    pub notify: NotifyConfig,
}

fn default_ref() -> String {
    "main".to_string()
}

impl Watch {
    /// The jobs mode this watch actually uses: its own `show.jobs` when set,
    /// otherwise the global `ui.jobs`.
    pub fn effective_jobs(&self, ui: &UiConfig) -> JobsMode {
        self.show.jobs.unwrap_or(ui.jobs)
    }

    /// The commit-group window this watch asks for, in seconds, or `None` when
    /// one row is one run.
    ///
    /// `fan_out_secs` without `group = "commit"` is inert (validation says
    /// so), which is why the mode decides here and not the key's presence.
    pub fn commit_group_window(&self) -> Option<u64> {
        match self.group {
            GroupMode::Run => None,
            GroupMode::Commit => Some(self.fan_out_secs.unwrap_or(DEFAULT_FAN_OUT_SECS)),
        }
    }

    /// The workflows every commit group must hold, or none when one row is one
    /// run.
    ///
    /// ⚠️ Empty under `group = "run"`, where `expect` is inert (validation says
    /// so): a row there is one run of one workflow, and demanding a second
    /// workflow of it would read every row dead.
    pub fn expected_workflows(&self) -> &[String] {
        match self.group {
            GroupMode::Run => &[],
            GroupMode::Commit => &self.expect,
        }
    }
}

/// The workflow FILE an `expect` entry or a run's `path` names.
///
/// `ci.yml`, `.github/workflows/ci.yml` and `.github/workflows/ci.yml@main`
/// are all `ci.yml`. GitHub loads workflow files only from directly inside
/// `.github/workflows/`, so the last segment is unique in a repository, and an
/// entry written as the whole path means the same file as one written short.
/// An `@ref` suffix is dropped defensively: the reusable-workflow references
/// GitHub reports carry one on the same kind of path, and whether a run's own
/// `path` ever does has not been measured. Stripping it costs nothing if not.
pub fn workflow_file(path: &str) -> &str {
    let path = path.trim();
    let path = path.split_once('@').map_or(path, |(file, _)| file);
    path.rsplit('/').next().unwrap_or(path).trim()
}

/// The commit-group window when `fan_out_secs` is absent.
///
/// Measured on real pushes, the several runs one push starts are created in the
/// SAME second; the window only has to absorb GitHub's own dispatch jitter.
/// Ninety seconds is generous for that and still far short of the gap that
/// matters on the other side: two `schedule` runs on an unchanged branch head
/// are an hour or a day apart, and they must stay two rows. A run that lands
/// later than this (a slow `workflow_dispatch`, a re-push of the same commit)
/// starts a row of its own, which is the safe way to be wrong.
pub const DEFAULT_FAN_OUT_SECS: u64 = 90;

/// What one row of a GitHub watch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupMode {
    /// One workflow run is one row.
    #[default]
    Run,
    /// Every run of one commit and event, created within `fan_out_secs` of the
    /// newest, is one row, and each run is one of its bridges.
    Commit,
}

impl GroupMode {
    /// True for the default, [`GroupMode::Run`].
    pub fn is_run(&self) -> bool {
        matches!(self, GroupMode::Run)
    }

    /// The config spelling, `run` or `commit`.
    pub fn as_str(&self) -> &'static str {
        match self {
            GroupMode::Run => "run",
            GroupMode::Commit => "commit",
        }
    }
}

/// A project, addressed either by numeric id or by namespace path.
///
/// ⚠️ Which forms are usable depends on the account's provider: GitLab takes
/// either, and GitHub takes only `owner/repo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ProjectRef {
    /// Numeric project id, the cheapest and most stable form.
    Id(u64),
    /// `group/subgroup/project`, URL-encoded before use.
    Path(String),
}

impl ProjectRef {
    /// The path segment to interpolate into a GitLab API URL.
    pub fn url_segment(&self) -> String {
        self.url_segment_for(Provider::Gitlab)
    }

    /// The path segment to interpolate into `provider`'s API URL.
    ///
    /// ⛔ **The slash is the difference, and it is easy to miss because both
    /// forms look right.** GitLab's `/projects/{id}` takes one opaque segment,
    /// so a path is percent-encoded whole into `group%2Fproject`. GitHub's
    /// `/repos/{owner}/{repo}` is two segments, so its slash must survive;
    /// encoding it gives a 404 that reads like a missing repository. Each
    /// segment is still encoded on its own, so nothing can break out of one.
    ///
    /// A numeric id renders as itself for both, because a total function is
    /// easier to reason about than one that can fail; GitHub has no
    /// `/repos/<id>` endpoint, and it is
    /// [`crate::config::validate`] and the GitHub client that refuse it, each
    /// with a message saying what to write instead.
    pub fn url_segment_for(&self, provider: Provider) -> String {
        match (self, provider) {
            (ProjectRef::Id(id), _) => id.to_string(),
            (ProjectRef::Path(p), Provider::Gitlab) => urlencoding::encode(p).into_owned(),
            (ProjectRef::Path(p), Provider::Github) => p
                .split('/')
                .map(|segment| urlencoding::encode(segment).into_owned())
                .collect::<Vec<_>>()
                .join("/"),
        }
    }
}

impl std::fmt::Display for ProjectRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectRef::Id(id) => write!(f, "{id}"),
            ProjectRef::Path(p) => f.write_str(p),
        }
    }
}

/// Whether a watch drives the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Contributes to the tray icon state and may notify.
    #[default]
    Primary,
    /// Contributes rows to the popover only. Never touches the icon, never
    /// notifies. This is what keeps an hourly schedule that is red by design
    /// out of the tray.
    Secondary,
}

impl Role {
    /// True for [`Role::Primary`].
    pub fn is_primary(&self) -> bool {
        matches!(self, Role::Primary)
    }
}

/// How many pipeline rows a watch contributes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowConfig {
    /// Hard ceiling on rows for this watch.
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
    /// How many already-settled pipelines to keep, on top of every unsettled
    /// one. Unsettled pipelines are always shown.
    #[serde(default = "default_settled")]
    pub settled: usize,
    /// Which jobs this watch's rows list, overriding `ui.jobs` for this watch
    /// alone. Absent means "whatever `ui.jobs` says"; see
    /// [`Watch::effective_jobs`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<JobsMode>,
}

impl Default for ShowConfig {
    fn default() -> Self {
        Self {
            max_rows: default_max_rows(),
            settled: default_settled(),
            jobs: None,
        }
    }
}

/// Which jobs a pipeline row lists when it is expanded.
///
/// Presentation only: the view model carries every job either way, so flipping
/// the mode redraws from what is already held instead of waiting for a fetch,
/// and the verdict never depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobsMode {
    /// Only the jobs the verdict names: failures, warnings and gates.
    Failures,
    /// Every job, parent and child, grouped by stage. The 0.1.0 default.
    #[default]
    All,
}

impl JobsMode {
    /// The config spelling, `failures` or `all`.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobsMode::Failures => "failures",
            JobsMode::All => "all",
        }
    }
}

fn default_max_rows() -> usize {
    5
}
fn default_settled() -> usize {
    1
}

/// Poll intervals, in seconds.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollConfig {
    /// Interval used while anything relevant is unsettled. The setup wizard
    /// starts a GitHub watch at 30, because a GitHub token gets 5,000
    /// requests an hour.
    #[serde(default = "default_live_secs")]
    pub live_secs: u64,
    /// Interval used when everything has settled. The setup wizard starts a
    /// GitHub watch at 120.
    #[serde(default = "default_idle_secs")]
    pub idle_secs: u64,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            live_secs: default_live_secs(),
            idle_secs: default_idle_secs(),
        }
    }
}

/// Five seconds while anything is live: "real time" for a desktop app is fast
/// polling, because GitLab offers no push channel one can use (webhooks need a
/// public inbound URL, and the web UI's live channel is internal).
///
/// What that costs, per watch and per live tick
/// ([`crate::poll::planner::live_tick_requests`]):
///
/// ```text
/// 1 list
/// + 2 per live or changed pipeline (/jobs and /bridges)
/// + 1 per dived child that is live or moved (/jobs, in the child's project)
/// ```
///
/// An idle watch costs only its list. Measured on the shipped example's
/// estate, a push pipeline with four children plus two secondary watches is
/// 9 requests a tick, so 108 a minute at 5 s, against the 2,000 a minute
/// GitLab.com allows authenticated API traffic per user today.
///
/// ⚠️ GitLab.com has published PROPOSED per-plan limits, not in effect as of
/// 2026-09-18, whose Free burst limit is 100 a minute (Premium 1,250,
/// Ultimate 2,000). Under those, this estate at 5 s would exceed a Free
/// account's budget while a pipeline runs; the poller would take the 429s and
/// back off. `planner.rs`'s `the_live_tick_budget_matches_what_is_really_sent`
/// holds the arithmetic to what the transport really sends.
fn default_live_secs() -> u64 {
    5
}
fn default_idle_secs() -> u64 {
    60
}

/// Which trigger jobs to walk into, or on a GitHub commit group, which
/// workflow runs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiveConfig {
    /// Glob over trigger-job names, or workflow names on a GitHub commit
    /// group. `"*"` dives into all of them; `""` into none, which turns the
    /// watch into a cheap one-request-per-tick row.
    #[serde(default = "default_dive_bridges")]
    pub bridges: String,
    /// Trigger-job name globs to skip even when `bridges` matches them.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// How many levels of child pipeline to walk. `1` is the parent's own
    /// children. GitLab only: GitHub runs do not nest.
    #[serde(default = "default_depth")]
    pub depth: u8,
    /// Restrict diving to bridges in a given state, e.g. `"failed"`. This is
    /// how a noisy secondary watch stays cheap until something breaks.
    #[serde(default)]
    pub only_when: Option<String>,
}

impl Default for DiveConfig {
    fn default() -> Self {
        Self {
            bridges: default_dive_bridges(),
            exclude: Vec::new(),
            depth: default_depth(),
            only_when: None,
        }
    }
}

fn default_dive_bridges() -> String {
    "*".to_string()
}
fn default_depth() -> u8 {
    1
}

/// What a secondary failure does to an otherwise successful deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    /// Deployed, but the icon says so with a warning overlay.
    #[default]
    Downgrade,
    /// Treat it as an outright failure.
    Fail,
    /// Do not let it affect the icon at all.
    Ignore,
}

/// How a job's class is forced, regardless of its raw status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobOverride {
    /// A manual job that should read as a gate rather than as missing work.
    Gate,
    /// Demote a blocking failure to a warning.
    Warning,
    /// Promote an allow-failure failure to blocking.
    Blocking,
    /// Hide the job from every verdict and from the rows.
    Ignore,
}

/// Ordered `pattern -> class` overrides.
///
/// Order is the file's order, because the rule is first match wins; a
/// [`std::collections::BTreeMap`] would silently re-sort the user's intent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobOverrides(pub Vec<(String, JobOverride)>);

impl JobOverrides {
    /// The entries, in the order they were written.
    pub fn entries(&self) -> &[(String, JobOverride)] {
        &self.0
    }

    /// True when no overrides are configured.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for JobOverrides {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for JobOverrides {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = JobOverrides;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a table of job-name patterns to gate|warning|blocking|ignore")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, JobOverride>()? {
                    out.push((k, v));
                }
                Ok(JobOverrides(out))
            }
        }
        d.deserialize_map(V)
    }
}

impl JsonSchema for JobOverrides {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "JobOverrides".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let value = generator.subschema_for::<JobOverride>();
        schemars::json_schema!({
            "type": "object",
            "description": "Job-name patterns (literal, or \"re:<regex>\") mapped to a forced class. Evaluated in file order; the first match wins.",
            "additionalProperties": value,
        })
    }
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

/// Which events notify, and what they say.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// A deploy marker succeeded.
    #[serde(default = "yes")]
    pub deployed: bool,
    /// A new blocking failure appeared, or a bridge lost its child.
    #[serde(default = "yes")]
    pub blocking_failure: bool,
    /// A pipeline reached a settled state.
    #[serde(default = "yes")]
    pub finished: bool,
    /// A pipeline started.
    #[serde(default)]
    pub started: bool,
    /// A bridge parked at a manual gate.
    #[serde(default)]
    pub gate_opened: bool,
    /// MiniJinja template for the notification title.
    #[serde(default = "default_title")]
    pub title: String,
    /// MiniJinja template for the notification body.
    #[serde(default = "default_body")]
    pub body: String,
    /// Where clicking the notification goes.
    #[serde(default)]
    pub click: ClickTarget,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            deployed: true,
            blocking_failure: true,
            finished: true,
            started: false,
            gate_opened: false,
            title: default_title(),
            body: default_body(),
            click: ClickTarget::default(),
        }
    }
}

fn yes() -> bool {
    true
}
fn default_title() -> String {
    "{{watch.id}} {{sha7}}: {{state}}".to_string()
}
/// The default body.
///
/// ⚠ The second argument to `default` is load-bearing. MiniJinja follows Jinja2:
/// `default(x)` substitutes only for an **undefined** value, and
/// `failures | join(', ')` on an empty list is the defined empty string. Without
/// `, true` a green pipeline notifies with a blank body. Measured, not assumed.
fn default_body() -> String {
    "{{failures | join(', ') | default('all green', true)}}".to_string()
}

/// Where a notification click lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClickTarget {
    /// The first failing job, falling back to the pipeline when green.
    #[default]
    FirstFailureOrPipeline,
    /// Always the pipeline.
    Pipeline,
    /// The deploy marker job, falling back to the pipeline.
    MarkerJob,
    /// Not clickable.
    None,
}

// ---------------------------------------------------------------------------
// Icon, verdict script, UI, log
// ---------------------------------------------------------------------------

/// Tray icon appearance.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IconConfig {
    /// `auto` is a template image on macOS and the coloured set elsewhere;
    /// `template` forces a macOS template image; `color` forces the coloured set.
    #[serde(default = "default_icon_mode")]
    pub mode: String,
    /// `builtin`, or a directory of PNG overrides named for the glyph a state
    /// resolves to: `<glyph>.png`, or `<glyph>@1x.png`. A glyph the directory
    /// does not carry falls back to the builtin, so a theme may replace one
    /// icon without shipping all of them.
    #[serde(default = "default_icon_theme")]
    pub theme: String,
    /// Per-state glyph names, for themes that offer alternatives.
    #[serde(default)]
    pub states: BTreeMap<String, String>,
}

impl Default for IconConfig {
    fn default() -> Self {
        Self {
            mode: default_icon_mode(),
            theme: default_icon_theme(),
            states: BTreeMap::new(),
        }
    }
}

fn default_icon_mode() -> String {
    "auto".to_string()
}
fn default_icon_theme() -> String {
    "builtin".to_string()
}

/// Optional Rhai hook replacing the built-in icon-state rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct VerdictConfig {
    /// Path to a `.rhai` script. `~` is expanded.
    pub script: Option<String>,
    /// Inline script, which takes precedence over `script`. Mostly for tests
    /// and for the GUI's script editor preview.
    pub script_source: Option<String>,
}

/// Window and presentation settings. The core reads only `jobs`, to put each
/// watch's effective mode on the view model; the rest are the GUI shell's, and
/// they live here so the config file stays the one source.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UiConfig {
    /// Popover geometry and behaviour.
    pub popover: PopoverConfig,
    /// Path to a CSS file layered over the built-in theme.
    pub theme_css: String,
    /// Register bridgewatch as a login item.
    pub launch_at_login: bool,
    /// Which jobs an expanded pipeline row lists: `all` (every job, grouped by
    /// stage) or `failures` (only what the verdict names). A watch may override
    /// it with `show.jobs`.
    pub jobs: JobsMode,
}

/// Popover geometry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PopoverConfig {
    /// Width in logical pixels.
    #[serde(default = "default_popover_width")]
    pub width: u32,
    /// Maximum height in logical pixels before the list scrolls.
    #[serde(default = "default_popover_height")]
    pub max_height: u32,
    /// Dismiss the popover when it loses focus.
    #[serde(default = "yes")]
    pub hide_on_blur: bool,
}

impl Default for PopoverConfig {
    fn default() -> Self {
        Self {
            width: default_popover_width(),
            max_height: default_popover_height(),
            hide_on_blur: true,
        }
    }
}

fn default_popover_width() -> u32 {
    440
}
fn default_popover_height() -> u32 {
    720
}

/// Logging and the bounded request ring the debug pane reads.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// `error` | `warn` | `info` | `debug` | `trace` | `off`. `RUST_LOG` wins
    /// when set.
    #[serde(default = "default_log_level")]
    pub level: String,
    /// How many recent HTTP requests to keep for the debug pane.
    #[serde(default = "default_keep_requests")]
    pub keep_requests: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            keep_requests: default_keep_requests(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_keep_requests() -> usize {
    50
}
