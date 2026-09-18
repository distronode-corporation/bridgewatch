//! Finding, seeding, validating and hot-reloading `config.toml`.
//!
//! The file is the source of truth, so everything here is careful about two
//! things: a bad edit must never take the running configuration down with it,
//! and a good edit must never lose a hand-written comment.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bridgewatch_core::config::{self, ConfigError, Diagnostic, Loaded, Severity};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;

/// The example that ships with the repository. No longer written by the app
/// (a first launch opens the setup wizard); kept so a test still proves the
/// example the README points people at is a valid configuration.
#[cfg(test)]
pub const EXAMPLE_CONFIG: &str = include_str!("../../examples/distronode.toml");

/// Debounce window for the file watcher.
///
/// Editors do not write a file once. vim writes a backup, renames, and writes
/// again; VS Code truncates then writes. 300 ms collapses that into one reload
/// and, more importantly, avoids reading a half-written file and reporting a
/// syntax error the user never made.
pub const RELOAD_DEBOUNCE: Duration = Duration::from_millis(300);

/// A diagnostic, flattened for the frontend with its line and column resolved.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticView {
    /// `error` or `warning`.
    pub severity: String,
    /// Dotted path to the offending key.
    pub path: String,
    /// What is wrong.
    pub message: String,
    /// 1-based line, when the problem could be located in the source.
    pub line: Option<usize>,
    /// 1-based column.
    pub col: Option<usize>,
}

impl DiagnosticView {
    /// Flatten a core diagnostic against the raw source it came from.
    pub fn new(d: &Diagnostic, raw: &str) -> Self {
        let (line, col) = match d.line_col(raw) {
            Some((l, c)) => (Some(l), Some(c)),
            None => (None, None),
        };
        Self {
            severity: match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            }
            .to_string(),
            path: d.path.clone(),
            message: d.message.clone(),
            line,
            col,
        }
    }
}

/// The answer to "is this text a usable configuration?", as the settings pane
/// renders it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Validation {
    /// True when the text would load. Warnings do not make it false.
    pub ok: bool,
    /// Every problem found, errors and warnings together.
    pub diagnostics: Vec<DiagnosticView>,
    /// Set when the text is valid but was NOT written because it makes a
    /// change that needs confirming first. See `guard`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<crate::guard::ConfirmRequest>,
    /// Set when the text was not written because the file changed since the
    /// caller read it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub conflict: bool,
}

impl Validation {
    /// A single error with no location.
    pub fn error(message: String) -> Self {
        Self {
            ok: false,
            diagnostics: vec![DiagnosticView {
                severity: "error".into(),
                path: String::new(),
                message,
                line: None,
                col: None,
            }],
            ..Self::default()
        }
    }

    /// Validate a candidate configuration without touching the disk.
    pub fn of(raw: &str, path: &Path) -> Self {
        match config::parse_str(raw, path) {
            Ok(loaded) => Self {
                ok: true,
                diagnostics: loaded
                    .warnings
                    .iter()
                    .map(|d| DiagnosticView::new(d, raw))
                    .collect(),
                ..Self::default()
            },
            Err(ConfigError::Invalid { diagnostics, .. }) => Self {
                ok: false,
                diagnostics: diagnostics
                    .iter()
                    .map(|d| DiagnosticView::new(d, raw))
                    .collect(),
                ..Self::default()
            },
            // A TOML syntax error has a span but no dotted path, so it is
            // reported against the file itself rather than against a key.
            Err(ConfigError::Parse { message, span, .. }) => {
                let (line, col) = span
                    .as_ref()
                    .map(|s| line_col(raw, s.start))
                    .map(|(l, c)| (Some(l), Some(c)))
                    .unwrap_or((None, None));
                Self {
                    ok: false,
                    diagnostics: vec![DiagnosticView {
                        severity: "error".into(),
                        path: String::new(),
                        message,
                        line,
                        col,
                    }],
                    ..Self::default()
                }
            }
            Err(e) => Self::error(e.to_string()),
        }
    }
}

/// 1-based line and column of a byte offset.
fn line_col(raw: &str, offset: usize) -> (usize, usize) {
    let start = offset.min(raw.len());
    let before = &raw[..start];
    (
        before.matches('\n').count() + 1,
        before
            .rsplit('\n')
            .next()
            .map(|s| s.chars().count())
            .unwrap_or(0)
            + 1,
    )
}

/// What `--config` / `$BRIDGEWATCH_CONFIG` / the platform config directory
/// resolved to, and what state it is in.
pub struct Resolved {
    /// The file that will be read.
    pub path: PathBuf,
    /// True when this run wrote the shipped example because nothing was there.
    /// Nothing sets it any more: seeding was replaced by the setup wizard, which
    /// writes only what the user answered. It still travels to the frontend in
    /// `Status`, where nothing reads it either (the Settings banner that did was
    /// removed as unreachable), so it is dead on both sides and kept only
    /// because taking it off the wire is a Rust and TypeScript change together.
    pub seeded: bool,
    /// True when there is no file at the DEFAULT path yet: a first launch. The
    /// shell opens the setup wizard instead of writing anything.
    pub first_run: bool,
    /// The configuration, when it loaded.
    pub loaded: Option<Loaded>,
    /// Why it did not, when it did not.
    pub validation: Validation,
    /// The text that was read, if any.
    pub text: Option<String>,
}

/// The error-strip line while a first launch has no file yet.
pub const FIRST_RUN_MESSAGE: &str = "No configuration yet. Finish the setup wizard, or choose \
     \"I'll edit config.toml\" to write one yourself in Settings.";

/// Resolve the config path. Nothing is written here.
///
/// A missing file at the DEFAULT path is a first launch: `first_run` is set and
/// the shell opens the setup wizard, which writes the file when it is finished.
/// Skipping the wizard writes nothing at all; Settings opens on an empty text
/// tab, and the first Save there creates the file. The parent directory is
/// created so the file watcher, which watches it, is armed before the file
/// exists.
///
/// ⛔ A `--config` or `$BRIDGEWATCH_CONFIG` naming a file that does not exist
/// is far more likely a typo than a request for a new file, and seeding there
/// used to create `~/bw.tmol` silently and monitor the example's project
/// instead of the user's. That case is reported instead; a Save from the text
/// tab still creates the file, as an explicit act.
pub fn resolve(explicit: Option<&Path>) -> Resolved {
    resolve_at(&config::resolve_path(explicit), &config::default_path())
}

/// [`resolve`] with both paths given, for tests.
pub fn resolve_at(path: &Path, default: &Path) -> Resolved {
    let path = path.to_path_buf();
    let raw = std::fs::read_to_string(&path);
    let missing = matches!(&raw, Err(e) if e.kind() == std::io::ErrorKind::NotFound);
    let first_run = missing && path == default;
    if first_run && let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (validation, loaded) = match &raw {
        Ok(raw) => (
            Validation::of(raw, &path),
            config::parse_str(raw, &path).ok(),
        ),
        Err(_) if first_run => (Validation::error(FIRST_RUN_MESSAGE.to_string()), None),
        Err(_) if missing => (
            Validation::error(format!(
                "{} does not exist. It was named explicitly, so it was not created; \
                 check the path, or save from this tab to create it",
                path.display()
            )),
            None,
        ),
        Err(e) => (
            Validation::error(format!("could not read {}: {e}", path.display())),
            None,
        ),
    };

    Resolved {
        path,
        seeded: false,
        first_run,
        loaded,
        validation,
        text: raw.ok(),
    }
}

/// Why a guarded write did not happen.
#[derive(Debug)]
pub enum WriteError {
    /// The file no longer holds the text the edit was based on.
    Conflict,
    /// The file system refused.
    Io(std::io::Error),
}

/// Replace the file's contents only if it still holds `base`.
///
/// ⛔ The compare-and-swap every GUI write goes through. A settings window
/// holds a copy of the file from whenever it last read it, and without this a
/// save from that copy silently reverts whatever `$EDITOR`, the CLI or the
/// other window wrote since. A missing file compares equal to `""`, which is
/// what the text tab shows for it.
///
/// ⚠ The window between the read and the rename is not closed (no advisory
/// lock is shared with editors, and none would honour one); it is a few
/// microseconds instead of the hour a settings window can sit open.
pub fn write_if_unchanged(path: &Path, base: &str, text: &str) -> Result<(), WriteError> {
    let current = match std::fs::read_to_string(path) {
        Ok(current) => current,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(WriteError::Io(e)),
    };
    if current != base {
        return Err(WriteError::Conflict);
    }
    write_atomic(path, text).map_err(WriteError::Io)
}

/// Write a file by writing a sibling and renaming it over the target.
///
/// A crash or a full disk mid-write used to leave a truncated `config.toml`,
/// which is the one file whose loss takes the monitoring down with it. A
/// rename is atomic on every file system this app runs on, and the file
/// watcher already copes with it (it watches the directory for exactly that
/// reason).
///
/// ⚠ A symlinked config (a dotfiles repository, typically) is resolved first
/// and the REAL file is replaced; renaming over the link itself would swap it
/// for a regular file and quietly detach the user's repository. The existing
/// file's permissions are carried over, so a `0600` file stays `0600`.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".into());
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));

    let result = (|| {
        std::fs::write(&temp, text)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temp, meta.permissions())?;
        }
        std::fs::rename(&temp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Watch a config file for changes and call `on_change` once per settled edit.
///
/// The PARENT directory is watched rather than the file: an editor that saves
/// by writing a temporary file and renaming it over the target replaces the
/// inode, and a watch on the old inode then sees nothing ever again. Watching
/// the directory survives that, at the cost of having to filter.
///
/// Returns the watcher, which must be kept alive — dropping it stops the watch,
/// silently. Boxed because the caller parks it in a `static` for the process
/// lifetime and an opaque type cannot be named there.
pub fn watch(
    path: &Path,
    on_change: impl Fn() + Send + 'static,
) -> Option<Box<dyn Watcher + Send>> {
    let parent = path.parent()?.to_path_buf();
    let target = path.to_path_buf();
    let (tx, rx) = mpsc::channel::<()>();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        // Compare file names, not full paths: macOS reports /private/var where
        // the config path says /var, and a full-path comparison then never
        // matches anything under $TMPDIR.
        let hit = event
            .paths
            .iter()
            .any(|p| p.file_name() == target.file_name());
        if hit {
            let _ = tx.send(());
        }
    })
    .ok()?;

    watcher.watch(&parent, RecursiveMode::NonRecursive).ok()?;

    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            // Collapse the burst: drain every further event inside one window
            // measured from the FIRST of them, then fire once. A window that
            // restarted on each event would never close while an editor with a
            // busy autosave was open.
            let deadline = Instant::now() + RELOAD_DEBOUNCE;
            while rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .is_ok()
            {}
            on_change();
        }
    });

    Some(Box::new(watcher))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_example_is_a_valid_configuration() {
        // If this fails the first-run experience is a validation error, which
        // is the worst possible first impression.
        let v = Validation::of(EXAMPLE_CONFIG, Path::new("examples/distronode.toml"));
        assert!(
            v.ok,
            "the embedded example does not validate: {:?}",
            v.diagnostics
        );
    }

    #[test]
    fn a_syntax_error_reports_a_line_and_column_rather_than_a_key() {
        let v = Validation::of("[accounts.gitlab]\nbase_url = \n", Path::new("x.toml"));
        assert!(!v.ok);
        assert_eq!(v.diagnostics.len(), 1);
        assert!(
            v.diagnostics[0].line.is_some(),
            "no line for a syntax error"
        );
    }

    #[test]
    fn a_schema_error_is_reported_against_its_key() {
        let v = Validation::of(
            "[accounts.gitlab]\nbase_url = \"gitlab.com\"\n[[watches]]\nid = \"a\"\naccount = \"gitlab\"\nproject = 1\n",
            Path::new("x.toml"),
        );
        assert!(!v.ok);
        assert!(
            v.diagnostics
                .iter()
                .any(|d| d.path == "accounts.gitlab.base_url"),
            "{:?}",
            v.diagnostics
        );
    }

    /// A scratch directory of its own, so the parallel test threads do not
    /// see each other's writes through a shared parent watch.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bridgewatch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn a_write_to_the_watched_file_fires_a_reload() {
        let dir = scratch("watch-write");
        let path = dir.join("config.toml");
        std::fs::write(&path, EXAMPLE_CONFIG).unwrap();

        let (tx, rx) = mpsc::channel();
        let watcher = watch(&path, move || {
            let _ = tx.send(());
        })
        .expect("a watcher");

        std::fs::write(&path, format!("{EXAMPLE_CONFIG}\n# touched\n")).unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("hot reload never fired for a plain write");

        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rename_over_the_file_fires_a_reload_too() {
        // ⛔ The case the parent-directory watch exists for. vim, VS Code and
        // `sed -i` all save by writing a temporary file and renaming it over
        // the target, which REPLACES THE INODE — a watch on the file itself
        // sees that once and then nothing, ever, with no error anywhere.
        let dir = scratch("watch-rename");
        let path = dir.join("config.toml");
        std::fs::write(&path, EXAMPLE_CONFIG).unwrap();

        let (tx, rx) = mpsc::channel();
        let watcher = watch(&path, move || {
            let _ = tx.send(());
        })
        .expect("a watcher");

        let temp = dir.join("config.toml.tmp");
        std::fs::write(&temp, format!("{EXAMPLE_CONFIG}\n# renamed in\n")).unwrap();
        std::fs::rename(&temp, &path).unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("hot reload never fired for a rename-over-the-file save");

        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_based_on_stale_text_is_refused_and_the_file_is_untouched() {
        let dir = scratch("cas");
        let path = dir.join("config.toml");
        std::fs::write(&path, "# external edit\n").unwrap();

        let refused = write_if_unchanged(&path, "# what the window read\n", "# window\n");
        assert!(matches!(refused, Err(WriteError::Conflict)), "{refused:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# external edit\n");

        write_if_unchanged(&path, "# external edit\n", "# window\n").expect("a current base");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# window\n");

        // A file that does not exist yet is what an empty editor was based on.
        let fresh = dir.join("new.toml");
        write_if_unchanged(&fresh, "", "# created\n").expect("creating from empty");
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "# created\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_atomic_write_keeps_a_symlink_and_the_file_mode() {
        let dir = scratch("atomic");
        let real = dir.join("real.toml");
        let link = dir.join("config.toml");
        std::fs::write(&real, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::os::unix::fs::symlink(&real, &link).unwrap();
        }
        #[cfg(not(unix))]
        std::fs::copy(&real, &link).unwrap();

        write_atomic(&link, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "the symlink was replaced by a regular file"
            );
            assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
            let mode = std::fs::metadata(&real).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_first_launch_writes_nothing_and_says_so() {
        // The wizard replaced seeding on a first run: nothing may appear on
        // disk until the user finishes it or chooses to edit by hand.
        let dir = scratch("first-run");
        let default = dir.join("sub").join("config.toml");
        let r = resolve_at(&default, &default);
        assert!(r.first_run);
        assert!(!r.seeded);
        assert!(!default.exists(), "a first launch wrote a file");
        assert!(
            default.parent().unwrap().is_dir(),
            "the watched directory is missing"
        );
        assert_eq!(r.validation.diagnostics[0].message, FIRST_RUN_MESSAGE);

        let typo = dir.join("config.tmol");
        let r = resolve_at(&typo, &default);
        assert!(
            !r.first_run,
            "an explicit missing path is not a first launch"
        );
        assert!(
            r.validation.diagnostics[0]
                .message
                .contains("named explicitly")
        );

        std::fs::write(&default, EXAMPLE_CONFIG).unwrap();
        let r = resolve_at(&default, &default);
        assert!(!r.first_run && r.loaded.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warnings_do_not_make_a_configuration_unusable() {
        let v = Validation::of(
            "[accounts.gitlab]\nbase_url = \"https://gitlab.com\"\n[[watches]]\nid = \"a\"\naccount = \"gitlab\"\nproject = 1\n",
            Path::new("x.toml"),
        );
        assert!(v.ok);
        // No deploy markers on a primary watch is a warning, not an error.
        assert!(!v.diagnostics.is_empty());
    }
}
