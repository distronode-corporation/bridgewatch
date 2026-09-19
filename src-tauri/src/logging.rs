//! stderr logging, and which of `RUST_LOG` and `[log].level` decides it.
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

use std::io::IsTerminal;
use std::sync::{Mutex, OnceLock};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, reload};

/// The crates `[log].level` is scoped to: this shell and the engine under it.
const OWN_CRATES: &[&str] = &["bridgewatch_app", "bridgewatch_core"];

/// Set only when `RUST_LOG` did NOT decide, so a reload cannot override it.
static HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
/// The directive currently applied through [`HANDLE`].
static CURRENT: Mutex<String> = Mutex::new(String::new());

/// The filter directive for a `RUST_LOG` value and a `[log].level`.
///
/// The rule itself is `config::log_directive` in the core, so that the CLI's
/// `init_tracing` cannot drift from it; only the crate names differ.
pub fn directive(rust_log: Option<&str>, level: Option<&str>) -> String {
    bridgewatch_core::config::log_directive(rust_log, level, OWN_CRATES)
}

/// Install the subscriber. Called once, before the configuration is read, so
/// that seeding and resolution can log; [`apply_config_level`] follows once
/// the file has loaded.
pub fn init() {
    let rust_log = std::env::var("RUST_LOG").ok();
    let from_env = rust_log.as_deref().is_some_and(|v| !v.trim().is_empty());
    let initial = directive(rust_log.as_deref(), None);
    let (filter, handle) = reload::Layer::new(EnvFilter::new(&initial));
    // This process's stderr is the systemd user journal on Linux and a pipe
    // under launchd, so colouring it (the fmt layer's default) writes escapes
    // into every recorded line. The rule is `config::use_ansi` in the core,
    // shared with the CLI's `init_tracing`.
    let ansi = bridgewatch_core::config::use_ansi(
        std::io::stderr().is_terminal(),
        std::env::var("NO_COLOR").ok().as_deref(),
    );
    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(ansi),
        )
        .try_init()
        .is_ok();
    if installed && !from_env {
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = initial;
        let _ = HANDLE.set(handle);
    }
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
    use super::{directive, startup_line};

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
