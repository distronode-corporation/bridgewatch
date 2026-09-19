//! Loading, validating and editing `config.toml`.
//!
//! The file is the source of truth. The GUI exposes the same keys and writes
//! them back through [`edit`], which goes via `toml_edit` so comments, key order
//! and `[[watches]]` order survive a round trip.

pub mod edit;
pub mod schema;

use std::ops::Range;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};
use regex::Regex;

pub use schema::*;

/// Environment variable checked before the platform config directory.
pub const CONFIG_ENV: &str = "BRIDGEWATCH_CONFIG";

/// Anything that can go wrong loading a configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid TOML, or does not match the schema.
    #[error("{path}: {message}")]
    Parse {
        /// The path that failed.
        path: PathBuf,
        /// Human-readable description, including the offending line when TOML
        /// gave us one.
        message: String,
        /// Byte range of the problem in the source, when known.
        span: Option<Range<usize>>,
    },
    /// No config path could be determined and none was supplied.
    #[error("no configuration file found: pass --config, set {CONFIG_ENV}, or create {0}")]
    NotFound(PathBuf),
    /// The file parsed but does not describe a workable setup.
    #[error("{} problem(s) in {path}", diagnostics.iter().filter(|d| d.severity == Severity::Error).count())]
    Invalid {
        /// The path that failed.
        path: PathBuf,
        /// Every problem found, not just the first.
        diagnostics: Vec<Diagnostic>,
    },
    /// A pattern in the file is not a valid regex or glob.
    #[error("{0}")]
    Pattern(String),
}

/// How badly a [`Diagnostic`] should be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The configuration cannot be used as written.
    Error,
    /// Usable, but worth knowing about.
    Warning,
}

/// One problem found in a configuration file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Diagnostic {
    /// Error or warning.
    pub severity: Severity,
    /// Dotted path to the offending key, e.g. `watches.1.account`.
    pub path: String,
    /// What is wrong, and where practical what to do about it.
    pub message: String,
    /// Byte range in the source file, when it could be located.
    pub span: Option<Range<usize>>,
}

impl Diagnostic {
    fn error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            path: path.into(),
            message: message.into(),
            span: None,
        }
    }

    fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            path: path.into(),
            message: message.into(),
            span: None,
        }
    }

    /// The 1-based line and column of this diagnostic in `raw`.
    pub fn line_col(&self, raw: &str) -> Option<(usize, usize)> {
        Some(line_col(raw, self.span.as_ref()?.start))
    }
}

/// The 1-based line and column of a byte offset in `raw`.
///
/// Public because a [`ConfigError::Parse`] carries a raw span and no
/// `Diagnostic` to hang it on: a TOML syntax error is the one problem a user
/// most needs a line number for, and `bridgewatch config validate` printed the
/// message without one.
pub fn line_col(raw: &str, offset: usize) -> (usize, usize) {
    let start = offset.min(raw.len());
    // Never split a character: a span that lands mid-UTF-8 would panic on the
    // slice, and a config file with an accented comment is ordinary.
    let start = (0..=start)
        .rev()
        .find(|i| raw.is_char_boundary(*i))
        .unwrap_or(0);
    let before = &raw[..start];
    let line = before.matches('\n').count() + 1;
    let col = before
        .rsplit('\n')
        .next()
        .map(|s| s.chars().count())
        .unwrap_or(0)
        + 1;
    (line, col)
}

/// A configuration file that has been read, parsed and checked.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The typed configuration.
    pub config: Config,
    /// Where it came from.
    pub path: PathBuf,
    /// The exact bytes on disk, kept so [`edit`] can round-trip them and so
    /// diagnostics can be reported with line numbers.
    pub raw: String,
    /// Warnings. Errors are returned as [`ConfigError::Invalid`] instead.
    pub warnings: Vec<Diagnostic>,
}

/// The default config path: `<config dir>/bridgewatch/config.toml`.
pub fn default_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("bridgewatch")
        .join("config.toml")
}

/// Resolve which file to read: `--config`, then `$BRIDGEWATCH_CONFIG`, then the
/// platform config directory.
///
/// The returned path is not guaranteed to exist; [`load`] reports that.
pub fn resolve_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return expand_tilde(p);
    }
    if let Ok(v) = std::env::var(CONFIG_ENV)
        && !v.is_empty()
    {
        return expand_tilde(Path::new(&v));
    }
    default_path()
}

/// Expand a leading `~` against the home directory.
pub fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    p.to_path_buf()
}

/// Read and validate a configuration file.
pub fn load(path: &Path) -> Result<Loaded, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::NotFound(path.to_path_buf()));
    }
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_str(&raw, path)
}

/// Validate a configuration that is already in memory. Used by the tests, by
/// `config validate` on stdin, and by the GUI's live preview of an edit.
pub fn parse_str(raw: &str, path: &Path) -> Result<Loaded, ConfigError> {
    let (mut config, mut diagnostics) = parse_lenient(raw, path)?;

    // ⚠ `toml` hands a table's entries to serde in sorted order, not document
    // order. `[watches.jobs]` is first-match-wins, so accepting that would
    // silently apply the wrong override: in the shipped example it would put
    // `kics-iac-sast` ahead of `re:^verify:web_coverage_full`. The order is
    // recovered from the document, which is the only place it survives.
    restore_job_override_order(&mut config, raw);

    diagnostics.extend(validate(&config));
    attach_spans(&mut diagnostics, raw);

    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(ConfigError::Invalid {
            path: path.to_path_buf(),
            diagnostics,
        });
    }

    Ok(Loaded {
        config,
        path: path.to_path_buf(),
        raw: raw.to_string(),
        warnings: diagnostics,
    })
}

/// How many unknown keys one file may carry before we stop being helpful and
/// hand back the parse error. A file with more than this is not a config that
/// drifted, it is a file that is not a bridgewatch config.
const MAX_UNKNOWN_KEYS: usize = 32;

/// Parse, treating an unknown key as a warning rather than as a fatal error.
///
/// ⛔ Every struct in [`schema`] carries `deny_unknown_fields`, so a single
/// unrecognised key used to fail the whole load: a config written by a newer
/// bridgewatch — or edited by hand with a key that moved — took the app down
/// to "invalid configuration" rather than ignoring the one line it could not
/// use. Unknown keys are now stripped one at a time and reported, which keeps
/// serde as the authority on what "unknown" means (a hand-written list would go
/// stale the first time a key was added) and keeps the exact "expected one of"
/// message it produces.
///
/// ⚠ The returned diagnostics carry a dotted PATH and no span: the offending
/// key is stripped from a working copy whose byte offsets no longer match the
/// file, so [`attach_spans`] resolves them against the original text instead.
/// Everything else — a syntax error, an unknown enum variant, a value of the
/// wrong type — is still fatal, because bridgewatch cannot guess what was meant.
fn parse_lenient(raw: &str, path: &Path) -> Result<(Config, Vec<Diagnostic>), ConfigError> {
    let mut text = raw.to_string();
    let mut diagnostics = Vec::new();

    for _ in 0..=MAX_UNKNOWN_KEYS {
        let error = match toml::from_str::<Config>(&text) {
            Ok(config) => return Ok((config, diagnostics)),
            Err(e) => e,
        };
        let fatal = || ConfigError::Parse {
            path: path.to_path_buf(),
            message: error.message().to_string(),
            // The span is only meaningful against the text that produced it, so
            // report it only while nothing has been stripped.
            span: diagnostics.is_empty().then(|| error.span()).flatten(),
        };
        if !error.message().starts_with("unknown field") {
            return Err(fatal());
        }
        let Some(span) = error.span() else {
            return Err(fatal());
        };
        let Some((stripped, key_path)) = strip_key_at(&text, span.start) else {
            return Err(fatal());
        };
        diagnostics.push(Diagnostic::warning(
            key_path,
            format!(
                "{} — ignored. An unrecognised key is a warning rather than an error so a \
                 file written for a newer bridgewatch still loads here; check the spelling \
                 if you meant one of the keys above",
                error.message()
            ),
        ));
        text = stripped;
    }
    Err(ConfigError::Parse {
        path: path.to_path_buf(),
        message: format!(
            "more than {MAX_UNKNOWN_KEYS} unrecognised keys; this does not look like a bridgewatch config"
        ),
        span: None,
    })
}

/// Remove the key whose name covers `offset`, returning the new text and the
/// dotted path of what was removed.
fn strip_key_at(text: &str, offset: usize) -> Option<(String, String)> {
    let doc = text.parse::<toml_edit::Document<String>>().ok()?;
    let mut prefix = Vec::new();
    let segments = key_path_at(doc.as_table(), offset, &mut prefix)?;

    // Re-quoted per segment, because a key may itself contain a dot and the
    // editor's path grammar is the one place that is already settled.
    let quoted: Vec<String> = segments
        .iter()
        .map(|s| edit::quote_path_segment(s))
        .collect();
    let mut editor = edit::ConfigEditor::new(text).ok()?;
    editor.unset(&quoted.join(".")).ok()?;
    Some((editor.to_toml(), segments.join(".")))
}

/// The dotted path of the key whose NAME covers `offset`.
///
/// Every container is walked rather than only the one whose span covers the
/// offset: an implicit table (`[accounts.gl]` with no `[accounts]` header) has
/// no span of its own, so descending by span alone stops at exactly the shape
/// this has to see through.
fn key_path_at(
    table: &toml_edit::Table,
    offset: usize,
    prefix: &mut Vec<String>,
) -> Option<Vec<String>> {
    for (name, item) in table.iter() {
        if covers(table.key(name).and_then(|k| k.span()), offset) {
            let mut found = prefix.clone();
            found.push(name.to_string());
            return Some(found);
        }
        prefix.push(name.to_string());
        let found = key_path_in_item(item, offset, prefix);
        prefix.pop();
        if found.is_some() {
            return found;
        }
    }
    None
}

fn key_path_in_item(
    item: &toml_edit::Item,
    offset: usize,
    prefix: &mut Vec<String>,
) -> Option<Vec<String>> {
    match item {
        toml_edit::Item::Table(t) => key_path_at(t, offset, prefix),
        toml_edit::Item::ArrayOfTables(a) => {
            for (index, t) in a.iter().enumerate() {
                prefix.push(index.to_string());
                let found = key_path_at(t, offset, prefix);
                prefix.pop();
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => {
            key_path_in_inline(t, offset, prefix)
        }
        _ => None,
    }
}

fn key_path_in_inline(
    table: &toml_edit::InlineTable,
    offset: usize,
    prefix: &mut Vec<String>,
) -> Option<Vec<String>> {
    for (name, value) in table.iter() {
        if covers(table.key(name).and_then(|k| k.span()), offset) {
            let mut found = prefix.clone();
            found.push(name.to_string());
            return Some(found);
        }
        if let toml_edit::Value::InlineTable(inner) = value {
            prefix.push(name.to_string());
            let found = key_path_in_inline(inner, offset, prefix);
            prefix.pop();
            if found.is_some() {
                return found;
            }
        }
    }
    None
}

fn covers(span: Option<Range<usize>>, offset: usize) -> bool {
    span.is_some_and(|s| s.contains(&offset))
}

/// Re-order each watch's `[watches.jobs]` entries to match the file.
///
/// See the call site: serde receives them sorted, and the rule is first match
/// wins, so the file's order is the user's intent and has to be recovered.
fn restore_job_override_order(config: &mut Config, raw: &str) {
    let Ok(doc) = raw.parse::<toml_edit::Document<String>>() else {
        return;
    };
    let Some(watches) = doc.get("watches").and_then(|w| w.as_array_of_tables()) else {
        return;
    };
    for (index, table) in watches.iter().enumerate() {
        let Some(watch) = config.watches.get_mut(index) else {
            break;
        };
        let order: Vec<String> = match table.get("jobs") {
            Some(toml_edit::Item::Table(t)) => t.iter().map(|(k, _)| k.to_string()).collect(),
            Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) => {
                t.iter().map(|(k, _)| k.to_string()).collect()
            }
            _ => continue,
        };
        watch
            .jobs
            .0
            .sort_by_key(|(key, _)| order.iter().position(|k| k == key).unwrap_or(usize::MAX));
    }
}

/// Check a parsed configuration and return **every** problem, not just the
/// first, so one pass of fixes clears the file.
pub fn validate(config: &Config) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    if config.accounts.is_empty() {
        out.push(Diagnostic::error(
            "accounts",
            "no accounts defined; add an [accounts.<name>] table",
        ));
    }
    if config.watches.is_empty() {
        out.push(Diagnostic::warning(
            "watches",
            "no watches defined; bridgewatch will show an empty popover",
        ));
    }

    for (name, account) in &config.accounts {
        let base = format!("accounts.{name}");
        // ⛔ An ERROR, not a warning, and it is deliberate that the file will
        // not load. The alternative is a watch that silently shows nothing, or
        // worse, a GitLab client aimed at GitHub's API answering 404 for every
        // project. The parser accepts the value so the diagnostic can name it;
        // the phase that adds the client deletes these four lines.
        if account.provider == Provider::Github {
            out.push(Diagnostic::error(
                format!("{base}.provider"),
                "GitHub support is not implemented yet: this build can only talk to GitLab. \
                 Remove the account, or set provider = \"gitlab\"",
            ));
        }
        if account.base_url.trim().is_empty() {
            out.push(Diagnostic::error(
                format!("{base}.base_url"),
                "base_url is empty",
            ));
        } else if !account.base_url.starts_with("http://")
            && !account.base_url.starts_with("https://")
        {
            out.push(Diagnostic::error(
                format!("{base}.base_url"),
                format!(
                    "base_url must start with http:// or https:// (found {:?})",
                    account.base_url
                ),
            ));
        }
        // ⛔ H1's other half. The credential is a request HEADER, so a plain
        // HTTP base URL puts the token on the wire in clear for every proxy on
        // the path to read, and the file accepted it with nothing said.
        // Loopback is exempt deliberately: a token sent to 127.0.0.1 never
        // leaves the machine, and a local instance is a normal way to develop.
        if account.base_url.starts_with("http://") && !is_loopback_url(&account.base_url) {
            out.push(Diagnostic::warning(
                format!("{base}.base_url"),
                "base_url is plain http://, so the token travels unencrypted in a request \
                 header and any proxy on the path can read it; use https:// unless this \
                 instance is on loopback",
            ));
        }
        if account.base_url.ends_with('/') {
            out.push(Diagnostic::warning(
                format!("{base}.base_url"),
                "base_url ends with a slash; it is joined to api_path verbatim",
            ));
        }
        // ⚠️ An EMPTY api_path is right for github.com, whose paths are
        // `/repos/...` on an `api.` host with no prefix at all; only GitHub
        // Enterprise Server inserts one (`/api/v3`). Empty is still wrong for
        // GitLab, where the prefix is what selects the API.
        let api_path_ok = account.api_path.starts_with('/')
            || (account.provider == Provider::Github && account.api_path.is_empty());
        if !api_path_ok {
            out.push(Diagnostic::error(
                format!("{base}.api_path"),
                "api_path must start with a slash",
            ));
        }
        if account.timeout_secs == 0 {
            out.push(Diagnostic::error(
                format!("{base}.timeout_secs"),
                "timeout_secs must be > 0",
            ));
        }
        match &account.token {
            TokenSource::Command(argv) => {
                if argv.is_empty() {
                    out.push(Diagnostic::error(
                        format!("{base}.token"),
                        "command token source is an empty list",
                    ));
                } else {
                    out.push(Diagnostic::warning(
                        format!("{base}.token"),
                        format!(
                            "runs a program to get the token: {:?}. bridgewatch will execute this on every token refresh",
                            argv.join(" ")
                        ),
                    ));
                }
            }
            TokenSource::Env(var) if var.trim().is_empty() => {
                out.push(Diagnostic::error(
                    format!("{base}.token"),
                    "env token source names an empty variable",
                ));
            }
            TokenSource::Keyring { service, .. } if service.trim().is_empty() => {
                out.push(Diagnostic::error(
                    format!("{base}.token"),
                    "keyring token source has an empty service",
                ));
            }
            TokenSource::Own(false) => {
                out.push(Diagnostic::error(
                    format!("{base}.token"),
                    "token = { own = false } selects no source; use own = true or another form",
                ));
            }
            _ => {}
        }
    }

    // ⚠ Everything from here to the watches loop is a value the file accepts
    // because its TYPE is right — a String, a usize, a map key — while its
    // CONTENT names something bridgewatch has never heard of. Each one used to
    // load in silence and then behave as though the key had not been written:
    // a misspelt glyph showed the question-mark icon, a misspelt source matched
    // no pipeline at all. They are warnings rather than errors on purpose, so a
    // file written for a newer build still runs on an older one.
    if !["auto", "template", "color"].contains(&config.icon.mode.as_str()) {
        out.push(Diagnostic::warning(
            "icon.mode",
            format!(
                "{:?} is not an icon mode bridgewatch knows (auto, template, color); \
                 the built-in default is used instead",
                config.icon.mode
            ),
        ));
    }
    let builtin_theme = config.icon.theme.trim().is_empty() || config.icon.theme == "builtin";
    for (state, glyph) in &config.icon.states {
        // A glyph name the builtin set lacks falls back to the question mark,
        // so a typo for `failed` drew "unknown" over a red pipeline. Only said
        // for the builtin theme: a theme directory may ship any name it likes,
        // and checking its files belongs to whoever draws them.
        if builtin_theme && !BUILTIN_GLYPHS.contains(&glyph.as_str()) {
            out.push(Diagnostic::warning(
                format!("icon.states.{state}"),
                format!(
                    "{glyph:?} is not a builtin glyph, so this state draws the \
                     question-mark icon instead. The builtin glyphs are {}",
                    BUILTIN_GLYPHS.join(", ")
                ),
            ));
        }
        if crate::verdict::IconState::parse(state).is_none() {
            out.push(Diagnostic::warning(
                format!("icon.states.{state}"),
                format!(
                    "{state:?} is not an icon state, so this glyph is never used. The states \
                     are unknown, failed, deployed_with_failure, deployed, running, canceled, \
                     parked_gate and succeeded_no_deploy"
                ),
            ));
        }
    }
    if !LOG_LEVELS.contains(&config.log.level.as_str()) {
        out.push(Diagnostic::warning(
            "log.level",
            format!(
                "{:?} is not a log level ({})",
                config.log.level,
                LOG_LEVELS.join(", ")
            ),
        ));
    }
    if config.ui.popover.width == 0 || config.ui.popover.max_height == 0 {
        out.push(Diagnostic::warning(
            "ui.popover",
            "a width or max_height of 0 leaves the popover with nothing to draw in",
        ));
    }

    let mut seen_ids: Vec<&str> = Vec::new();
    let mut primaries = 0usize;
    for (i, watch) in config.watches.iter().enumerate() {
        let base = format!("watches.{i}");
        if watch.id.trim().is_empty() {
            out.push(Diagnostic::error(format!("{base}.id"), "id is empty"));
        } else if seen_ids.contains(&watch.id.as_str()) {
            out.push(Diagnostic::error(
                format!("{base}.id"),
                format!("duplicate watch id {:?}", watch.id),
            ));
        } else {
            seen_ids.push(&watch.id);
        }

        if !config.accounts.contains_key(&watch.account) {
            let known: Vec<&str> = config.accounts.keys().map(String::as_str).collect();
            out.push(Diagnostic::error(
                format!("{base}.account"),
                format!(
                    "unknown account {:?}; defined accounts are [{}]",
                    watch.account,
                    known.join(", ")
                ),
            ));
        }

        match RefMatcher::parse(&watch.ref_pattern) {
            Err(e) => out.push(Diagnostic::error(format!("{base}.ref"), e.to_string())),
            // ⚠ Lo2. `re:` is an ordinary unanchored regex, which is the useful
            // default and the surprising one: `re:main` matches `not-main` and
            // `mainline` too. Said once, here, rather than changed — anchoring
            // it would make every pattern in every existing file mean something
            // new.
            Ok(RefMatcher::Regex(_)) if !has_anchor(&watch.ref_pattern) => {
                out.push(Diagnostic::warning(
                    format!("{base}.ref"),
                    format!(
                        "{:?} is matched unanchored, so it also matches any ref that merely \
                         CONTAINS it; write it as re:^…$ to match the whole ref",
                        watch.ref_pattern
                    ),
                ));
            }
            // ⚠ Lo2's other half, handled the same way: in a ref glob `*` also
            // matches `/`. Only said when a `*` has a `/` after it, which is the
            // shape whose author evidently meant one path segment; a trailing
            // `pf/*` reaching `pf/a/b` is what nearly everyone wants.
            Ok(RefMatcher::Glob(_)) if glob_star_precedes_slash(&watch.ref_pattern) => {
                out.push(Diagnostic::warning(
                    format!("{base}.ref"),
                    format!(
                        "in {:?} each `*` also matches `/`, so it spans path segments \
                         (`*/main` matches `a/b/main`, not only `a/main`); if one segment \
                         is meant, write a regex such as re:^[^/]+/main$",
                        watch.ref_pattern
                    ),
                ));
            }
            Ok(_) => {}
        }

        for (j, source) in watch.sources.iter().enumerate() {
            if !KNOWN_PIPELINE_SOURCES.contains(&source.as_str()) {
                out.push(Diagnostic::warning(
                    format!("{base}.sources.{j}"),
                    format!(
                        "{source:?} is not a pipeline source bridgewatch knows; no pipeline \
                         will match it, and the watch will show nothing"
                    ),
                ));
            }
        }

        // ⚠ A warning, not an error, and the wording is measured rather than
        // assumed: `select_rows` keeps a PRIMARY watch's newest match whatever
        // the trim says, precisely so a zero row count cannot leave the tray
        // `unknown` over a green estate. A secondary watch really does show
        // nothing, which is the part worth saying out loud.
        if watch.show.max_rows == 0 {
            out.push(Diagnostic::warning(
                format!("{base}.show.max_rows"),
                "max_rows is 0, so this watch contributes no rows to the popover; a primary \
                 watch still keeps its newest pipeline so the icon has something to read",
            ));
        }

        for (j, marker) in watch.deploy_markers.iter().enumerate() {
            if let Err(e) = Pattern::parse(marker) {
                out.push(Diagnostic::error(
                    format!("{base}.deploy_markers.{j}"),
                    e.to_string(),
                ));
            }
        }

        for (pattern, _) in watch.jobs.entries() {
            if let Err(e) = Pattern::parse(pattern) {
                out.push(Diagnostic::error(
                    format!("{base}.jobs.{pattern}"),
                    e.to_string(),
                ));
            }
        }

        if !watch.dive.bridges.is_empty()
            && let Err(e) = Glob::new(&watch.dive.bridges)
        {
            out.push(Diagnostic::error(
                format!("{base}.dive.bridges"),
                format!("invalid glob: {e}"),
            ));
        }
        for (j, ex) in watch.dive.exclude.iter().enumerate() {
            if let Err(e) = Glob::new(ex) {
                out.push(Diagnostic::error(
                    format!("{base}.dive.exclude.{j}"),
                    format!("invalid glob: {e}"),
                ));
            }
        }
        if let Some(only) = &watch.dive.only_when {
            let s = Status::from(only.clone());
            if matches!(s, Status::Unknown(_)) {
                out.push(Diagnostic::warning(
                    format!("{base}.dive.only_when"),
                    format!("{only:?} is not a status bridgewatch knows; no bridge will match it"),
                ));
            }
        }

        if watch.poll.live_secs == 0 || watch.poll.idle_secs == 0 {
            out.push(Diagnostic::error(
                format!("{base}.poll"),
                "poll intervals must be > 0",
            ));
        }
        if watch.poll.live_secs > watch.poll.idle_secs {
            out.push(Diagnostic::warning(
                format!("{base}.poll"),
                "live_secs is larger than idle_secs, so a busy pipeline is polled less often than an idle one",
            ));
        }

        if watch.role.is_primary() {
            primaries += 1;
            if watch.deploy_markers.is_empty() {
                out.push(Diagnostic::warning(
                    format!("{base}.deploy_markers"),
                    "primary watch with no deploy markers: it can never report 'deployed', only 'succeeded_no_deploy'",
                ));
            }
        }

        for (j, tpl) in [("title", &watch.notify.title), ("body", &watch.notify.body)] {
            if let Err(e) = minijinja::Environment::new().template_from_str(tpl) {
                out.push(Diagnostic::error(
                    format!("{base}.notify.{j}"),
                    format!("invalid template: {e}"),
                ));
            }
        }
    }

    if primaries > 1 {
        out.push(Diagnostic::warning(
            "watches",
            format!(
                "{primaries} watches are primary; the tray icon is the WORST of their states, \
                 so one red hourly schedule will hold the icon red. Mark all but one \
                 role = \"secondary\" if that is not what you want"
            ),
        ));
    }
    if primaries == 0 && !config.watches.is_empty() {
        out.push(Diagnostic::warning(
            "watches",
            "every watch is secondary, so the tray icon will always read 'unknown'",
        ));
    }

    out
}

/// Every glyph name the builtin icon set ships: the eight state names, which is
/// what a state with no `[icon].states` entry draws, then the eight symbol
/// names a remap may point at.
///
/// ⚠ The PNGs themselves live in the app crate (`src-tauri/icons/tray/`), which
/// the core cannot see. This list is the canonical one: validation reads it,
/// and a test in `src-tauri/src/icons.rs` fails if the shell's `builtin_set!`
/// differs from it.
pub const BUILTIN_GLYPHS: &[&str] = &[
    "unknown",
    "failed",
    "deployed_with_failure",
    "deployed",
    "running",
    "canceled",
    "parked_gate",
    "succeeded_no_deploy",
    "question",
    "octagon",
    "triangle",
    "check",
    "check-outline",
    "arrows",
    "slash",
    "hourglass",
];

/// The `source` values GitLab documents for a pipeline, as of 17.x.
///
/// Used for a warning only: GitLab adds sources (`duo_workflow` and
/// `pipeline_execution_policy_schedule` are both recent), so an unrecognised
/// one must not stop a file loading — it just cannot match anything today.
pub const KNOWN_PIPELINE_SOURCES: &[&str] = &[
    "push",
    "web",
    "trigger",
    "schedule",
    "api",
    "external",
    "pipeline",
    "chat",
    "webide",
    "merge_request_event",
    "external_pull_request_event",
    "parent_pipeline",
    "ondemand_dast_scan",
    "ondemand_dast_validation",
    "security_orchestration_policy",
    "container_registry_push",
    "duo_workflow",
    "pipeline_execution_policy_schedule",
    "unknown",
];

/// The values `[log].level` accepts. Anything else is a warning from
/// [`validate`], and [`log_directive`] falls back to `warn`.
pub const LOG_LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace", "off"];

/// The `tracing` filter directive for a `RUST_LOG` value and a `[log].level`.
///
/// A non-empty `RUST_LOG` wins, whole and unmodified. An empty or all-whitespace
/// one counts as unset, because an exported-but-blank variable is a leftover in a
/// shell profile rather than a request to silence the program. With neither, the
/// level is `warn`.
///
/// ⚠ `[log].level` scopes to `crates`, bridgewatch's own: asking for `debug`
/// should show what bridgewatch is doing, not every frame the window system and
/// the HTTP stack log at that level. A level QUIETER than `warn` (`error`, `off`)
/// applies to everything, since "less, please" means less of all of it.
///
/// Lives here because the CLI and the app both need the same answer and neither
/// can see the other: `bridgewatch-cli`'s `init_tracing` and the shell's
/// `logging::directive` are the two callers, and they differ only in which crate
/// names they pass.
pub fn log_directive(rust_log: Option<&str>, level: Option<&str>, crates: &[&str]) -> String {
    if let Some(env) = rust_log.map(str::trim).filter(|v| !v.is_empty()) {
        return env.to_string();
    }
    let level = level
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| LOG_LEVELS.contains(&l.as_str()))
        .unwrap_or_else(|| "warn".to_string());
    match level.as_str() {
        "warn" | "error" | "off" => level,
        _ => std::iter::once("warn".to_string())
            .chain(crates.iter().map(|name| format!("{name}={level}")))
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// Should the log writer emit ANSI colour?
///
/// ⛔ `tracing_subscriber`'s fmt layer colours by DEFAULT, and it does not look
/// at where it is writing. So `bridgewatch check 2>&1 >/dev/null | head` came
/// out full of `ESC[2m`, and the desktop app wrote the same escapes into the
/// systemd user journal, where they are noise in every `journalctl` line
/// forever.
///
/// ⚠ `no_color` is "present and not empty", which is the `NO_COLOR` convention
/// and deliberately NOT how [`log_directive`] reads `RUST_LOG`. A blank
/// `RUST_LOG` is treated as unset because an exported-but-empty variable is a
/// leftover in a shell profile; a blank `NO_COLOR` is defined by the convention
/// itself to mean nothing at all, so the two disagree on purpose.
///
/// Takes both facts as arguments rather than reading the environment so that
/// the decision is testable without a terminal or a `set_var`. Lives here for
/// the same reason [`log_directive`] does: `bridgewatch-cli`'s `init_tracing`
/// and the shell's `logging::init` both need it and neither can see the other.
pub fn use_ansi(stderr_is_terminal: bool, no_color: Option<&str>) -> bool {
    if no_color.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    stderr_is_terminal
}

/// The one `info` line a shell logs when it has read a configuration.
///
/// `head` says WHICH read this was ("using /path/config.toml", "reloaded
/// /path/config.toml"), and the rest is what somebody debugging "why is my tray
/// wrong" needs before anything else: how much the file describes, and how loud
/// the program will now be.
///
/// ⚠ The directive is part of the line on purpose. `[log].level` is read from
/// the very file being reported, but a non-empty `RUST_LOG` beats it (see
/// [`log_directive`]), and a leftover `export RUST_LOG=` in a shell profile is
/// otherwise invisible: the level asked for and the level in force disagree and
/// nothing says so.
///
/// Lives here for the same reason [`log_directive`] does: the CLI and the app
/// both say it, neither can see the other, and two copies would drift.
pub fn describe_load(head: &str, config: Option<&Config>, directive: &str) -> String {
    match config {
        Some(c) => format!(
            "{head}: {} watch(es), {} account(s), logging {directive}",
            c.watches.len(),
            c.account_count()
        ),
        None => format!("{head}: the file does not load, nothing is watched"),
    }
}

/// Does a `re:` pattern pin either end of the name?
///
/// Deliberately crude: it answers "did the author think about anchoring", not
/// "is this regex anchored", and it is only used to decide whether to say so.
fn has_anchor(pattern: &str) -> bool {
    let body = pattern.strip_prefix("re:").unwrap_or(pattern);
    body.starts_with('^') || body.ends_with('$')
}

/// Does a glob have a `*` with a `/` somewhere after it?
///
/// globset is built with its default `literal_separator = false`, so that `*`
/// crosses the separator; see the call site for why only this shape is said.
fn glob_star_precedes_slash(pattern: &str) -> bool {
    pattern
        .find('*')
        .is_some_and(|star| pattern[star..].contains('/'))
}

/// Is this base URL pointing at this machine?
///
/// ⚠ The host is parsed as an address, never prefix-matched: `127.0.0.1.evil.com`
/// and `localhost.evil.com` are somebody else's machine. A bracketed IPv6 host
/// is split on its `]`, because splitting `[::1]:8080` on `:` leaves `[`.
fn is_loopback_url(base_url: &str) -> bool {
    let rest = base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Userinfo, if anybody wrote one, is not the host.
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
    .to_ascii_lowercase();
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Locate each diagnostic's dotted path in the raw TOML and record its span.
fn attach_spans(diagnostics: &mut [Diagnostic], raw: &str) {
    let Ok(doc) = raw.parse::<toml_edit::Document<String>>() else {
        return;
    };
    for d in diagnostics.iter_mut() {
        d.span = span_for(&doc, &d.path);
    }
}

/// Walk a dotted path through the document and return the tightest span that
/// still exists, so a diagnostic points at the narrowest thing we could find
/// rather than at nothing.
fn span_for(doc: &toml_edit::Document<String>, path: &str) -> Option<Range<usize>> {
    /// Where the walk currently is. A `Table` is not an `Item`, and an inline
    /// table's members are `Value`s, so all three shapes need representing.
    #[derive(Clone, Copy)]
    enum Node<'a> {
        Item(&'a toml_edit::Item),
        Table(&'a toml_edit::Table),
        Value(&'a toml_edit::Value),
    }

    impl<'a> Node<'a> {
        fn span(self) -> Option<Range<usize>> {
            match self {
                Node::Item(i) => i.span(),
                Node::Table(t) => t.span(),
                Node::Value(v) => v.span(),
            }
        }

        fn child(self, segment: &str) -> Option<Node<'a>> {
            match self {
                Node::Table(t) => t.get(segment).map(Node::Item),
                Node::Item(toml_edit::Item::Table(t)) => t.get(segment).map(Node::Item),
                Node::Item(toml_edit::Item::ArrayOfTables(a)) => segment
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| a.get(i))
                    .map(Node::Table),
                Node::Item(toml_edit::Item::Value(v)) | Node::Value(v) => match v {
                    toml_edit::Value::InlineTable(t) => t.get(segment).map(Node::Value),
                    toml_edit::Value::Array(a) => segment
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| a.get(i))
                        .map(Node::Value),
                    _ => None,
                },
                Node::Item(_) => None,
            }
        }
    }

    let root = Node::Item(doc.as_item());
    let mut best = root.span();
    let mut cursor = root;

    for segment in path.split('.') {
        let Some(next) = cursor.child(segment) else {
            break;
        };
        if let Some(s) = next.span() {
            best = Some(s);
        }
        cursor = next;
    }
    best
}

// ---------------------------------------------------------------------------
// Pattern matching
// ---------------------------------------------------------------------------

use crate::status::Status;

/// A literal-or-regex matcher, the shape used for deploy markers and for the
/// keys of `[watches.jobs]`.
#[derive(Debug, Clone)]
pub enum Pattern {
    /// Matches a name exactly.
    Literal(String),
    /// Written `re:<regex>`; matched unanchored, as a regex is expected to be.
    Regex(Regex),
}

impl Pattern {
    /// Parse a pattern. A `re:` prefix selects regex; anything else is literal.
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        match s.strip_prefix("re:") {
            Some(rx) => Regex::new(rx)
                .map(Pattern::Regex)
                .map_err(|e| ConfigError::Pattern(format!("invalid regex {rx:?}: {e}"))),
            None => Ok(Pattern::Literal(s.to_string())),
        }
    }

    /// Does this pattern match a job name?
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Pattern::Literal(l) => l == name,
            Pattern::Regex(r) => r.is_match(name),
        }
    }
}

/// How a watch's `ref` key is matched against a pipeline's ref.
#[derive(Debug, Clone)]
pub enum RefMatcher {
    /// A plain name. This is the only form that can be pushed to the API as
    /// `?ref=`, which is why it is kept distinct.
    Exact(String),
    /// A glob such as `pf/*`, matched client-side.
    Glob(Box<GlobMatcher>),
    /// `re:<regex>`, matched client-side.
    Regex(Regex),
}

impl RefMatcher {
    /// Parse a ref pattern: `re:` prefix is a regex, glob metacharacters make
    /// it a glob, anything else is an exact name.
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        if let Some(rx) = s.strip_prefix("re:") {
            return Regex::new(rx)
                .map(RefMatcher::Regex)
                .map_err(|e| ConfigError::Pattern(format!("invalid ref regex {rx:?}: {e}")));
        }
        if s.contains(['*', '?', '[', '{']) {
            return Glob::new(s)
                .map(|g| RefMatcher::Glob(Box::new(g.compile_matcher())))
                .map_err(|e| ConfigError::Pattern(format!("invalid ref glob {s:?}: {e}")));
        }
        Ok(RefMatcher::Exact(s.to_string()))
    }

    /// Does a pipeline's ref match?
    pub fn matches(&self, r: &str) -> bool {
        match self {
            RefMatcher::Exact(e) => e == r,
            RefMatcher::Glob(g) => g.is_match(r),
            RefMatcher::Regex(rx) => rx.is_match(r),
        }
    }

    /// The exact ref, when the pattern is one. `Some` means the list request
    /// may carry `?ref=`, which is both cheaper and more accurate than paging
    /// recent pipelines and filtering.
    pub fn exact(&self) -> Option<&str> {
        match self {
            RefMatcher::Exact(e) => Some(e),
            _ => None,
        }
    }
}

/// A watch with its patterns compiled once, ready for the verdict engine.
///
/// Compiling on every tick would be wasteful and would turn a bad regex into a
/// runtime surprise rather than a load-time diagnostic.
#[derive(Debug, Clone)]
pub struct WatchRules {
    /// How to match the ref.
    pub ref_matcher: RefMatcher,
    /// Deploy markers, in configuration order.
    pub deploy_markers: Vec<Pattern>,
    /// Job class overrides, in file order; first match wins.
    pub job_overrides: Vec<(Pattern, JobOverride)>,
    /// Which bridges to dive into. `None` when `dive.bridges` is empty, which
    /// means "dive into nothing".
    pub dive: Option<Box<GlobMatcher>>,
    /// Bridges to skip even when `dive` matches.
    pub dive_exclude: Vec<GlobMatcher>,
    /// Only dive when the bridge is in this status.
    pub dive_only_when: Option<Status>,
}

impl WatchRules {
    /// Compile a watch's patterns. Returns the first failure; [`validate`]
    /// reports all of them, and is what a user-facing path should call first.
    pub fn compile(watch: &Watch) -> Result<Self, ConfigError> {
        let dive = if watch.dive.bridges.is_empty() {
            None
        } else {
            Some(Box::new(
                Glob::new(&watch.dive.bridges)
                    .map_err(|e| ConfigError::Pattern(format!("invalid dive glob: {e}")))?
                    .compile_matcher(),
            ))
        };
        let mut dive_exclude = Vec::new();
        for ex in &watch.dive.exclude {
            dive_exclude.push(
                Glob::new(ex)
                    .map_err(|e| ConfigError::Pattern(format!("invalid dive exclude glob: {e}")))?
                    .compile_matcher(),
            );
        }
        Ok(Self {
            ref_matcher: RefMatcher::parse(&watch.ref_pattern)?,
            deploy_markers: watch
                .deploy_markers
                .iter()
                .map(|m| Pattern::parse(m))
                .collect::<Result<_, _>>()?,
            job_overrides: watch
                .jobs
                .entries()
                .iter()
                .map(|(p, o)| Pattern::parse(p).map(|p| (p, *o)))
                .collect::<Result<_, _>>()?,
            dive,
            dive_exclude,
            dive_only_when: watch.dive.only_when.clone().map(Status::from),
        })
    }

    /// The override that applies to a job name, if any. First match wins.
    pub fn job_override(&self, name: &str) -> Option<JobOverride> {
        self.job_overrides
            .iter()
            .find(|(p, _)| p.matches(name))
            .map(|(_, o)| *o)
    }

    /// Is this job name a deploy marker?
    pub fn is_deploy_marker(&self, name: &str) -> bool {
        self.deploy_markers.iter().any(|p| p.matches(name))
    }

    /// Should this bridge be walked into, given its name and current status?
    pub fn should_dive(&self, bridge_name: &str, bridge_status: &Status) -> bool {
        let Some(dive) = &self.dive else {
            return false;
        };
        if !dive.is_match(bridge_name) {
            return false;
        }
        if self.dive_exclude.iter().any(|g| g.is_match(bridge_name)) {
            return false;
        }
        match &self.dive_only_when {
            Some(want) => want == bridge_status,
            None => true,
        }
    }

    /// Does a pipeline's source pass the watch's `sources` filter?
    pub fn source_matches(sources: &[String], source: Option<&str>) -> bool {
        if sources.is_empty() {
            return true;
        }
        match source {
            Some(s) => sources.iter().any(|w| w == s),
            None => false,
        }
    }
}
