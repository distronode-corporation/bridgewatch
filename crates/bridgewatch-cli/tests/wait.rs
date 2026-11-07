//! `bridgewatch wait`, run as a script or a coding agent would run it.
//!
//! ⛔ Nothing here touches the network or a keyring: every verdict is answered
//! from a recorded fixture (`--fixture`), which resolves no token, and every
//! config file and git repository is made in a per-test temp directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bridgewatch"));
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

/// The commit a fixture's newest pipeline ran on, read from its list.
fn fixture_sha(name: &str) -> String {
    let list: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture(name).join("list.json")).unwrap())
            .unwrap();
    list[0]["sha"].as_str().expect("a sha").to_string()
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("bw-wait-{tag}-{}-{n}", std::process::id()));
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

fn wait(fixture_name: &str, extra: &[&str]) -> Output {
    run(bin()
        .arg("--config")
        .arg(example_config())
        .args(["wait", "--fixture"])
        .arg(fixture(fixture_name))
        .args(extra))
}

/// Each final verdict exits with the code `check` uses for it.
#[test]
fn wait_exits_with_the_commit_s_verdict_code() {
    for (name, expected, state) in [
        ("4cfaced9-deployed", 0, "deployed"),
        ("synth-dead-bridge", 1, "failed"),
        ("ca41ab28-deployed-with-failure", 2, "deployed_with_failure"),
        ("synth-parent-gate", 3, "parked_gate"),
    ] {
        let sha = fixture_sha(name);
        let out = wait(name, &["--sha", &sha, "--timeout", "30s"]);
        assert_eq!(
            code(&out),
            expected,
            "{name}: stdout {} stderr {}",
            stdout(&out),
            stderr(&out)
        );
        let line = stdout(&out);
        assert!(
            line.starts_with("main-push ") && line.contains(&format!(": {state} ")),
            "{name}: {line}"
        );
    }
}

/// `--json` prints one object: the verdict, its exit code and the pipeline.
#[test]
fn wait_json_prints_the_final_verdict() {
    let sha = fixture_sha("4cfaced9-deployed");
    let out = wait("4cfaced9-deployed", &["--sha", &sha[..8], "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("one JSON object");
    assert_eq!(v["watch"], "main-push");
    assert_eq!(v["sha"], sha[..8].to_string());
    assert_eq!(v["state"], "deployed");
    assert_eq!(v["exit"], 0);
    assert_eq!(v["timed_out"], false);
    assert_eq!(v["pipeline"]["id"], 2856963900u64);
    assert_eq!(v["pipeline"]["sha"], sha);
}

/// A pipeline still in flight is not a verdict, even one that already reads
/// `failed`: `--timeout` ends the wait with 124.
#[test]
fn a_pipeline_still_running_times_out_with_124() {
    let name = "synth-marker-failed-live-sibling";
    let sha = fixture_sha(name);
    let out = wait(name, &["--sha", &sha, "--timeout", "2s", "--json"]);
    assert_eq!(code(&out), 124, "stdout {} stderr {}", stdout(&out), stderr(&out));
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("JSON");
    assert_eq!(v["timed_out"], true);
    assert_eq!(v["exit"], 124);
    assert_eq!(v["pipeline"]["live"], true);
    let err = stderr(&out);
    assert!(err.contains("waiting: main-push "), "{err}");
    assert!(err.contains("timed out after 2s"), "{err}");
}

/// A commit with no pipeline yet is waited for, not refused.
#[test]
fn a_commit_with_no_pipeline_yet_is_waited_for() {
    let out = wait("4cfaced9-deployed", &["--sha", "deadbeefcafe", "--timeout", "1s"]);
    assert_eq!(code(&out), 124, "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "stdout carries only the answer");
    let err = stderr(&out);
    assert!(err.contains("no pipeline for deadbeefcafe on main-push yet"), "{err}");
    assert!(err.contains("last seen: no pipeline"), "{err}");
}

/// Without `--sha` it waits on the current directory's HEAD.
#[test]
fn without_sha_it_waits_on_head() {
    let dir = TempDir::new("head");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&dir.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
        stdout(&out)
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "x"]);
    let head = git(&["rev-parse", "HEAD"]).trim().to_string();

    let out = run(bin()
        .current_dir(&dir.0)
        .arg("--config")
        .arg(example_config())
        .args(["wait", "--timeout", "1s", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 124, "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("no pipeline for {head} on main-push")),
        "{}",
        stderr(&out)
    );
}

/// Outside a repository, and with no `--sha`, there is nothing to wait on.
#[test]
fn without_sha_outside_a_repository_is_a_usage_error() {
    let dir = TempDir::new("norepo");
    let out = run(bin()
        .current_dir(&dir.0)
        .env("GIT_CEILING_DIRECTORIES", dir.0.parent().unwrap())
        .arg("--config")
        .arg(example_config())
        .args(["wait", "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("no --sha given"), "{}", stderr(&out));
}

/// The command line's mistakes are usage errors (64), never a verdict.
#[test]
fn bad_arguments_are_usage_errors() {
    let sha = fixture_sha("4cfaced9-deployed");
    for args in [
        vec!["--sha", "main"],
        vec!["--sha", "4cface"],
        vec!["--sha", &sha, "--timeout", "soon"],
        vec!["--sha", &sha, "--timeout", "0"],
        vec!["--sha", &sha, "--watch", "nope"],
    ] {
        let out = wait("4cfaced9-deployed", &args);
        assert_eq!(code(&out), 64, "{args:?}: {}", stderr(&out));
    }
}

/// With one primary watch among several it needs no `--watch`; with two
/// primary watches it has to be told.
#[test]
fn the_watch_is_inferred_only_when_there_is_one_answer() {
    let dir = TempDir::new("two");
    let config = dir.write(
        "config.toml",
        r#"
[accounts.gitlab]
token = { env = "BRIDGEWATCH_TEST_TOKEN_NEVER_SET" }

[[watches]]
id = "a"
account = "gitlab"
project = 12345678
ref = "main"

[[watches]]
id = "b"
account = "gitlab"
project = 12345678
ref = "main"
"#,
    );
    let sha = fixture_sha("4cfaced9-deployed");
    let out = run(bin()
        .arg("--config")
        .arg(&config)
        .args(["wait", "--sha", &sha, "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("name one with --watch"), "{}", stderr(&out));

    let out = run(bin()
        .arg("--config")
        .arg(&config)
        .args(["wait", "--watch", "b", "--sha", &sha, "--fixture"])
        .arg(fixture("4cfaced9-deployed")));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).starts_with("b "), "{}", stdout(&out));
}

/// `--help` documents 124.
#[test]
fn the_help_documents_the_timeout_code() {
    let out = run(bin().arg("--help"));
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("124 wait: --timeout ran out"), "{}", stdout(&out));
}
