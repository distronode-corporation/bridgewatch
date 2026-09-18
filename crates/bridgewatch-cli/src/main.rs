//! `bridgewatch` — the command-line face of the bridge-aware GitLab monitor.
//!
//! Everything the tray does, without the tray: a one-shot verdict for scripts
//! and CI, config tooling, a headless watch loop, and the fixture recorder the
//! test suite is built from.

#![forbid(unsafe_code)]

mod render;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use bridgewatch_core::client::fixture::ScrubMode;
use bridgewatch_core::client::{FixtureTransport, GitLabClient, RequestRing, ReqwestTransport};
use bridgewatch_core::config::{self, Config, ProjectRef};
use bridgewatch_core::notify::NotifyLedger;
use bridgewatch_core::poll::Poller;
use bridgewatch_core::token::{self, Secret, SystemTokenProvider};
use bridgewatch_core::verdict::{IconState, Snapshot};
use clap::{Args, Parser, Subcommand};

/// Exit codes, a stable and documented set.
///
/// ⛔ The verdict codes and the error codes never overlap. They used to: a
/// config that failed to load came back through `anyhow` as `1`, which is also
/// `failed`, and clap reports a usage error as `2`, which is also
/// `deployed_with_failure`. A script branching on "did it deploy" could not
/// tell a red pipeline from a typo in its own command line. The error codes
/// are the BSD `sysexits.h` ones, which nothing here uses for a verdict.
mod exit {
    /// `check`/`watch`: deployed, or green with nothing to deploy. Every other
    /// command: it did what it was asked.
    pub const OK: i32 = 0;
    /// `check`/`watch`: something failed. `fixture scrub --check`: a fixture
    /// would change.
    pub const FAILED: i32 = 1;
    /// `check`/`watch`: deployed, but something else failed.
    pub const DEPLOYED_WITH_FAILURE: i32 = 2;
    /// `check`/`watch`: still in flight, parked at a gate, or cancelled.
    pub const PENDING: i32 = 3;
    /// `check`/`watch`: no verdict. Nothing matched, or a request failed with
    /// nothing cached to fall back on; the errors are printed.
    pub const UNKNOWN: i32 = 4;
    /// The command line is wrong: clap's own errors, or a `--watch` naming no
    /// configured watch. `EX_USAGE`.
    pub const USAGE: i32 = 64;
    /// Something else failed before there was a verdict: a token that would
    /// not resolve, an unreadable fixture, a failed recording, I/O.
    /// `EX_SOFTWARE`.
    pub const ERROR: i32 = 70;
    /// The configuration file is missing, is not TOML, or does not describe a
    /// workable setup. `EX_CONFIG`.
    pub const CONFIG: i32 = 78;

    /// The table printed at the foot of `--help`, and the one the README
    /// quotes. A test holds it to the constants above.
    pub const HELP: &str = "\
Exit codes:
  0   check/watch: deployed or succeeded_no_deploy; any other command: success
  1   check/watch: failed; fixture scrub --check: a fixture would change
  2   check/watch: deployed_with_failure
  3   check/watch: running, parked_gate or canceled
  4   check/watch: unknown (nothing matched, or a request failed; errors on stderr)
  64  usage error: bad arguments, or --watch names no configured watch
  70  any other error before a verdict (token, fixture, recording, I/O)
  78  the configuration is missing, unreadable or invalid";
}

/// A failure, with the exit code it maps to.
///
/// `?` on an `anyhow::Error` lands on [`exit::ERROR`]; the two places that know
/// better — loading the config and naming a watch — say so explicitly.
#[derive(Debug)]
struct Failure {
    code: i32,
    error: anyhow::Error,
}

impl Failure {
    fn new(code: i32, error: impl Into<anyhow::Error>) -> Self {
        Self {
            code,
            error: error.into(),
        }
    }
}

impl<E: Into<anyhow::Error>> From<E> for Failure {
    fn from(error: E) -> Self {
        Self::new(exit::ERROR, error)
    }
}

type Outcome = std::result::Result<i32, Failure>;

#[derive(Debug, Parser)]
#[command(
    name = "bridgewatch",
    version,
    about = "Bridge-aware GitLab CI monitor: walks trigger jobs into child pipelines and derives a deploy verdict.",
    long_about = None,
    after_help = exit::HELP,
)]
struct Cli {
    #[command(flatten)]
    config: ConfigArgs,
    #[command(subcommand)]
    command: Command,
}

/// ⛔ Declared once, on the top-level parser, with `global = true`: it was
/// flattened into each subcommand instead, so `bridgewatch --config x check`
/// — the order the README shows — was rejected, and so was `--config` on
/// `config schema` and `fixture scrub`, which took no config at all.
#[derive(Debug, Args, Clone)]
struct ConfigArgs {
    /// Path to config.toml. Falls back to $BRIDGEWATCH_CONFIG (an empty value
    /// counts as unset), then the platform config directory.
    //
    // ⚠ Not `env = "BRIDGEWATCH_CONFIG"`: clap reads an empty variable as an
    // empty path and refuses it with a usage error, while the app treats
    // `BRIDGEWATCH_CONFIG=""` as unset. `config::resolve_path` reads the
    // variable itself, so both front ends agree.
    #[arg(long, short, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the current verdict once and exit with a code that says what it is.
    Check(CheckArgs),
    /// Inspect and validate the configuration file.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Record a pipeline into a test fixture directory.
    Fixture {
        #[command(subcommand)]
        action: FixtureAction,
    },
    /// Poll continuously, printing one line per change.
    Watch(WatchArgs),
    /// Print the config.toml the setup wizard would write, from flags.
    ///
    /// Non-interactive and offline: nothing is fetched, no token is read, and
    /// nothing is written. When the config file already exists it is EDITED
    /// (comments and other watches kept) and the result printed; redirect it
    /// yourself once it reads right.
    Init(InitArgs),
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Numeric project id or group/project path (a project URL works too).
    #[arg(long)]
    project: String,
    /// Instance root.
    #[arg(long, default_value = "https://gitlab.com", value_name = "URL")]
    base_url: String,
    /// The [accounts.<name>] key. Defaults to one derived from the URL.
    #[arg(long)]
    account: Option<String>,
    /// Branch to watch.
    #[arg(long = "ref", default_value = "main", value_name = "REF")]
    ref_name: String,
    /// The watch id. Defaults to <project>-<ref>.
    #[arg(long, value_name = "ID")]
    watch_id: Option<String>,
    /// Pipeline source to accept. May be repeated.
    #[arg(long = "source", default_value = "push", value_name = "SOURCE")]
    sources: Vec<String>,
    /// Deploy marker job name (or re:<regex>). May be repeated.
    #[arg(long = "deploy-marker", value_name = "JOB")]
    deploy_markers: Vec<String>,
    /// Add a secondary watch for scheduled pipelines on the same branch.
    #[arg(long)]
    schedule: bool,
    /// Add a secondary watch for this ref glob, e.g. "pf/*".
    #[arg(long, value_name = "GLOB")]
    preflight: Option<String>,
    /// Poll interval while a pipeline is live, in seconds.
    #[arg(long, value_name = "SECS")]
    live_secs: Option<u64>,
    /// Use glab's keyring item for this instance (glab:<host>:token).
    #[arg(long, group = "token")]
    glab: bool,
    /// Read the token from this environment variable.
    #[arg(long, group = "token", value_name = "VAR")]
    token_env: Option<String>,
    /// Read the token from this keyring service (user "").
    #[arg(long, group = "token", value_name = "SERVICE")]
    token_keyring: Option<String>,
    /// Run this program for the token; repeat for each argument.
    #[arg(long, group = "token", value_name = "ARG", num_args = 1..)]
    token_command: Vec<String>,
}

#[derive(Debug, Args)]
struct CheckArgs {
    /// Only this watch. May be repeated.
    #[arg(long = "watch", value_name = "ID")]
    watches: Vec<String>,
    /// Print the Snapshot as JSON instead of a table.
    #[arg(long)]
    json: bool,
    /// Answer from a recorded fixture directory instead of the network. Every
    /// account is served by it, so no token is resolved and nothing is sent.
    #[arg(long, value_name = "DIR")]
    fixture: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct WatchArgs {
    /// Only this watch. May be repeated.
    #[arg(long = "watch", value_name = "ID")]
    watches: Vec<String>,
    /// Print one JSON object per change instead of a line.
    #[arg(long)]
    json: bool,
    /// Stop after this many ticks and exit with the last tick's verdict code.
    /// Mostly for testing.
    #[arg(long, value_name = "N")]
    ticks: Option<u32>,
    /// Answer from a recorded fixture directory instead of the network, as
    /// `check --fixture` does.
    #[arg(long, value_name = "DIR")]
    fixture: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Print the path that would be loaded.
    Path,
    /// Load it and report every problem with its line and column. Warnings do
    /// not fail it; an unusable file exits 78.
    Validate,
    /// Print the JSON Schema for config.toml.
    Schema,
    /// Print the configuration as loaded, with every default filled in.
    Dump,
}

#[derive(Debug, Subcommand)]
enum FixtureAction {
    /// Re-apply the recording allow-list to fixture directories, in place.
    ///
    /// Recordings made before the allow-list existed, or before it changed,
    /// carry fields the engine does not read — and a raw GitLab payload
    /// includes real names, email addresses, commit messages and runner
    /// descriptions. Fixtures are committed to a public repository.
    Scrub {
        /// Fixture directories to rewrite.
        #[arg(required = true)]
        dirs: Vec<PathBuf>,
        /// Report what would change, write nothing, and exit 1 if anything
        /// would.
        #[arg(long)]
        check: bool,
    },
    /// Walk one parent pipeline and write a fixture directory.
    Record {
        /// The parent pipeline id.
        pipeline_id: u64,
        /// Directory to write. Defaults to one named after the pipeline id, in
        /// the current directory.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Which account to use. Defaults to the only one, or to the first
        /// watch's.
        #[arg(long)]
        account: Option<String>,
        /// Project id or path. Defaults to the first watch's on that account.
        #[arg(long)]
        project: Option<String>,
        /// Also record the pipeline list page, not just the one row.
        #[arg(long)]
        list: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // `--help` and `--version` arrive here too, as "errors" clap wants
            // printed on stdout; they are successes. Everything else is a usage
            // error, and clap's own code for that (2) is a verdict here.
            let code = if e.use_stderr() {
                exit::USAGE
            } else {
                exit::OK
            };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let code = match run(cli).await {
        Ok(code) => code,
        Err(Failure { code, error }) => {
            eprintln!("error: {error:#}");
            code
        }
    };
    std::process::exit(code);
}

async fn run(cli: Cli) -> Outcome {
    let config = cli.config;
    match cli.command {
        Command::Config { action } => config_command(&config, action),
        Command::Check(args) => check(&config, args).await,
        Command::Watch(args) => watch(&config, args).await,
        Command::Fixture { action } => fixture(&config, action).await,
        Command::Init(args) => init(&config, args),
    }
}

/// `bridgewatch init`: the wizard's `build_config`, answered from flags.
fn init(config_args: &ConfigArgs, args: InitArgs) -> Outcome {
    use bridgewatch_core::config::TokenSource;
    use bridgewatch_core::wizard::{self, WizardAnswers, WizardError};

    let usage = |e: WizardError| Failure::new(exit::USAGE, e);
    let project = wizard::parse_project_input(&args.project).map_err(usage)?;
    let token = if args.glab {
        let service = wizard::glab_service_for(&args.base_url)
            .ok_or_else(|| Failure::new(exit::USAGE, anyhow::anyhow!("--base-url has no host")))?;
        TokenSource::Keyring {
            service,
            user: String::new(),
        }
    } else if let Some(var) = args.token_env {
        TokenSource::Env(var)
    } else if let Some(service) = args.token_keyring {
        TokenSource::Keyring {
            service,
            user: String::new(),
        }
    } else if !args.token_command.is_empty() {
        TokenSource::Command(args.token_command)
    } else {
        TokenSource::Own(true)
    };
    let watch_id = args
        .watch_id
        .unwrap_or_else(|| wizard::suggest_watch_id(&project.to_string(), &args.ref_name));
    let answers = WizardAnswers {
        account: args
            .account
            .unwrap_or_else(|| wizard::suggest_account_name(&args.base_url)),
        base_url: args.base_url,
        token,
        project: Some(project),
        watch_id,
        ref_name: args.ref_name,
        sources: args.sources,
        deploy_markers: args.deploy_markers,
        schedule_watch: args.schedule,
        preflight_ref: args.preflight,
        notify: None,
        launch_at_login: None,
        live_secs: args.live_secs,
    };

    let path = config::resolve_path(config_args.config.as_deref());
    let existing = match std::fs::read_to_string(&path) {
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(Failure::new(
                exit::CONFIG,
                anyhow::anyhow!("{}: {e}", path.display()),
            ));
        }
    };
    match wizard::build_config(&answers, existing.as_deref()) {
        Ok(built) => {
            for w in &built.warnings {
                eprintln!("warning: {}: {}", w.path, w.message);
            }
            eprintln!(
                "{} {} (nothing written)",
                if built.edited_existing {
                    "edited"
                } else {
                    "new file for"
                },
                path.display()
            );
            print!("{}", built.toml);
            Ok(exit::OK)
        }
        Err(e @ WizardError::Answers(_)) => {
            if let WizardError::Answers(issues) = &e {
                for i in issues {
                    eprintln!("error: {:?} {}: {}", i.step, i.field, i.message);
                }
            }
            Err(usage(e))
        }
        Err(e @ (WizardError::Edit(_) | WizardError::Invalid(_))) => Err(Failure::new(
            exit::CONFIG,
            anyhow::Error::new(e).context(format!("cannot edit {}", path.display())),
        )),
        Err(e) => Err(Failure::new(exit::USAGE, e)),
    }
}

/// The crates `[log].level` is scoped to: this binary and the engine under it.
///
/// ⚠ `CARGO_CRATE_NAME` rather than the literal: the package is
/// `bridgewatch-cli`, but the target is `[[bin]] name = "bridgewatch"`, so the
/// crate rustc compiles (and the target `tracing` stamps on every event from
/// this file) is `bridgewatch`.
const OWN_CRATES: &[&str] = &[env!("CARGO_CRATE_NAME"), "bridgewatch_core"];

/// Initialise tracing. A non-empty `RUST_LOG` wins; otherwise the config's
/// `log.level`, applied to bridgewatch's own crates with everything else left
/// at `warn`.
///
/// ⛔ This is the README's documented rule and it used to be the app's alone:
/// the level went to EVERY crate, and a set-but-empty `RUST_LOG` (a leftover
/// `export RUST_LOG=` in a shell profile) beat the config and silenced the
/// program. `config::log_directive` is now the single answer for both; the
/// other caller is `logging::directive` in `src-tauri/src/logging.rs`.
fn init_tracing(level: &str) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(tracing_filter(
            std::env::var("RUST_LOG").ok().as_deref(),
            level,
        )))
        .with_writer(std::io::stderr)
        .try_init();
}

/// The filter directive [`init_tracing`] installs, separated out so it can be
/// tested without touching the process-wide subscriber.
fn tracing_filter(rust_log: Option<&str>, level: &str) -> String {
    config::log_directive(rust_log, Some(level), OWN_CRATES)
}

/// `path:line:col` when the offset is known, else just the path.
fn location(path: &std::path::Path, raw: &str, offset: Option<usize>) -> String {
    match offset {
        Some(offset) => {
            let (line, col) = config::line_col(raw, offset);
            format!("{}:{line}:{col}", path.display())
        }
        None => path.display().to_string(),
    }
}

/// Load the configuration, printing every diagnostic with its location.
///
/// Any failure is [`exit::CONFIG`]: a file that cannot be used is never a
/// verdict about a pipeline.
fn load(args: &ConfigArgs) -> std::result::Result<config::Loaded, Failure> {
    let path = config::resolve_path(args.config.as_deref());
    match config::load(&path) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                let where_ = location(&loaded.path, &loaded.raw, w.span.as_ref().map(|s| s.start));
                eprintln!("warning: {where_}: {}: {}", w.path, w.message);
            }
            Ok(loaded)
        }
        Err(config::ConfigError::Invalid { path, diagnostics }) => {
            let raw = std::fs::read_to_string(&path).unwrap_or_default();
            for d in &diagnostics {
                let severity = match d.severity {
                    config::Severity::Error => "error",
                    config::Severity::Warning => "warning",
                };
                let where_ = location(&path, &raw, d.span.as_ref().map(|s| s.start));
                eprintln!("{severity}: {where_}: {}: {}", d.path, d.message);
            }
            Err(Failure::new(
                exit::CONFIG,
                anyhow::anyhow!("{} is not usable", path.display()),
            ))
        }
        // ⛔ M16. A TOML syntax error is the one problem a user most needs a
        // line number for, and it was the one printed without: the span was in
        // the error and nothing turned it into a position.
        Err(config::ConfigError::Parse {
            path,
            message,
            span,
        }) => {
            let raw = std::fs::read_to_string(&path).unwrap_or_default();
            let where_ = location(&path, &raw, span.map(|s| s.start));
            Err(Failure::new(
                exit::CONFIG,
                anyhow::anyhow!("{where_}: {}", message.trim_end()),
            ))
        }
        Err(e) => Err(Failure::new(
            exit::CONFIG,
            anyhow::Error::new(e).context("cannot load configuration"),
        )),
    }
}

fn config_command(args: &ConfigArgs, action: ConfigAction) -> Outcome {
    match action {
        ConfigAction::Path => {
            println!("{}", config::resolve_path(args.config.as_deref()).display());
            Ok(exit::OK)
        }
        ConfigAction::Validate => {
            let loaded = load(args)?;
            println!(
                "{}: ok ({} account(s), {} watch(es), {} warning(s))",
                loaded.path.display(),
                loaded.config.account_count(),
                loaded.config.watches.len(),
                loaded.warnings.len()
            );
            Ok(exit::OK)
        }
        ConfigAction::Schema => {
            let schema = schemars::schema_for!(Config);
            println!("{}", serde_json::to_string_pretty(&schema)?);
            Ok(exit::OK)
        }
        ConfigAction::Dump => {
            let loaded = load(args)?;
            println!("{}", toml::to_string_pretty(&loaded.config)?);
            Ok(exit::OK)
        }
    }
}

/// Narrow a configuration to the named watches, if any were named.
///
/// A name that matches nothing is a usage error, not a config one: the file is
/// fine, the command line asked for something it does not contain.
fn filter_watches(config: &mut Config, ids: &[String]) -> std::result::Result<(), Failure> {
    if ids.is_empty() {
        return Ok(());
    }
    let known: Vec<String> = config.watches.iter().map(|w| w.id.clone()).collect();
    for id in ids {
        if !known.contains(id) {
            return Err(Failure::new(
                exit::USAGE,
                anyhow::anyhow!(
                    "no watch with id {id:?}; configured watches are [{}]",
                    known.join(", ")
                ),
            ));
        }
    }
    config.watches.retain(|w| ids.contains(&w.id));
    Ok(())
}

/// Build a poller, either against the network or against a fixture directory.
fn build_poller(config: &Config, fixture: Option<&PathBuf>) -> Result<Poller> {
    let ring = RequestRing::new(config.log.keep_requests);
    match fixture {
        Some(dir) => {
            let transport = Arc::new(
                FixtureTransport::load(dir)
                    .with_context(|| format!("cannot load fixture {}", dir.display()))?,
            );
            let mut clients = std::collections::BTreeMap::new();
            for (name, account) in &config.accounts {
                clients.insert(
                    name.clone(),
                    // A fixture serves recorded bytes; the token is never sent
                    // anywhere, and resolving a real one would prompt for a
                    // keychain unlock to answer a question from a file.
                    GitLabClient::new(
                        account,
                        &Secret::new("fixture"),
                        transport.clone(),
                        ring.clone(),
                    ),
                );
            }
            Poller::with_clients(config, clients, ring).context("cannot build poller")
        }
        None => Poller::from_config(config, &SystemTokenProvider).context("cannot build poller"),
    }
}

async fn check(config_args: &ConfigArgs, args: CheckArgs) -> Outcome {
    let loaded = load(config_args)?;
    init_tracing(&loaded.config.log.level);
    let mut config = loaded.config;
    filter_watches(&mut config, &args.watches)?;

    let mut poller = build_poller(&config, args.fixture.as_ref())?;
    let tick = poller.tick().await;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&tick.snapshot)?);
    } else {
        print!("{}", render::snapshot(&tick.snapshot));
    }
    Ok(exit_code(&tick.snapshot))
}

/// The verdict code for a snapshot.
///
/// ⛔ B1's CLI half: a pipeline whose `/jobs` or `/bridges` request failed,
/// with nothing cached, used to be judged from an empty job list — "nothing
/// failed" — and `check` exited 0 over a red pipeline. The core now reports
/// that pipeline as `unknown`, and `unknown` is its own code here, distinct
/// from every verdict and from every error, so a script can tell "could not
/// see" from "saw green".
fn exit_code(snapshot: &Snapshot) -> i32 {
    match snapshot.icon_state {
        IconState::Deployed | IconState::SucceededNoDeploy => exit::OK,
        IconState::Failed => exit::FAILED,
        IconState::DeployedWithFailure => exit::DEPLOYED_WITH_FAILURE,
        IconState::Running | IconState::ParkedGate | IconState::Canceled => exit::PENDING,
        IconState::Unknown => exit::UNKNOWN,
    }
}

async fn watch(config_args: &ConfigArgs, args: WatchArgs) -> Outcome {
    let loaded = load(config_args)?;
    init_tracing(&loaded.config.log.level);
    let mut config = loaded.config;
    filter_watches(&mut config, &args.watches)?;

    let poller = build_poller(&config, args.fixture.as_ref())?;
    // A fixture run is a rehearsal: it must not write the real ledger, or the
    // next launch of the app would believe it had already announced a
    // recorded pipeline.
    let mut poller = if args.fixture.is_some() {
        poller
    } else {
        poller.with_ledger(NotifyLedger::cli_path())
    };

    let mut previous: Option<String> = None;
    let mut ticks = 0u32;
    let limit = args.ticks;

    loop {
        let tick = poller.tick().await;
        ticks += 1;

        let line = if args.json {
            serde_json::to_string(&tick.snapshot)?
        } else {
            render::one_line(&tick.snapshot)
        };
        // Only print when something moved: a watch loop that repeats itself
        // every twenty seconds is a log nobody reads.
        let fingerprint = render::fingerprint(&tick.snapshot);
        if previous.as_deref() != Some(fingerprint.as_str()) {
            println!("{line}");
            previous = Some(fingerprint);
        }
        for n in &tick.notifications {
            println!("notify [{}] {} — {}", n.kind.as_str(), n.title, n.body);
        }

        if limit.is_some_and(|l| ticks >= l) {
            return Ok(exit_code(&tick.snapshot));
        }

        tokio::select! {
            _ = tokio::time::sleep(tick.next_interval) => {}
            _ = tokio::signal::ctrl_c() => return Ok(exit_code(&tick.snapshot)),
        }
    }
}

async fn fixture(config_args: &ConfigArgs, action: FixtureAction) -> Outcome {
    let (pipeline_id, out, account, project, list) = match action {
        FixtureAction::Scrub { dirs, check } => return scrub_fixtures(&dirs, check),
        FixtureAction::Record {
            pipeline_id,
            out,
            account,
            project,
            list,
        } => (pipeline_id, out, account, project, list),
    };
    let out = out.unwrap_or_else(|| PathBuf::from(pipeline_id.to_string()));

    let loaded = load(config_args)?;
    init_tracing(&loaded.config.log.level);
    let config = loaded.config;

    let account_name = match account {
        Some(a) => a,
        None => config
            .watches
            .first()
            .map(|w| w.account.clone())
            .or_else(|| config.accounts.keys().next().cloned())
            .context("no accounts configured")?,
    };
    let account_def = config
        .accounts
        .get(&account_name)
        .with_context(|| format!("no account named {account_name:?}"))?;

    let project_ref = match project {
        Some(p) => match p.parse::<u64>() {
            Ok(id) => ProjectRef::Id(id),
            Err(_) => ProjectRef::Path(p),
        },
        None => config
            .watches
            .iter()
            .find(|w| w.account == account_name)
            .map(|w| w.project.clone())
            .context("no watch on that account to take a project from; pass --project")?,
    };

    let token = token::resolve(&account_def.token, &account_name, &SystemTokenProvider)
        .with_context(|| format!("cannot resolve the token for account {account_name:?}"))?;
    let transport =
        ReqwestTransport::new(std::time::Duration::from_secs(account_def.timeout_secs))?;
    let ring = RequestRing::new(config.log.keep_requests);
    let client = GitLabClient::new(account_def, &token, Arc::new(transport), ring);

    let recorded =
        bridgewatch_core::client::fixture::record(&client, &project_ref, pipeline_id, &out, list)
            .await
            .context("recording failed")?;

    println!("wrote {}:", recorded.dir.display());
    for f in &recorded.files {
        println!("  {f}");
    }
    for (bridge, child) in &recorded.children {
        match child {
            Some(id) => println!("  bridge {bridge} -> child {id}"),
            None => println!("  bridge {bridge} -> no child (dead bridge)"),
        }
    }
    println!("\nNow write {}/expected.json.", recorded.dir.display());
    Ok(exit::OK)
}

/// Re-apply the recording allow-list to fixture directories.
///
/// ⛔ `--check` READS. It used to scrub for real and then write the old bytes
/// back from a copy in memory, so the gate modified the tree it was checking
/// and an interrupt between the two writes lost the file.
fn scrub_fixtures(dirs: &[PathBuf], check: bool) -> Outcome {
    let mode = if check {
        ScrubMode::Check
    } else {
        ScrubMode::Write
    };
    let mut total = 0usize;
    for dir in dirs {
        if !dir.is_dir() {
            return Err(Failure::new(
                exit::USAGE,
                anyhow::anyhow!("{} is not a directory", dir.display()),
            ));
        }
        let changed = bridgewatch_core::client::fixture::scrub_dir_with(dir, mode)
            .with_context(|| format!("cannot scrub {}", dir.display()))?;
        for path in &changed {
            println!(
                "{} {}",
                if check { "would scrub" } else { "scrubbed" },
                path.display()
            );
        }
        total += changed.len();
    }
    let verb = if check { "would change" } else { "changed" };
    println!("{total} file(s) {verb}");
    // `--check` is a gate: a dirty fixture must fail a pipeline.
    Ok(if check && total > 0 {
        exit::FAILED
    } else {
        exit::OK
    })
}

#[cfg(test)]
mod tests {
    use super::{OWN_CRATES, exit, tracing_filter};

    /// The `--help` table is prose, so nothing ties it to the constants but
    /// this: every constant has exactly one row, and every row is a constant.
    #[test]
    fn the_help_table_matches_the_exit_constants() {
        let mut documented: Vec<i32> = exit::HELP
            .lines()
            .skip(1)
            .map(|l| {
                l.split_whitespace()
                    .next()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or_else(|| panic!("a row that does not start with a code: {l:?}"))
            })
            .collect();
        documented.sort_unstable();
        let mut defined = vec![
            exit::OK,
            exit::FAILED,
            exit::DEPLOYED_WITH_FAILURE,
            exit::PENDING,
            exit::UNKNOWN,
            exit::USAGE,
            exit::ERROR,
            exit::CONFIG,
        ];
        defined.sort_unstable();
        assert_eq!(documented, defined);
    }

    /// An exported-but-blank `RUST_LOG` is a leftover in a shell profile, not a
    /// request to silence the program. It used to win here and produce an
    /// EnvFilter with no directives at all, which logged nothing whatever the
    /// config asked for.
    #[test]
    fn an_empty_rust_log_is_ignored() {
        for blank in ["", "   "] {
            assert_eq!(
                tracing_filter(Some(blank), "debug"),
                tracing_filter(None, "debug"),
                "{blank:?} should count as unset"
            );
        }
        assert_eq!(tracing_filter(Some(""), "warn"), "warn");
    }

    /// A non-empty `RUST_LOG` is the escape hatch: it wins whole, unmodified,
    /// and unscoped.
    #[test]
    fn a_non_empty_rust_log_wins() {
        assert_eq!(tracing_filter(Some("trace"), "error"), "trace");
        assert_eq!(
            tracing_filter(Some("hyper=debug,bridgewatch_core=trace"), "info"),
            "hyper=debug,bridgewatch_core=trace"
        );
    }

    /// `[log].level` says how loud BRIDGEWATCH should be, not the HTTP stack.
    #[test]
    fn the_file_level_scopes_to_bridgewatch_crates() {
        let filter = tracing_filter(None, "debug");
        assert!(filter.starts_with("warn,"), "{filter}");
        for name in OWN_CRATES {
            assert!(filter.contains(&format!("{name}=debug")), "{filter}");
        }
        assert_eq!(filter.matches("=debug").count(), OWN_CRATES.len());
        // Quieter than `warn` means quieter everywhere, so there is nothing to
        // scope; nonsense falls back to `warn` rather than failing a command.
        assert_eq!(tracing_filter(None, "off"), "off");
        assert_eq!(tracing_filter(None, "loud"), "warn");
    }

    /// The binary's crate name is its TARGET name, and the filter is useless if
    /// it names something `tracing` never stamps on an event.
    #[test]
    fn the_scoped_crate_names_are_the_ones_tracing_sees() {
        assert_eq!(OWN_CRATES[0], module_path!().split("::").next().unwrap());
        assert_eq!(OWN_CRATES[1], "bridgewatch_core");
    }
}
