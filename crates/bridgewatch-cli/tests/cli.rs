//! The `bridgewatch` binary, run as a script would run it.
//!
//! These drive the built executable through `std::process::Command`, because
//! what a script depends on is the process boundary: the exit code, which
//! stream a line lands on, and which arguments clap accepts where. None of that
//! is visible from inside `main`.
//!
//! ⛔ Nothing here touches the network or a keyring. Every verdict is answered
//! from a recorded fixture (`check --fixture`), which resolves no token, and
//! every config file is written to a per-test temp directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

/// The binary under test, built by cargo for this test target.
fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bridgewatch"));
    // A developer's own settings must not leak into what is being asserted.
    cmd.env_remove("BRIDGEWATCH_CONFIG").env_remove("RUST_LOG");
    cmd
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("the binary runs")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exited, not signalled")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn example_config() -> PathBuf {
    repo_root().join("examples/distronode.toml")
}

fn fixture(name: &str) -> PathBuf {
    repo_root()
        .join("crates/bridgewatch-core/tests/fixtures")
        .join(name)
}

/// A temp directory unique to one call, removed when dropped.
///
/// A counter as well as the pid: tests in one binary share a pid and run on
/// parallel threads, and a pid-only name is how the notify suite's ledgers
/// once read each other.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("bw-cli-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, body).expect("write");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One account, one primary watch on `main`. The fixtures in the core crate
/// were recorded against project 82468124, which is what this names.
const MINIMAL: &str = r#"
[accounts.gitlab]
token = { env = "BRIDGEWATCH_TEST_TOKEN_NEVER_SET" }

[[watches]]
id = "main-push"
account = "gitlab"
project = 82468124
ref = "main"
sources = ["push"]
deploy_markers = ["deploy:origins", "deploy:marketing"]
"#;

/// The same shape with the one watch declared SECONDARY, which is what
/// `--watch` on a rows-only watch is about. Both sources are accepted so that
/// one file answers a scheduled fixture and a pushed one.
const SECONDARY_ONLY: &str = r#"
[accounts.gitlab]
token = { env = "BRIDGEWATCH_TEST_TOKEN_NEVER_SET" }

[[watches]]
id = "hourly"
account = "gitlab"
project = 82468124
ref = "main"
role = "secondary"
sources = ["push", "schedule"]
deploy_markers = ["deploy:origins", "deploy:marketing"]
"#;

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

/// Each verdict has its own exit code, and the recorded fixtures cover every
/// one of them. The mapping is the documented table in `--help`.
#[test]
fn check_exits_with_the_verdict_code() {
    for (name, expected) in [
        ("4cfaced9-deployed", 0),
        ("synth-dead-bridge", 1),
        ("ca41ab28-deployed-with-failure", 2),
        ("synth-parent-gate", 3),
    ] {
        let out = run(bin()
            .arg("--config")
            .arg(example_config())
            .args(["check", "--fixture"])
            .arg(fixture(name)));
        assert_eq!(
            code(&out),
            expected,
            "{name}: stdout {} stderr {}",
            stdout(&out),
            stderr(&out)
        );
    }
}

/// ⛔ `unknown` is its own code, distinct from every verdict AND every error.
/// A script has to be able to tell "could not see" from "saw green" and from
/// "the command line was wrong".
#[test]
fn a_watch_that_sees_nothing_exits_unknown() {
    let dir = TempDir::new("unknown");
    let config = dir.write(
        "config.toml",
        &MINIMAL.replace(r#"ref = "main""#, r#"ref = "no-such-branch""#),
    );
    let out = run(bin()
        .arg("--config")
        .arg(&config)
        .args(["check", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(
        code(&out),
        4,
        "stdout {} stderr {}",
        stdout(&out),
        stderr(&out)
    );

    // The same answer as JSON, so the code is checkably the icon state's.
    let out = run(bin()
        .arg("--config")
        .arg(&config)
        .args(["check", "--json", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 4);
    let snapshot: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("JSON on stdout");
    assert_eq!(snapshot["icon_state"], "unknown", "{snapshot}");
}

/// ⛔ Naming a watch makes the watches NAMED the subject of the question.
/// Folding primary watches only (which is right for the tray, where a
/// secondary watch is rows and never a colour) meant `check --watch <a
/// secondary watch>` printed `icon: unknown` and exited 4, "could not see",
/// over a verdict it had read perfectly well. A failed schedule never reached
/// exit 1.
#[test]
fn a_secondary_only_selection_exits_with_the_verdict_it_read() {
    let dir = TempDir::new("secondary");
    let config = dir.write("config.toml", SECONDARY_ONLY);

    for (name, expected, icon) in [
        ("schedule-hourly-failed", 1, "failed"),
        ("04eff5c5-docs-only", 0, "succeeded_no_deploy"),
    ] {
        let out = run(bin()
            .arg("--config")
            .arg(&config)
            .args(["check", "--watch", "hourly", "--fixture"])
            .arg(fixture(name)));
        assert_eq!(
            code(&out),
            expected,
            "{name}: stdout {} stderr {}",
            stdout(&out),
            stderr(&out)
        );
        // The printed line, the JSON and the code all read the same field, and
        // this is what holds them to it.
        assert!(
            stdout(&out).starts_with(&format!("icon: {icon}\n")),
            "{name}: {}",
            stdout(&out)
        );

        let out = run(bin()
            .arg("--config")
            .arg(&config)
            .args(["check", "--watch", "hourly", "--json", "--fixture"])
            .arg(fixture(name)));
        assert_eq!(code(&out), expected, "{name} as JSON");
        let snapshot: serde_json::Value =
            serde_json::from_str(&stdout(&out)).expect("JSON on stdout");
        assert_eq!(snapshot["icon_state"], icon, "{name}: {snapshot}");
    }
}

/// A selection that still holds a primary watch keeps the tray's rule: a
/// secondary watch cannot outvote a primary one merely by being asked for
/// alongside it. The fixture is a green push and a red schedule in one list.
#[test]
fn a_selection_holding_a_primary_watch_still_follows_the_primaries() {
    let out = run(bin()
        .arg("--config")
        .arg(example_config())
        .args([
            "check",
            "--watch",
            "hourly",
            "--watch",
            "main-push",
            "--fixture",
        ])
        .arg(fixture("mixed-primary-green-secondary-red")));
    assert_eq!(code(&out), 0, "stdout {}", stdout(&out));
    assert!(
        stdout(&out).starts_with("icon: deployed\n"),
        "{}",
        stdout(&out)
    );
}

/// ⛔ With no `--watch` the rule is unchanged: primaries only. The same fixture
/// that exits 1 when the schedule is NAMED exits 4 when it is not, because
/// nothing asked about the schedule and no push has been looked at.
#[test]
fn no_watch_filter_leaves_the_primary_rule_alone() {
    let out = run(bin()
        .arg("--config")
        .arg(example_config())
        .args(["check", "--fixture"])
        .arg(fixture("schedule-hourly-failed")));
    assert_eq!(code(&out), 4, "stdout {}", stdout(&out));
    assert!(
        stdout(&out).starts_with("icon: unknown\n"),
        "{}",
        stdout(&out)
    );

    // Named, the same watch in the same file is 1.
    let out = run(bin()
        .arg("--config")
        .arg(example_config())
        .args(["check", "--watch", "hourly", "--fixture"])
        .arg(fixture("schedule-hourly-failed")));
    assert_eq!(code(&out), 1, "stdout {}", stdout(&out));
}

/// ⛔ And the code the fix must NOT swallow: a selection that read nothing is
/// still `unknown`. "Could not see" has to stay distinguishable from "saw
/// green", which is the whole reason 4 exists.
#[test]
fn a_selection_that_read_nothing_is_still_unknown() {
    let dir = TempDir::new("secondary-blind");
    let config = dir.write(
        "config.toml",
        &SECONDARY_ONLY.replace(r#"ref = "main""#, r#"ref = "no-such-branch""#),
    );
    let out = run(bin()
        .arg("--config")
        .arg(&config)
        .args(["check", "--watch", "hourly", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 4, "stdout {}", stdout(&out));
}

/// `watch` takes the same selection rule as `check`, because a script that can
/// poll for a schedule needs the same answer the one-shot gives it.
#[test]
fn watch_applies_the_selection_rule_too() {
    let dir = TempDir::new("secondary-watch");
    let config = dir.write("config.toml", SECONDARY_ONLY);
    let out = run(bin()
        .arg("--config")
        .arg(&config)
        .args(["watch", "--ticks", "1", "--watch", "hourly", "--fixture"])
        .arg(fixture("schedule-hourly-failed")));
    assert_eq!(code(&out), 1, "stderr {}", stderr(&out));
    assert!(stdout(&out).contains("failed"), "{}", stdout(&out));
}

/// `watch --ticks N` ends with the same code `check` would give.
#[test]
fn watch_with_a_tick_limit_exits_with_the_verdict_code() {
    let out = run(bin()
        .arg("--config")
        .arg(example_config())
        .args(["watch", "--ticks", "1", "--fixture"])
        .arg(fixture("ca41ab28-deployed-with-failure")));
    assert_eq!(code(&out), 2, "stderr {}", stderr(&out));
    assert!(!stdout(&out).trim().is_empty(), "one line per change");
}

/// ⛔ A usage error is 64, never 2: clap's own code for it is 2, which is
/// also `deployed_with_failure`.
#[test]
fn a_usage_error_is_not_a_verdict() {
    let out = run(bin().arg("no-such-subcommand"));
    assert_eq!(code(&out), 64, "{}", stderr(&out));

    let out = run(bin().args(["check", "--no-such-flag"]));
    assert_eq!(code(&out), 64);

    // A watch id that is not in the file is the command line's fault, not the
    // file's, and not a verdict.
    let out = run(bin()
        .arg("--config")
        .arg(example_config())
        .args(["check", "--watch", "nope", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 64);
    assert!(
        stderr(&out).contains("no watch with id"),
        "{}",
        stderr(&out)
    );
}

/// A configuration that cannot be used is 78, never 1 (`failed`).
#[test]
fn a_config_that_cannot_be_used_is_not_a_verdict() {
    let dir = TempDir::new("badconfig");
    let missing = dir.0.join("absent.toml");
    let out = run(bin()
        .arg("--config")
        .arg(&missing)
        .args(["check", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 78, "{}", stderr(&out));

    let invalid = dir.write(
        "invalid.toml",
        &MINIMAL.replace(r#"account = "gitlab""#, r#"account = "elsewhere""#),
    );
    let out = run(bin()
        .arg("--config")
        .arg(&invalid)
        .args(["check", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 78, "{}", stderr(&out));
}

/// `--help` and `--version` are successes, on stdout.
#[test]
fn help_and_version_exit_zero_and_help_documents_every_code() {
    let out = run(bin().arg("--help"));
    assert_eq!(code(&out), 0);
    let help = stdout(&out);
    assert!(help.contains("Exit codes:"), "{help}");
    for code in ["0 ", "1 ", "2 ", "3 ", "4 ", "64 ", "70 ", "78 "] {
        assert!(
            help.lines().any(|l| l.trim_start().starts_with(code)),
            "exit code {code}is not documented in --help:\n{help}"
        );
    }
    assert!(help.contains("unknown"), "{help}");

    let out = run(bin().arg("--version"));
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).starts_with("bridgewatch "), "{}", stdout(&out));
}

// ---------------------------------------------------------------------------
// config path
// ---------------------------------------------------------------------------

/// ⛔ H18. `--config` is global: before the subcommand (the order the README
/// shows), after it, and on subcommands that read no file at all.
#[test]
fn config_path_honours_config_in_either_position() {
    let target = "/tmp/somewhere/bridgewatch.toml";
    for args in [
        vec!["--config", target, "config", "path"],
        vec!["config", "path", "--config", target],
        vec!["config", "--config", target, "path"],
        vec!["-c", target, "config", "path"],
    ] {
        let out = run(bin().args(&args));
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
        assert_eq!(stdout(&out).trim(), target, "{args:?}");
    }
}

/// The environment variable is read when there is no flag, and the flag wins
/// over it.
#[test]
fn config_path_reads_the_environment_and_the_flag_wins() {
    let out = run(bin()
        .env("BRIDGEWATCH_CONFIG", "/tmp/from-env.toml")
        .args(["config", "path"]));
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out).trim(), "/tmp/from-env.toml");

    let out = run(bin().env("BRIDGEWATCH_CONFIG", "/tmp/from-env.toml").args([
        "config",
        "path",
        "--config",
        "/tmp/from-flag.toml",
    ]));
    assert_eq!(stdout(&out).trim(), "/tmp/from-flag.toml");
}

/// ⛔ H18. `BRIDGEWATCH_CONFIG=""` is "unset", as it is in the app. clap read
/// it as an empty path and refused it with a usage error.
#[test]
fn an_empty_config_variable_counts_as_unset() {
    let unset = run(bin().args(["config", "path"]));
    let empty = run(bin().env("BRIDGEWATCH_CONFIG", "").args(["config", "path"]));
    assert_eq!(code(&empty), 0, "{}", stderr(&empty));
    assert_eq!(stdout(&empty), stdout(&unset));
    assert!(stdout(&unset).trim().ends_with("config.toml"));
}

// ---------------------------------------------------------------------------
// config validate
// ---------------------------------------------------------------------------

/// The shipped example validates, and says what it found.
#[test]
fn config_validate_accepts_the_example() {
    let out = run(bin()
        .arg("--config")
        .arg(example_config())
        .args(["config", "validate"]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).contains(": ok ("), "{}", stdout(&out));
}

/// The shipped GitHub example validates with no error and exactly the one
/// warning its own comment explains.
#[test]
fn config_validate_accepts_the_github_example() {
    let out = run(bin()
        .arg("--config")
        .arg(repo_root().join("examples/github.toml"))
        .args(["config", "validate"]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains(": ok (1 account(s), 2 watch(es), 1 warning(s))"),
        "{}",
        stdout(&out)
    );
    let err = stderr(&out);
    let warnings: Vec<&str> = err.lines().filter(|l| l.starts_with("warning:")).collect();
    assert_eq!(warnings.len(), 1, "{err}");
    assert!(warnings[0].contains("watches.0.deploy_markers"), "{err}");
    assert!(!err.contains("error:"), "{err}");
}

/// ⛔ M16. A TOML syntax error is reported as `path:line:col`, on stderr, and
/// exits 78.
#[test]
fn config_validate_puts_a_syntax_error_at_its_line_and_column() {
    let dir = TempDir::new("syntax");
    let path = dir.write(
        "config.toml",
        "[accounts.gl]\ntoken = { env = \"T\" }\nbase_url = \n",
    );
    let out = run(bin()
        .arg("--config")
        .arg(&path)
        .args(["config", "validate"]));
    assert_eq!(code(&out), 78);
    let err = stderr(&out);
    assert!(
        err.contains(&format!("{}:3:", path.display())),
        "no line:col for the syntax error: {err}"
    );
}

/// ⛔ M16. An unknown key is a warning with its line, and does not fail the
/// file.
#[test]
fn config_validate_puts_an_unknown_key_at_its_line() {
    let dir = TempDir::new("unknownkey");
    let path = dir.write(
        "config.toml",
        &MINIMAL.replacen(
            "[accounts.gitlab]\n",
            "[accounts.gitlab]\ncolour = \"red\"\n",
            1,
        ),
    );
    let out = run(bin()
        .arg("--config")
        .arg(&path)
        .args(["config", "validate"]));
    assert_eq!(
        code(&out),
        0,
        "a warning is not a failure: {}",
        stderr(&out)
    );
    let err = stderr(&out);
    // Line 3: the constant opens with a newline, then the table header.
    assert!(
        err.contains(&format!("warning: {}:3:", path.display())) && err.contains("colour"),
        "no line:col for the unknown key: {err}"
    );
    assert!(stdout(&out).contains("1 warning(s)"), "{}", stdout(&out));
}

/// Every error in a file that does not validate is printed, each with its
/// position, and the file exits 78.
#[test]
fn config_validate_reports_every_error_with_a_position() {
    let dir = TempDir::new("invalid");
    let path = dir.write(
        "config.toml",
        &MINIMAL
            .replace(r#"account = "gitlab""#, r#"account = "elsewhere""#)
            .replace(r#"ref = "main""#, r#"ref = "re:[unclosed""#),
    );
    let out = run(bin()
        .arg("--config")
        .arg(&path)
        .args(["config", "validate"]));
    assert_eq!(code(&out), 78);
    let err = stderr(&out);
    let located: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with(&format!("error: {}:", path.display())))
        .collect();
    assert!(located.len() >= 2, "both errors, each located: {err}");
    assert!(err.contains("watches.0.account"), "{err}");
    assert!(err.contains("watches.0.ref"), "{err}");
}

// ---------------------------------------------------------------------------
// config schema
// ---------------------------------------------------------------------------

/// The schema is JSON Schema, needs no config file, and accepts `--config`
/// anyway (H18: it used to be rejected there).
#[test]
fn config_schema_prints_a_json_schema_without_a_config_file() {
    let dir = TempDir::new("schema");
    for args in [
        vec!["config".to_string(), "schema".to_string()],
        vec![
            "--config".to_string(),
            dir.0.join("absent.toml").display().to_string(),
            "config".to_string(),
            "schema".to_string(),
        ],
    ] {
        let out = run(bin().args(&args));
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
        let schema: serde_json::Value =
            serde_json::from_str(&stdout(&out)).expect("the schema is JSON");
        assert!(
            schema["$schema"]
                .as_str()
                .is_some_and(|s| s.contains("json-schema")),
            "{schema}"
        );
        for key in ["accounts", "watches", "icon", "log"] {
            assert!(
                schema["properties"].get(key).is_some(),
                "the schema describes `{key}`"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// fixture scrub
// ---------------------------------------------------------------------------

/// ⛔ M26. `fixture scrub --check` is a gate that READS: 0 on a clean fixture,
/// 1 on a dirty one, and the dirty file is left byte for byte as it was.
#[test]
fn fixture_scrub_check_gates_without_writing() {
    let dir = TempDir::new("scrub");
    let clean = dir.0.join("clean");
    std::fs::create_dir_all(&clean).unwrap();
    for entry in std::fs::read_dir(fixture("4cfaced9-deployed")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), clean.join(entry.file_name())).unwrap();
    }
    let out = run(bin().args(["fixture", "scrub", "--check"]).arg(&clean));
    assert_eq!(
        code(&out),
        0,
        "a committed fixture is clean: {}",
        stdout(&out)
    );

    let dirty = dir.0.join("dirty");
    std::fs::create_dir_all(&dirty).unwrap();
    let raw = "[\n  {\n    \"id\": 1,\n    \"name\": \"x\",\n    \"user\": { \"name\": \"A Person\" }\n  }\n]\n";
    let jobs = dirty.join("jobs.json");
    std::fs::write(&jobs, raw).unwrap();
    let out = run(bin().args(["fixture", "scrub", "--check"]).arg(&dirty));
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    assert!(stdout(&out).contains("would scrub"), "{}", stdout(&out));
    assert_eq!(
        std::fs::read_to_string(&jobs).unwrap(),
        raw,
        "--check wrote"
    );

    // And a path that is not a directory is the command line's fault.
    let out = run(bin().args(["fixture", "scrub", "--check"]).arg(&jobs));
    assert_eq!(code(&out), 64);
}

/// ⛔ H18. `fixture record` takes no required `--out`; the README runs it as
/// `bridgewatch fixture record <id>`. Checked at the parser only (`--help`),
/// because recording needs the network.
#[test]
fn fixture_record_does_not_require_out() {
    let out = run(bin().args(["fixture", "record", "--help"]));
    assert_eq!(code(&out), 0);
    let help = stdout(&out);
    let usage = help
        .lines()
        .find(|l| l.starts_with("Usage:"))
        .expect("a usage line");
    assert!(
        !usage.contains("--out"),
        "--out is optional, so the usage line must not demand it: {usage}"
    );
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

/// From flags to a config that loads, printed and never written.
#[test]
fn init_prints_a_config_that_validates_and_writes_nothing() {
    let dir = TempDir::new("init-new");
    let path = dir.0.join("config.toml");
    let out = run(bin().args(["--config"]).arg(&path).args([
        "init",
        "--project",
        "https://gitlab.com/group/app",
        "--glab",
        "--deploy-marker",
        "deploy:production",
        "--schedule",
        "--preflight",
        "pf/*",
        "--live-secs",
        "5",
    ]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!path.exists(), "init prints; it never writes");
    let text = stdout(&out);
    assert!(
        text.contains("service = \"glab:gitlab.com:token\""),
        "{text}"
    );
    assert!(text.contains("id = \"app-main\""), "{text}");
    assert!(text.contains("id = \"app-main-schedule\""), "{text}");
    assert!(stderr(&out).contains("nothing written"), "{}", stderr(&out));

    // What it printed is a config the validator accepts.
    let written = dir.write("printed.toml", &text);
    let check = run(bin()
        .args(["--config"])
        .arg(&written)
        .args(["config", "validate"]));
    assert_eq!(code(&check), 0, "{}", stderr(&check));
}

/// Against an existing file it edits: every comment of the shipped example
/// survives, the file on disk is untouched, and the new watch is appended.
#[test]
fn init_edits_an_existing_config_rather_than_replacing_it() {
    let dir = TempDir::new("init-edit");
    let original = std::fs::read_to_string(example_config()).unwrap();
    let path = dir.write("config.toml", &original);
    let out = run(bin().args(["--config"]).arg(&path).args([
        "init",
        "--project",
        "82468124",
        "--ref",
        "release",
        "--glab",
    ]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    for line in original.lines().filter(|l| l.trim_start().starts_with('#')) {
        assert!(text.contains(line), "comment lost: {line}");
    }
    assert!(text.contains("id = \"82468124-release\""), "{text}");
    assert!(text.starts_with(&original[..original.find("[[watches]]").unwrap()]));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "disk untouched"
    );
    assert!(stderr(&out).contains("edited"), "{}", stderr(&out));
}

/// The `role` line of the `7-main` watch in printed TOML.
fn role_of_7_main(text: &str) -> String {
    let block = text
        .split("id = \"7-main\"")
        .nth(1)
        .expect("the watch is there");
    let block = block.split("\n\n").next().unwrap_or("");
    block
        .lines()
        .find(|l| l.starts_with("role = "))
        .unwrap_or("(no role line)")
        .to_string()
}

/// ⛔ Found by a real run: `init` into a config that already had a primary
/// watch added a second primary, and the result warned "2 watches are
/// primary". The new watch is now secondary there, and one stderr line says
/// why and how to ask for the other answer.
#[test]
fn init_into_a_config_with_a_primary_adds_a_secondary_watch_and_says_so() {
    let dir = TempDir::new("init-second");
    let original = std::fs::read_to_string(example_config()).unwrap();
    let path = dir.write("config.toml", &original);
    let args = ["init", "--project", "7", "--glab"];

    let out = run(bin().args(["--config"]).arg(&path).args(args));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(role_of_7_main(&text), "role = \"secondary\"", "{text}");
    let err = stderr(&out);
    assert!(!err.contains("are primary"), "{err}");
    let notes: Vec<&str> = err.lines().filter(|l| l.starts_with("note:")).collect();
    assert_eq!(
        notes,
        [
            "note: added \"7-main\" as role = \"secondary\" because \"main-push\" is already \
             primary; pass --primary to make it primary too"
        ]
    );

    let out = run(bin()
        .args(["--config"])
        .arg(&path)
        .args(args)
        .arg("--primary"));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(role_of_7_main(&stdout(&out)), "role = \"primary\"");
    let err = stderr(&out);
    assert!(err.contains("2 watches are primary"), "{err}");
    assert!(!err.contains("note:"), "{err}");
}

/// Unusable answers are a usage error naming the step, and a token pasted
/// where a variable name belongs is never echoed back.
#[test]
fn init_refuses_bad_answers_as_a_usage_error() {
    let dir = TempDir::new("init-bad");
    let path = dir.0.join("config.toml");
    let out = run(bin().args(["--config"]).arg(&path).args([
        "init",
        "--project",
        "1",
        "--token-env",
        "glpat-SHOULDNOTECHO",
        "--source",
        "pushh",
    ]));
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("Account token"), "{err}");
    assert!(err.contains("Watch sources"), "{err}");
    assert!(!err.contains("SHOULDNOTECHO") && !stdout(&out).contains("SHOULDNOTECHO"));

    let out = run(bin()
        .args(["--config"])
        .arg(&path)
        .args(["init", "--project", "nope"]));
    assert_eq!(code(&out), 64, "{}", stderr(&out));

    // Two token sources at once is clap's usage error.
    let out = run(bin().args(["--config"]).arg(&path).args([
        "init",
        "--project",
        "1",
        "--glab",
        "--token-env",
        "T",
    ]));
    assert_eq!(code(&out), 64, "{}", stderr(&out));
}

/// A file that is not TOML is a config problem, and it is not replaced.
#[test]
fn init_refuses_to_edit_a_file_that_is_not_toml() {
    let dir = TempDir::new("init-broken");
    let path = dir.write("config.toml", "this is [not toml");
    let out = run(bin()
        .args(["--config"])
        .arg(&path)
        .args(["init", "--project", "1"]));
    assert_eq!(code(&out), 78, "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
}

/// `--provider github` prints a config the validator accepts with zero
/// diagnostics, and it carries none of GitLab's account keys: `base_url`,
/// `api_path` and `header` all default per provider on github.com.
#[test]
fn init_for_github_prints_a_config_that_validates_with_nothing_to_say() {
    let dir = TempDir::new("init-github");
    let path = dir.0.join("config.toml");
    let out = run(bin().args(["--config"]).arg(&path).args([
        "init",
        "--provider",
        "github",
        "--project",
        "acme-corp/monorepo",
        "--gh",
        "--workflow",
        "ci.yml",
        "--deploy-marker",
        "publish",
        "--schedule",
    ]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!path.exists(), "init prints; it never writes");
    let text = stdout(&out);
    for wanted in [
        "provider = \"github\"",
        "service = \"gh:github.com\"",
        "project = \"acme-corp/monorepo\"",
        "workflow = \"ci.yml\"",
        "id = \"monorepo-main\"",
        "id = \"monorepo-main-schedule\"",
    ] {
        assert!(text.contains(wanted), "{wanted} missing:\n{text}");
    }
    for absent in ["base_url", "api_path", "header", "glab"] {
        assert!(!text.contains(absent), "{absent} was written:\n{text}");
    }

    let written = dir.write("printed.toml", &text);
    let check = run(bin()
        .args(["--config"])
        .arg(&written)
        .args(["config", "validate"]));
    assert_eq!(code(&check), 0, "{}", stderr(&check));
    let said = format!("{}{}", stdout(&check), stderr(&check));
    assert!(
        said.contains("ok (1 account(s), 2 watch(es), 0 warning(s))"),
        "the validator had something to say about the wizard's own output:\n{said}"
    );
}

/// GitHub Enterprise Server needs its host and `/api/v3` written; the gh
/// keyring item is named for that host.
#[test]
fn init_for_github_enterprise_writes_the_host_and_the_api_prefix() {
    let dir = TempDir::new("init-ghes");
    let path = dir.0.join("config.toml");
    let out = run(bin().args(["--config"]).arg(&path).args([
        "init",
        "--provider",
        "github",
        "--base-url",
        "https://ghe.acme.com",
        "--project",
        "https://ghe.acme.com/platform/api/actions",
        "--gh",
    ]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    for wanted in [
        "[accounts.ghe]",
        "base_url = \"https://ghe.acme.com\"",
        "api_path = \"/api/v3\"",
        "service = \"gh:ghe.acme.com\"",
        "project = \"platform/api\"",
    ] {
        assert!(text.contains(wanted), "{wanted} missing:\n{text}");
    }
    let written = dir.write("printed.toml", &text);
    let check = run(bin()
        .args(["--config"])
        .arg(&written)
        .args(["config", "validate"]));
    assert_eq!(code(&check), 0, "{}", stderr(&check));
}

/// The answers a github account cannot mean are usage errors, each named: a
/// numeric project, the other CLI's keyring item, a GitLab-only source and a
/// preflight watch. And a gitlab account refuses the gh item and a workflow.
#[test]
fn init_refuses_answers_that_do_not_fit_the_provider() {
    let dir = TempDir::new("init-mismatch");
    let path = dir.0.join("config.toml");
    let github = |extra: &[&str]| {
        let mut cmd = bin();
        cmd.args(["--config"])
            .arg(&path)
            .args(["init", "--provider", "github"])
            .args(extra);
        run(&mut cmd)
    };

    let out = github(&["--project", "82468124"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("owner/repo"), "{}", stderr(&out));

    let out = github(&["--project", "acme/web", "--glab"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("--gh"), "{}", stderr(&out));

    let out = github(&["--project", "acme/web", "--source", "merge_request_event"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("Watch sources"), "{}", stderr(&out));

    let out = github(&["--project", "acme/web", "--preflight", "pf/*"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("preflight"), "{}", stderr(&out));

    let out = run(bin()
        .args(["--config"])
        .arg(&path)
        .args(["init", "--project", "g/p", "--gh"]));
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("--glab"), "{}", stderr(&out));

    let out = run(bin().args(["--config"]).arg(&path).args([
        "init",
        "--project",
        "g/p",
        "--workflow",
        "ci.yml",
    ]));
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("workflow"), "{}", stderr(&out));

    assert!(!path.exists());
}

/// ⛔ D3. `fixture record` against a github account is refused BEFORE a token
/// is resolved or a request is built: the recorder and both PII allow-lists are
/// GitLab's, so a GitHub run would be written unscrubbed. The account names a
/// keyring item that does not exist, so reaching the resolver would fail
/// differently, which is what proves the refusal comes first.
#[test]
fn fixture_record_refuses_a_github_account_before_touching_a_token() {
    let dir = TempDir::new("record-github");
    let path = dir.write(
        "config.toml",
        "[accounts.gh]\nprovider = \"github\"\ntoken = { env = \"BW_TEST_TOKEN_THAT_IS_NEVER_SET\" }\n\n\
         [[watches]]\nid = \"w\"\naccount = \"gh\"\nproject = \"acme/web\"\nref = \"main\"\n",
    );
    let out = run(bin()
        .env_remove("BW_TEST_TOKEN_THAT_IS_NEVER_SET")
        .args(["--config"])
        .arg(&path)
        .args(["fixture", "record", "1"])
        .current_dir(&dir.0));
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("github account"), "{err}");
    assert!(err.contains("PII allow-list"), "{err}");
    assert!(!err.contains("token"), "the resolver was reached: {err}");
    assert!(!dir.0.join("1").exists(), "a fixture directory was created");
}
