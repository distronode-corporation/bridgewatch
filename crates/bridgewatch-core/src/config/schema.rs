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
    /// GitLab instances to talk to, keyed by the name watches refer to.
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

/// One GitLab instance and the credential used against it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// Instance root, without a trailing slash, e.g. `https://gitlab.com`.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// API prefix. Overridable for proxies that mount the API elsewhere.
    #[serde(default = "default_api_path")]
    pub api_path: String,
    /// Where the token comes from. Never the token itself.
    #[serde(default)]
    pub token: TokenSource,
    /// Which header carries the token.
    #[serde(default)]
    pub header: AuthHeader,
    /// Per-request timeout.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// How far the poll interval is allowed to back off after rate limiting.
    #[serde(default)]
    pub rate_limit_backoff: RateLimitBackoff,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_path: default_api_path(),
            token: TokenSource::default(),
            header: AuthHeader::default(),
            timeout_secs: default_timeout_secs(),
            rate_limit_backoff: RateLimitBackoff::default(),
        }
    }
}

fn default_base_url() -> String {
    "https://gitlab.com".to_string()
}
fn default_api_path() -> String {
    "/api/v4".to_string()
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
    /// `Authorization: Bearer <token>`, for OAuth and CI job tokens.
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
    /// Linux Secret Service.
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
                     { env = \"VAR\" }, { command = [\"prog\", \"arg\"] } or { own = true }",
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
                        "token = { } names no source; use keyring, env, command or own",
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
                    other => {
                        return Err(A::Error::custom(format!(
                            "unknown token source {other:?}; use keyring, env, command or own"
                        )));
                    }
                };
                if let Some(extra) = map.next_key::<String>()? {
                    return Err(A::Error::custom(format!(
                        "a token source names exactly one of keyring, env, command or own; \
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
    /// Numeric project id, or a `group/path` which is URL-encoded for you.
    pub project: ProjectRef,
    /// Ref pattern: exact (`main`), glob (`pf/*`) or regex (`re:^release/.*$`).
    #[serde(rename = "ref", default = "default_ref")]
    pub ref_pattern: String,
    /// Pipeline sources to accept. Empty means all of them.
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
}

/// A project, addressed either by numeric id or by namespace path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ProjectRef {
    /// Numeric project id, the cheapest and most stable form.
    Id(u64),
    /// `group/subgroup/project`, URL-encoded before use.
    Path(String),
}

impl ProjectRef {
    /// The path segment to interpolate into an API URL.
    pub fn url_segment(&self) -> String {
        match self {
            ProjectRef::Id(id) => id.to_string(),
            ProjectRef::Path(p) => urlencoding::encode(p).into_owned(),
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
    /// Interval used while anything relevant is unsettled.
    #[serde(default = "default_live_secs")]
    pub live_secs: u64,
    /// Interval used when everything has settled.
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

/// Which trigger jobs to walk into.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiveConfig {
    /// Glob over trigger-job names. `"*"` dives into all of them; `""` into
    /// none, which turns the watch into a cheap one-request-per-tick row.
    #[serde(default = "default_dive_bridges")]
    pub bridges: String,
    /// Trigger-job name globs to skip even when `bridges` matches them.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// How many levels of child pipeline to walk. `1` is the parent's own
    /// children.
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
    /// `error` | `warn` | `info` | `debug` | `trace`. `RUST_LOG` wins when set.
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
