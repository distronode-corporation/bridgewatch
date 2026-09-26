//! Logging: to stderr and to a daily log file, and which of `RUST_LOG` and
//! `[log].level` decides how much.
//!
//! The precedence: a non-empty `RUST_LOG` wins, whole and unmodified, for the
//! life of the process. Otherwise `[log].level` from the configuration is
//! applied at startup and again on every reload that changes it. With neither,
//! the level is `warn`.
//!
//! ⚠ `[log].level` is applied to bridgewatch's own crates only. Asking for
//! `debug` should show what bridgewatch is doing, not every frame the window
//! system and the HTTP stack log at that level; the rest stays at `warn`. A
//! level QUIETER than `warn` (`error`, `off`) applies to everything, since
//! "less, please" means less of all of it. `RUST_LOG` is the escape hatch for
//! anything finer.
//!
//! ⛔ The file is not optional in practice. A tray app started from Finder, the
//! Dock or a login item on macOS has its stderr pointed at `/dev/null`, so the
//! stderr-only logging this used to be wrote NOTHING anywhere for every
//! ordinary launch, and the only way to get a log for a bug report was to start
//! the binary from a shell. Where the file goes is [`log_dir`].

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, reload};

/// The crates `[log].level` is scoped to: this shell and the engine under it.
const OWN_CRATES: &[&str] = &["bridgewatch_app", "bridgewatch_core"];

/// The log files are `bridgewatch.<date>.log`.
const FILE_PREFIX: &str = "bridgewatch";
const FILE_SUFFIX: &str = "log";
/// A week of daily files. Older ones are deleted when a new day's is opened.
const KEEP_FILES: usize = 7;

/// Set only when `RUST_LOG` did NOT decide, so a reload cannot override it.
static HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
/// The directive currently applied through [`HANDLE`].
static CURRENT: Mutex<String> = Mutex::new(String::new());
/// The directory the log file is being written to, when there is one.
static FILE_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Keeps the file writer's background thread alive; see [`flush`].
static GUARD: Mutex<Option<WorkerGuard>> = Mutex::new(None);

/// The filter directive for a `RUST_LOG` value and a `[log].level`.
///
/// The rule itself is `config::log_directive` in the core, so that the CLI's
/// `init_tracing` cannot drift from it; only the crate names differ.
pub fn directive(rust_log: Option<&str>, level: Option<&str>) -> String {
    bridgewatch_core::config::log_directive(rust_log, level, OWN_CRATES)
}

/// Where the log file goes: `~/Library/Logs/bridgewatch` on macOS, where
/// Console.app lists it, and `$XDG_STATE_HOME/bridgewatch` (by default
/// `~/.local/state/bridgewatch`, beside the notification ledger) elsewhere.
/// `None` only when there is no home directory to put it under.
pub fn log_dir() -> Option<PathBuf> {
    log_dir_for(
        cfg!(target_os = "macos"),
        dirs::home_dir(),
        // `dirs::state_dir` is `$XDG_STATE_HOME` when that is absolute and
        // `~/.local/state` otherwise, which is the XDG rule; it is `None` on
        // macOS, where the first branch below applies instead.
        dirs::state_dir().or_else(dirs::data_local_dir),
    )
}

/// [`log_dir`] with the platform and its directories as arguments, so both
/// platforms' answers are testable on either.
fn log_dir_for(macos: bool, home: Option<PathBuf>, state: Option<PathBuf>) -> Option<PathBuf> {
    if macos {
        home.map(|h| h.join("Library").join("Logs").join("bridgewatch"))
    } else {
        state.map(|s| s.join("bridgewatch"))
    }
}

/// The directory the file layer is writing to, or where it would be if it
/// could not be opened. What "Open log folder" opens.
pub fn folder() -> Option<PathBuf> {
    FILE_DIR.get().cloned().or_else(log_dir)
}

/// A daily file writer in `dir`, creating the directory first.
///
/// Fails when the directory cannot be created or the day's file cannot be
/// opened for appending, which is the moment to find out: the non-blocking
/// writer that wraps this reports nothing once it is running.
fn file_appender(dir: &Path) -> Result<RollingFileAppender, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(KEEP_FILES)
        .build(dir)
        .map_err(|e| e.to_string())
}

/// The subscriber: one filter, then a stderr layer and, when there is one, a
/// file layer.
///
/// ⛔ The filter is on the REGISTRY, once, and not on each layer. A
/// `reload::Layer<EnvFilter>` stacked like this is a global filter: an event it
/// refuses reaches neither writer, and one reload changes both. Giving each fmt
/// layer its own filter would need two reload handles kept in step, and the
/// first time they disagreed the file would say something different from the
/// terminal. The handle's type is also what [`HANDLE`] stores, which only works
/// while the filter sits directly on the `Registry`.
///
/// The file never gets ANSI escapes, whatever stderr is: nothing that reads a
/// log file renders them, and they turn every line of a pasted bug report into
/// noise.
fn subscriber<E, F>(
    filter: reload::Layer<EnvFilter, Registry>,
    stderr: E,
    stderr_ansi: bool,
    file: Option<F>,
) -> impl tracing::Subscriber + Send + Sync + 'static
where
    E: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    F: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(stderr)
                .with_ansi(stderr_ansi),
        )
        .with(file.map(|writer| {
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
        }))
}

/// Install the subscriber. Called once, before the configuration is read, so
/// that seeding and resolution can log; [`apply_config_level`] follows once
/// the file has loaded.
///
/// A log directory that cannot be created or written costs the file and
/// nothing else: the app runs on with stderr alone and says so in one `warn`
/// line, which goes to stderr and so is seen by whoever launched it from a
/// shell to find out why there is no file.
pub fn init() {
    let rust_log = std::env::var("RUST_LOG").ok();
    let from_env = rust_log.as_deref().is_some_and(|v| !v.trim().is_empty());
    let initial = directive(rust_log.as_deref(), None);
    let (filter, handle) = reload::Layer::new(EnvFilter::new(&initial));
    // Colouring stderr (the fmt layer's default) writes escapes into every
    // recorded line when it is not a terminal: the systemd user journal on
    // Linux, a pipe, or a file somebody redirected it to. The rule is
    // `config::use_ansi` in the core, shared with the CLI's `init_tracing`.
    let ansi = bridgewatch_core::config::use_ansi(
        std::io::stderr().is_terminal(),
        std::env::var("NO_COLOR").ok().as_deref(),
    );

    let dir = log_dir();
    let opened = match &dir {
        Some(dir) => file_appender(dir).map(|appender| (dir.clone(), appender)),
        None => Err("there is no home directory to put it under".to_string()),
    };
    let (file, file_error) = match opened {
        Ok((dir, appender)) => {
            // ⚠ Non-blocking, so a slow disk never stalls the poll task or the
            // UI thread on a log line. The guard flushes the writer's queue
            // when dropped, and it is dropped by [`flush`] on the way out.
            let (writer, guard) = tracing_appender::non_blocking(appender);
            *GUARD.lock().unwrap_or_else(|e| e.into_inner()) = Some(guard);
            let _ = FILE_DIR.set(dir);
            (Some(writer), None)
        }
        Err(e) => (None, Some(e)),
    };

    let installed = subscriber(filter, std::io::stderr, ansi, file)
        .try_init()
        .is_ok();
    if installed && !from_env {
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = initial;
        let _ = HANDLE.set(handle);
    }
    if let Some(error) = file_error {
        tracing::warn!(
            dir = %dir.as_deref().map(Path::display).map(|d| d.to_string()).unwrap_or_default(),
            error = %error,
            "could not open a log file; logging to stderr only"
        );
    }
}

/// Write out whatever the file writer still has queued. Called on exit.
///
/// ⚠ Needed because the process ends by `exit`, which runs no destructors, so
/// the guard in [`GUARD`] would otherwise never be dropped and the last lines
/// before a quit (the ones that say why somebody quit) could be lost.
pub fn flush() {
    drop(GUARD.lock().unwrap_or_else(|e| e.into_inner()).take());
}

/// Apply `[log].level`, unless `RUST_LOG` decided at startup.
pub fn apply_config_level(level: &str) {
    let Some(handle) = HANDLE.get() else { return };
    let wanted = directive(None, Some(level));
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if *current == wanted {
        return;
    }
    match handle.reload(EnvFilter::new(&wanted)) {
        Ok(()) => *current = wanted,
        Err(e) => eprintln!("bridgewatch: could not change the log level: {e}"),
    }
}

/// The filter directive in force, for the config-loaded line to quote.
///
/// [`CURRENT`] is empty exactly when `RUST_LOG` decided at startup, in which
/// case the variable itself is the honest answer: [`HANDLE`] was never set, so
/// nothing this program does can change the level for the rest of the run.
pub fn current_directive() -> String {
    let current = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !current.is_empty() {
        return current;
    }
    match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => directive(None, None),
    }
}

/// The one info line logged at startup: which file, what it asks for, and how
/// loud the program will be.
pub fn startup_line(resolved: &crate::config::Resolved) -> String {
    let seeded = if resolved.seeded {
        " (seeded from the example)"
    } else {
        ""
    };
    config_line(
        &format!(
            "bridgewatch {} using {}{seeded}",
            env!("CARGO_PKG_VERSION"),
            resolved.path.display()
        ),
        resolved.loaded.as_ref().map(|l| &l.config),
    )
}

/// The info line for a configuration that has just been read, from whichever
/// read it was.
///
/// ⚠ A reload logs the same shape as the startup line on purpose. `info` is
/// the level for "a person is debugging why the tray is wrong", and the first
/// question is always which file is in force and what is in it, an answer that
/// has to survive the wizard writing the file after startup, and every hot
/// reload after that.
pub fn config_line(head: &str, config: Option<&bridgewatch_core::config::Config>) -> String {
    bridgewatch_core::config::describe_load(head, config, &current_directive())
}

#[cfg(test)]
mod tests {
    use super::{directive, file_appender, log_dir_for, startup_line, subscriber};
    use std::path::PathBuf;
    use tracing_subscriber::{EnvFilter, reload};

    /// A directory of its own under the system temp dir, emptied first.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bw-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Everything written to the log files in `dir`, and how many there are.
    fn read_logs(dir: &std::path::Path) -> (usize, String) {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        files.sort();
        let text = files
            .iter()
            .map(|f| std::fs::read_to_string(f).unwrap())
            .collect::<String>();
        (files.len(), text)
    }

    #[test]
    fn the_log_folder_is_library_logs_on_macos_and_xdg_state_elsewhere() {
        let home = Some(PathBuf::from("/home/u"));
        let state = Some(PathBuf::from("/home/u/.local/state"));
        assert_eq!(
            log_dir_for(true, home.clone(), None),
            Some(PathBuf::from("/home/u/Library/Logs/bridgewatch"))
        );
        assert_eq!(
            log_dir_for(false, home.clone(), state),
            Some(PathBuf::from("/home/u/.local/state/bridgewatch"))
        );
        assert_eq!(
            log_dir_for(false, home, Some(PathBuf::from("/custom/state"))),
            Some(PathBuf::from("/custom/state/bridgewatch")),
            "an XDG_STATE_HOME that dirs resolved is used as given"
        );
        assert_eq!(log_dir_for(true, None, None), None);
        assert_eq!(log_dir_for(false, None, None), None);
    }

    /// ⛔ A folder that cannot be made is an error at startup, where `init`
    /// can still fall back to stderr alone, and not a writer that fails
    /// silently on every line afterwards. Here the "directory" is under a
    /// regular FILE, which no permission setting can make work, so the test
    /// does not depend on who runs it.
    #[test]
    fn a_log_folder_that_cannot_be_created_is_reported_and_not_a_silent_writer() {
        let dir = scratch("blocked");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-directory");
        std::fs::write(&file, "").unwrap();
        assert!(file_appender(&file.join("bridgewatch")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The file layer obeys the one reloadable filter, exactly as stderr does:
    /// a `debug` line is absent at `warn` and present once `[log].level` is
    /// reloaded to `debug`. And the file carries no ANSI escapes even when
    /// stderr is told to colour.
    ///
    /// ⚠ Captured with `with_default` and callsites that exist only in this
    /// test, so no other test's thread can register them first and cache a
    /// `never` interest for everybody (see the core's `tests/support`).
    #[test]
    fn the_file_obeys_the_reloadable_filter_and_never_carries_ansi() {
        let dir = scratch("filter");
        let appender = file_appender(&dir).expect("a writable temp dir");
        let (filter, handle) = reload::Layer::new(EnvFilter::new(directive(None, None)));
        let sub = subscriber(filter, std::io::sink, true, Some(appender));
        tracing::subscriber::with_default(sub, || {
            tracing::debug!("bw-file-test quiet before the reload");
            tracing::warn!(field = "value", "bw-file-test loud before the reload");
            handle
                .reload(EnvFilter::new(directive(None, Some("debug"))))
                .unwrap();
            tracing::debug!("bw-file-test quiet after the reload");
        });
        let (count, text) = read_logs(&dir);
        assert_eq!(count, 1, "one file for today: {text}");
        assert!(text.contains("bw-file-test loud before"), "{text}");
        assert!(!text.contains("quiet before"), "filtered at warn: {text}");
        assert!(
            text.contains("bw-file-test quiet after"),
            "the reload reached the file too: {text}"
        );
        assert!(!text.contains('\u{1b}'), "no escape sequences: {text:?}");
        let name = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name();
        let name = name.to_string_lossy();
        assert!(
            name.starts_with("bridgewatch.") && name.ends_with(".log"),
            "{name}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_startup_line_names_the_file_and_counts_the_watches() {
        let raw = "[accounts.gl]\nbase_url = \"https://gitlab.com\"\n\
                   [[watches]]\nid = \"a\"\naccount = \"gl\"\nproject = 1\n";
        let path = std::path::PathBuf::from("/tmp/bw/config.toml");
        let resolved = crate::config::Resolved {
            path: path.clone(),
            seeded: false,
            first_run: false,
            loaded: bridgewatch_core::config::parse_str(raw, &path).ok(),
            validation: Default::default(),
            text: None,
        };
        let line = startup_line(&resolved);
        assert!(line.contains("/tmp/bw/config.toml"), "{line}");
        assert!(line.contains("1 watch(es)"), "{line}");
        let broken = crate::config::Resolved {
            loaded: None,
            ..resolved
        };
        assert!(startup_line(&broken).contains("does not load"));
    }

    #[test]
    fn rust_log_wins_over_the_file() {
        assert_eq!(directive(Some("trace"), Some("error")), "trace");
        assert_eq!(
            directive(Some("bridgewatch_core=debug"), Some("info")),
            "bridgewatch_core=debug"
        );
    }

    #[test]
    fn an_empty_rust_log_is_unset() {
        assert_eq!(directive(Some("  "), Some("error")), "error");
    }

    #[test]
    fn the_file_level_is_used_when_rust_log_is_unset() {
        assert_eq!(
            directive(None, Some("debug")),
            "warn,bridgewatch_app=debug,bridgewatch_core=debug"
        );
        assert_eq!(directive(None, Some("INFO")).matches("=info").count(), 2);
        assert_eq!(directive(None, Some("off")), "off");
    }

    #[test]
    fn nothing_or_nonsense_means_warn() {
        assert_eq!(directive(None, None), "warn");
        assert_eq!(directive(None, Some("loud")), "warn");
    }
}
