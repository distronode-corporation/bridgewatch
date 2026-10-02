//! Table-driven verdicts over recorded fixtures of real pipelines.
//!
//! Each `tests/fixtures/<name>/expected.json` names the icon state, the bridge
//! verdict words, the failing job names and the deploy outcome. A case may carry
//! `overrides`, a list of config edits applied to `examples/distronode.toml`
//! before the run, which is how one recording asserts two configurations.

mod support;

use std::collections::BTreeMap;

use bridgewatch_core::config::edit::Edit;
use bridgewatch_core::verdict::{Snapshot, VerdictScript};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Expected {
    /// Provenance: the real pipeline this was recorded from, or what it was
    /// synthesised from and why.
    source: String,
    #[serde(default)]
    note: String,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    /// Edits applied to the example configuration before this case runs.
    #[serde(default)]
    overrides: Vec<Edit>,
    /// The tray icon state the whole snapshot should produce.
    icon_state: String,
    /// Per-watch expectations, keyed by watch id. A watch not named here is not
    /// asserted on, but a watch named here must exist.
    watches: BTreeMap<String, ExpectedWatch>,
}

#[derive(Debug, Deserialize)]
struct ExpectedWatch {
    /// `null` for a secondary watch, which must never contribute an icon.
    #[serde(default)]
    icon_state: Option<String>,
    rows: Vec<ExpectedRow>,
}

#[derive(Debug, Deserialize)]
struct ExpectedRow {
    id: u64,
    state: String,
    deploy: String,
    #[serde(default)]
    deploy_marker: Option<String>,
    #[serde(default)]
    failures: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    gates: Vec<String>,
    #[serde(default)]
    post_deploy_failures: Vec<String>,
    #[serde(default)]
    sibling_failures: Vec<String>,
    /// Bridge name to verdict word.
    #[serde(default)]
    bridges: BTreeMap<String, String>,
}

async fn snapshot_for(dir: &std::path::Path, overrides: &[Edit]) -> Snapshot {
    let config = support::config_with(overrides);
    let (mut poller, _) = support::fixture_poller(&config, dir);
    poller.tick().await.snapshot
}

#[tokio::test]
async fn every_fixture_matches_its_expectation() {
    let dirs = support::fixture_dirs();
    assert!(
        dirs.len() >= 17,
        "expected at least 17 fixtures, found {}",
        dirs.len()
    );

    let mut checked = 0usize;
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let raw = std::fs::read_to_string(dir.join("expected.json")).expect("expected.json");
        let expected: Expected =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{name}/expected.json: {e}"));
        assert!(
            !expected.source.is_empty(),
            "{name}: expected.json must record its source"
        );

        for case in &expected.cases {
            let label = format!("{name}[{}]", case.name);
            let snapshot = snapshot_for(&dir, &case.overrides).await;

            assert!(
                snapshot.errors.is_empty(),
                "{label}: unexpected errors {:?} ({})",
                snapshot.errors,
                expected.note
            );
            assert_eq!(
                snapshot.icon_state.as_str(),
                case.icon_state,
                "{label}: tray icon state"
            );

            for (watch_id, want) in &case.watches {
                let got = snapshot
                    .watches
                    .iter()
                    .find(|w| &w.id == watch_id)
                    .unwrap_or_else(|| panic!("{label}: no watch {watch_id:?}"));

                assert_eq!(
                    got.icon_state.map(|s| s.as_str().to_string()),
                    want.icon_state,
                    "{label}/{watch_id}: watch icon state"
                );
                assert_eq!(
                    got.rows.len(),
                    want.rows.len(),
                    "{label}/{watch_id}: row count; got {:?}",
                    got.rows.iter().map(|r| r.id).collect::<Vec<_>>()
                );

                for (row, want_row) in got.rows.iter().zip(&want.rows) {
                    let at = format!("{label}/{watch_id}/#{}", want_row.id);
                    assert_eq!(row.id, want_row.id, "{at}: pipeline id / order");
                    assert_eq!(row.state.as_str(), want_row.state, "{at}: state");
                    assert_eq!(row.deploy, want_row.deploy, "{at}: deploy outcome");
                    assert_eq!(
                        row.deploy_marker.as_ref().map(|m| m.name.clone()),
                        want_row.deploy_marker,
                        "{at}: deploy marker"
                    );
                    assert_eq!(
                        sorted(&row.failures),
                        sorted(&want_row.failures),
                        "{at}: failures"
                    );
                    assert_eq!(
                        sorted(&row.warnings),
                        sorted(&want_row.warnings),
                        "{at}: warnings"
                    );
                    assert_eq!(sorted(&row.gates), sorted(&want_row.gates), "{at}: gates");
                    assert_eq!(
                        sorted(&row.post_deploy_failures),
                        sorted(&want_row.post_deploy_failures),
                        "{at}: post-deploy failures"
                    );
                    assert_eq!(
                        sorted(&row.sibling_failures),
                        sorted(&want_row.sibling_failures),
                        "{at}: sibling failures"
                    );

                    let got_bridges: BTreeMap<String, String> = row
                        .bridges
                        .iter()
                        .map(|b| (b.name.clone(), b.verdict.clone()))
                        .collect();
                    assert_eq!(got_bridges, want_row.bridges, "{at}: bridge verdicts");
                }
            }
            checked += 1;
        }
    }
    assert!(checked >= 37, "expected at least 37 cases, ran {checked}");
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut out = v.to_vec();
    out.sort();
    out
}

/// A secondary watch contributes rows and never an icon, even when it is the
/// only red thing on the screen. This is the whole reason the role exists: the
/// hourly reconcile schedule is red by design and must not paint the tray.
#[tokio::test]
async fn a_red_secondary_never_reaches_the_icon() {
    let dir = support::fixtures_dir().join("mixed-primary-green-secondary-red");
    let snapshot = snapshot_for(&dir, &[]).await;

    let hourly = snapshot.watches.iter().find(|w| w.id == "hourly").unwrap();
    assert_eq!(hourly.rows.len(), 1);
    assert_eq!(
        hourly.rows[0].state.as_str(),
        "failed",
        "the schedule really did fail"
    );
    assert!(
        hourly.icon_state.is_none(),
        "a secondary watch must not produce an icon state"
    );
    assert_eq!(
        snapshot.icon_state.as_str(),
        "deployed",
        "the tray follows the primary alone"
    );
}

/// The icon follows the highest pipeline id, not the most recently finished
/// pipeline. An older run that is still parked at a gate must not outrank a
/// newer green push.
#[tokio::test]
async fn the_icon_follows_the_highest_pipeline_id() {
    let dir = support::fixtures_dir().join("9da437fd-parked-gate-stacked");
    let snapshot = snapshot_for(&dir, &[]).await;
    let watch = snapshot
        .watches
        .iter()
        .find(|w| w.id == "main-push")
        .unwrap();

    assert_eq!(watch.rows[0].id, 2859138213, "newest first");
    assert_eq!(watch.rows[1].id, 2858536145);
    assert_eq!(watch.rows[1].state.as_str(), "parked_gate");
    assert_eq!(
        watch.icon_state.map(|s| s.as_str().to_string()),
        Some("succeeded_no_deploy".into()),
        "the icon is the newest pipeline's, not the parked one's"
    );
}

/// A `[verdict].script` replaces rules 1 to 7 for every watch.
#[tokio::test]
async fn a_verdict_script_replaces_the_icon_rules() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let mut config = support::config_with(&[]);
    config.verdict.script_source = Some(
        // Deliberately the opposite of what the built-in rules say, so a pass
        // cannot be a coincidence.
        r#"if pipeline.failures.is_empty() { "deployed" } else { "canceled" }"#.into(),
    );
    let (mut poller, _) = support::fixture_poller(&config, &dir);
    let snapshot = poller.tick().await.snapshot;

    assert_eq!(snapshot.icon_state.as_str(), "canceled");
    let row = &snapshot.watches[0].rows[0];
    assert_eq!(row.state.as_str(), "canceled");
    assert_eq!(
        row.deploy, "live",
        "the script changes the state, not the facts"
    );
}

/// A script that returns nonsense yields `unknown` and an error, rather than a
/// confidently wrong colour.
#[tokio::test]
async fn a_broken_verdict_script_degrades_to_unknown() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let mut config = support::config_with(&[]);
    config.verdict.script_source = Some(r#""not_a_state""#.into());
    let (mut poller, _) = support::fixture_poller(&config, &dir);
    let snapshot = poller.tick().await.snapshot;

    assert_eq!(snapshot.icon_state.as_str(), "unknown");
    assert!(
        snapshot.errors.iter().any(|e| e.contains("not_a_state")),
        "the error names what the script returned: {:?}",
        snapshot.errors
    );
}

/// The sandbox stops a runaway script instead of the tray.
#[test]
fn a_runaway_script_is_stopped_by_the_operation_cap() {
    let script = VerdictScript::from_source("let i = 0; while true { i += 1; } \"deployed\"")
        .expect("compiles");
    let view = serde_json::from_str(
        r#"{"id":1,"iid":null,"sha":"abc","sha7":"abc","ref":"main","source":"push",
            "status":"success","web_url":null,"state":"deployed","deploy":"live",
            "deploy_marker":null,"deploy_failures":[],"failures":[],"warnings":[],
            "gates":[],"post_deploy_failures":[],"sibling_failures":[],"bridges":[],
            "parent_jobs":[],"updated_at":null,"created_at":null,"live":false}"#,
    )
    .expect("view decodes");

    let err = script.evaluate(&view).expect_err("the cap fires");
    assert!(
        err.to_string().to_lowercase().contains("operation"),
        "expected an operations-limit error, got: {err}"
    );
}

/// `eval` is not reachable from a verdict script.
#[test]
fn a_verdict_script_cannot_call_eval() {
    let result = VerdictScript::from_source(r#"eval("1+1"); "deployed""#);
    assert!(
        result.is_err(),
        "eval must not compile inside a verdict script"
    );
}

// ---------------------------------------------------------------------------
// The publication guard
// ---------------------------------------------------------------------------

/// ⛔ **Fixtures are committed to a public repository and are recorded from a
/// private one.** A raw GitLab payload carries the committer's real name and
/// email address, the full commit message and title, the pushing user's profile
/// (job title, location, employer, social handles) and the runner that executed
/// the job — including a self-hosted project runner's IP address, system id,
/// tags and free-text description. None of it is read by the verdict engine.
///
/// This asserts the allow-list in `client::fixture` against every committed
/// fixture, so:
///
/// - a recording made before the allow-list existed fails until it is scrubbed;
/// - a field GitLab adds next month is *dropped* by default rather than
///   published, and if it is genuinely wanted the failure names it.
#[test]
fn fixtures_contain_no_personal_data() {
    use bridgewatch_core::client::fixture::{JOB_KEYS, PIPELINE_KEYS, RecordKind, kind_for_file};

    // Deliberately broad: it fires on anything that looks like an address, not
    // on a list of keys somebody remembered to write down.
    let email = regex::Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}").unwrap();

    // Present in a real payload, and each one a separate disclosure. The
    // allow-list already removes them; naming them makes the failure legible.
    const NEVER: &[&str] = &[
        "commit",
        "user",
        "runner",
        "runner_manager",
        "avatar_url",
        "author_email",
        "committer_email",
        "public_email",
        "email",
        "author_name",
        "committer_name",
        "tag_list",
        "artifacts",
        "artifacts_file",
        "project",
    ];

    /// The only address a fixture may contain. `.invalid` is reserved by
    /// RFC 2606, so it can never be routed anywhere.
    const ALLOWED_EMAIL: &str = "redacted@example.invalid";

    struct Sweep<'a> {
        email: &'a regex::Regex,
        never: &'a [&'a str],
        problems: Vec<String>,
        objects: usize,
    }

    impl Sweep<'_> {
        fn walk(
            &mut self,
            value: &serde_json::Value,
            pointer: &str,
            kind: Option<RecordKind>,
            file: &str,
        ) {
            match value {
                serde_json::Value::String(text) => {
                    for found in self.email.find_iter(text) {
                        if found.as_str() != ALLOWED_EMAIL {
                            self.problems.push(format!(
                                "{file}{pointer}: contains an email address {:?}",
                                found.as_str()
                            ));
                        }
                    }
                }
                serde_json::Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        self.walk(item, &format!("{pointer}/{i}"), kind, file);
                    }
                }
                serde_json::Value::Object(map) => {
                    self.objects += 1;
                    for (key, child) in map {
                        let here = format!("{pointer}/{key}");
                        if let Some(kind) = kind
                            && !self.check_key(key, &here, kind, file)
                        {
                            continue;
                        }
                        // The two nested objects that survive are both pipelines.
                        let child_kind = match (kind, key.as_str()) {
                            (Some(_), "pipeline" | "downstream_pipeline") => {
                                Some(RecordKind::Pipeline)
                            }
                            (k, _) => k,
                        };
                        self.walk(child, &here, child_kind, file);
                    }
                }
                _ => {}
            }
        }

        /// `true` when the key is allowed and its value should be walked.
        fn check_key(&mut self, key: &str, at: &str, kind: RecordKind, file: &str) -> bool {
            if self.never.contains(&key) {
                self.problems.push(format!(
                    "{file}{at}: {key:?} carries personal data or private-repo content and must \
                     not be recorded. Run: bridgewatch fixture scrub <dir>"
                ));
                return false;
            }
            let (allowed, name) = match kind {
                RecordKind::Pipeline => (PIPELINE_KEYS, "PIPELINE_KEYS"),
                RecordKind::Job => (JOB_KEYS, "JOB_KEYS"),
            };
            if !allowed.contains(&key) {
                self.problems.push(format!(
                    "{file}{at}: {key:?} is not in the {kind:?} allow-list. If the engine now \
                     needs it, add it to {name} in client::fixture; otherwise run: \
                     bridgewatch fixture scrub <dir>"
                ));
                return false;
            }
            true
        }
    }

    let mut sweep = Sweep {
        email: &email,
        never: NEVER,
        problems: Vec::new(),
        objects: 0,
    };
    let mut files = 0usize;

    for dir in std::fs::read_dir(support::fixtures_dir()).expect("fixtures directory") {
        let dir = dir.expect("readable").path();
        if !dir.is_dir() {
            continue;
        }
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("readable")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        entries.sort();

        for path in entries {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let label = format!("{}/{name}", dir.file_name().unwrap().to_string_lossy());
            let raw = std::fs::read_to_string(&path).expect("readable");
            let value: serde_json::Value =
                serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{label}: {e}"));
            files += 1;
            // `fixture.json` and `expected.json` are bridgewatch's own, not
            // recorded responses, so the allow-list does not apply to them —
            // but the email sweep does, everywhere.
            sweep.walk(&value, "", kind_for_file(&name), &label);
        }
    }

    assert!(
        files >= 90,
        "expected to sweep at least 90 fixture files, swept {files}"
    );
    assert!(
        sweep.objects >= 500,
        "expected to inspect many objects, inspected {}",
        sweep.objects
    );
    assert!(
        sweep.problems.is_empty(),
        "{} fixture problem(s) across {files} files:\n  {}",
        sweep.problems.len(),
        sweep.problems.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// What a failed request does to a verdict
// ---------------------------------------------------------------------------

/// A pipeline whose `/jobs` did not arrive is **unknown**, not green.
///
/// ⛔ This is the one that turns a red pipeline into the outline check. An empty
/// `jobs` list is indistinguishable from a pipeline with nothing wrong in it, so
/// a 429 or a 15 s timeout on the second request of a tick used to produce
/// `succeeded_no_deploy`, exit 0, and a `finished` notification saying so — with
/// the error visible only in the watch's `error` string. The same fixture,
/// answered in full, is `deployed_with_failure`.
#[tokio::test]
async fn a_pipeline_whose_detail_did_not_arrive_is_unknown() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[]);

    let whole = snapshot_for(&dir, &[]).await;
    assert_eq!(
        whole.icon_state.as_str(),
        "deployed_with_failure",
        "the fixture answered in full"
    );

    let transport = std::sync::Arc::new(support::ScriptedTransport::load(&dir));
    transport.fail_always(
        "/jobs",
        bridgewatch_core::client::ClientError::RateLimited {
            retry_after: Some(30),
            reset: None,
        },
    );
    let mut poller = support::poller_with_transport(&config, transport);
    let snapshot = poller.tick().await.snapshot;

    assert_eq!(
        snapshot.icon_state.as_str(),
        "unknown",
        "nothing was read, so nothing is known"
    );
    assert!(
        !snapshot.errors.is_empty(),
        "and the reason is reported, not swallowed: {:?}",
        snapshot.errors
    );
    let watch = snapshot
        .watches
        .iter()
        .find(|w| w.id == "main-push")
        .expect("the primary watch");
    assert!(watch.error.is_some(), "the watch carries the error");
    let row = &watch.rows[0];
    assert_eq!(row.state.as_str(), "unknown");
    assert!(
        !row.live,
        "the list row said `success`, and that much did arrive"
    );
}

/// The same failure on a pipeline the list says is still running keeps `live`.
///
/// `live` drives the poll interval and the popover's spinner, and it comes from
/// the list row, which is the one request that did arrive. Dropping it as well
/// would slow the poller down precisely when it should be watching.
#[tokio::test]
async fn an_unknown_row_keeps_the_live_flag_the_list_gave_it() {
    let dir = support::fixtures_dir().join("synth-marker-failed-live-sibling");
    let config = support::config_with(&[]);
    let transport = std::sync::Arc::new(support::ScriptedTransport::load(&dir));
    transport.fail_always(
        "/jobs",
        bridgewatch_core::client::ClientError::Transport("connection reset".into()),
    );
    let mut poller = support::poller_with_transport(&config, transport);
    let snapshot = poller.tick().await.snapshot;

    let row = &snapshot
        .watches
        .iter()
        .find(|w| w.id == "main-push")
        .expect("the primary watch")
        .rows[0];
    assert_eq!(row.state.as_str(), "unknown");
    assert!(row.live, "the list row said `running`");
    assert!(snapshot.any_live(), "so the fast interval still applies");
}

// ---------------------------------------------------------------------------
// A bridge is a job
// ---------------------------------------------------------------------------

/// `allow_failure` on a trigger job is honoured, exactly as it is on any other
/// job.
///
/// `Bridge.allow_failure` had no reader anywhere in the crate until 2026-09-17:
/// a `trigger:*` job GitLab was told to tolerate still red-ed the tray.
#[test]
fn a_tolerated_trigger_job_is_a_warning_not_a_failure() {
    use bridgewatch_core::config::WatchRules;
    use bridgewatch_core::model::{Bridge, DownstreamPipeline, Job};
    use bridgewatch_core::status::{JobClass, Status};
    use bridgewatch_core::verdict::bridge;

    let loaded = bridgewatch_core::config::parse_str(
        r#"
        [accounts.gl]
        token = { env = "T" }
        [[watches]]
        id = "x"
        account = "gl"
        project = 1
        "#,
        std::path::Path::new("t.toml"),
    )
    .unwrap();
    let rules = WatchRules::compile(&loaded.config.watches[0]).unwrap();

    let child = vec![Job {
        id: 2,
        name: "verify:android".into(),
        status: Status::Failed,
        stage: None,
        allow_failure: false,
        started_at: None,
        finished_at: None,
        duration: None,
        web_url: None,
    }];
    let mut trigger = Bridge {
        id: 1,
        name: "trigger:android".into(),
        status: Status::Failed,
        stage: None,
        allow_failure: false,
        started_at: None,
        web_url: None,
        downstream_pipeline: Some(DownstreamPipeline {
            id: 9,
            project_id: Some(1),
            sha: None,
            ref_name: None,
            status: Status::Failed,
            web_url: None,
        }),
    };

    assert_eq!(
        bridge::own_class(&trigger, &rules),
        JobClass::BlockingFailure
    );
    assert_eq!(
        bridge::verdict(&trigger, Some(&child), &[], &rules).word(),
        "failed"
    );

    trigger.allow_failure = true;
    assert_eq!(
        bridge::own_class(&trigger, &rules),
        JobClass::WarningFailure
    );
    let tolerated = bridge::verdict(&trigger, Some(&child), &[], &rules);
    assert_eq!(tolerated.word(), "passed_with_warnings");
    assert_eq!(
        tolerated.jobs(),
        ["verify:android"],
        "the job that failed still has to be named, or the warning says nothing"
    );

    // A tolerated trigger job whose child was never created has no job to name,
    // so it names itself.
    trigger.downstream_pipeline = None;
    let dead = bridge::verdict(&trigger, None, &[], &rules);
    assert_eq!(dead.word(), "passed_with_warnings");
    assert_eq!(dead.jobs(), ["trigger:android"]);
}

// ---------------------------------------------------------------------------
// Diving past the first level
// ---------------------------------------------------------------------------

/// `dive.depth = 2` reaches a grandchild, and the tree it builds says where the
/// marker was found.
///
/// The fixture's deploy marker is in the grandchild, so depth 1 is honestly
/// `absent` and depth 2 is `deployed`; `expected.json` pins both. This asserts
/// the shape the popover draws from, which the table test does not reach: a
/// bridge's own `bridges`, and the marker's `pipeline_id`.
#[tokio::test]
async fn a_depth_two_dive_carries_the_child_s_own_bridges() {
    use bridgewatch_core::config::edit::{Edit, EditValue};

    let dir = support::fixtures_dir().join("synth-dive-depth-two");
    let snapshot = snapshot_for(
        &dir,
        &[Edit::Set {
            path: "watches.0.dive.depth".into(),
            value: EditValue::Integer(2),
        }],
    )
    .await;

    let row = &snapshot
        .watches
        .iter()
        .find(|w| w.id == "main-push")
        .expect("the primary watch")
        .rows[0];

    let website = &row.bridges[0];
    assert_eq!(website.name, "trigger:website");
    assert!(website.dived, "its jobs were fetched");
    assert_eq!(
        website.bridges.len(),
        1,
        "and so were its own trigger jobs, which is what depth 2 buys"
    );
    let deploy = &website.bridges[0];
    assert_eq!(deploy.name, "trigger:deploy");
    assert_eq!(deploy.verdict, "passed");
    assert_eq!(deploy.child_id, Some(2900000003));
    assert!(deploy.dived);
    assert_eq!(
        deploy.child_project_id,
        Some(91000001),
        "a multi-project trigger's child lives somewhere else, and the view says where"
    );

    let marker = row.deploy_marker.as_ref().expect("the grandchild deployed");
    assert_eq!(marker.name, "deploy:origins");
    assert_eq!(
        marker.pipeline_id, 2900000003,
        "found in the grandchild, not in the parent"
    );
}

/// A verdict script cannot read a file, even one that exists and is valid Rhai.
///
/// ⛔ Rhai's default engine has no `open` or `read` function, which is what made
/// "no filesystem" look true — but its default module resolver is
/// `FileModuleResolver`, so `import "/path/to/x"` loads that file off disk and
/// RUNS it, inside a process holding a GitLab token, from a path the config file
/// names. The resolver is replaced with the dummy one. This plants a real module
/// on disk, so it fails if the resolver is ever restored.
#[test]
fn a_verdict_script_cannot_import_a_file() {
    let module = std::env::temp_dir().join(format!("bw-import-{}.rhai", std::process::id()));
    std::fs::write(&module, "fn state() { \"deployed\" }").expect("writable");
    let stem = module.with_extension("");

    let source = format!(r#"import "{}" as m; m::state()"#, stem.display());
    let outcome = VerdictScript::from_source(&source).and_then(|script| {
        let view = serde_json::from_str(
            r#"{"id":1,"iid":null,"sha":"abc","sha7":"abc","ref":"main","source":"push",
                "status":"success","web_url":null,"state":"deployed","deploy":"absent",
                "deploy_marker":null,"deploy_failures":[],"failures":[],"warnings":[],
                "gates":[],"post_deploy_failures":[],"sibling_failures":[],"bridges":[],
                "parent_jobs":[],"updated_at":null,"created_at":null,"live":false}"#,
        )
        .expect("view decodes");
        script.evaluate(&view)
    });
    let _ = std::fs::remove_file(&module);

    let err = outcome.expect_err("the import must not resolve");
    let text = err.to_string().to_lowercase();
    assert!(
        text.contains("module") || text.contains("import"),
        "expected a module-resolution refusal, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Two answers to "which marker deployed?"
// ---------------------------------------------------------------------------

fn rules_for(toml: &str) -> bridgewatch_core::config::WatchRules {
    let loaded = bridgewatch_core::config::parse_str(
        &format!(
            r#"
            [accounts.gl]
            token = {{ env = "T" }}
            [[watches]]
            id = "x"
            account = "gl"
            project = 1
            {toml}
            "#
        ),
        std::path::Path::new("t.toml"),
    )
    .expect("parses");
    bridgewatch_core::config::WatchRules::compile(&loaded.config.watches[0]).unwrap()
}

fn job(
    id: u64,
    name: &str,
    status: &str,
    started_at: Option<&str>,
) -> bridgewatch_core::model::Job {
    bridgewatch_core::model::Job {
        id,
        name: name.into(),
        status: bridgewatch_core::status::Status::from(status.to_string()),
        stage: None,
        allow_failure: false,
        started_at: started_at.map(str::to_string),
        finished_at: None,
        duration: None,
        web_url: None,
    }
}

/// The same marker name in two children resolves to the first one that ran, and
/// the failures around it are read in **that** child.
///
/// A fleet deploy really does look like this: one marker job per region, same
/// name, different child pipeline. Which one is "the" deploy decides where
/// `post_deploy_failures` and the marker-scope failures are looked for, so
/// picking arbitrarily would make the two lists describe different pipelines on
/// different ticks.
#[test]
fn a_marker_in_two_children_resolves_to_the_one_that_ran_first() {
    use bridgewatch_core::verdict::deploy::{self, JobScope};

    let rules = rules_for(r#"deploy_markers = ["deploy:origins"]"#);
    let eu = vec![
        job(
            20,
            "deploy:origins",
            "success",
            Some("2026-09-17T07:00:00Z"),
        ),
        job(21, "verify:eu", "failed", Some("2026-09-17T06:00:00Z")),
    ];
    let apac = vec![
        job(
            30,
            "deploy:origins",
            "success",
            Some("2026-09-17T08:00:00Z"),
        ),
        job(31, "verify:apac", "failed", Some("2026-09-17T09:00:00Z")),
    ];
    let scopes = [
        JobScope {
            pipeline_id: 2,
            pipeline_live: false,
            jobs: &eu,
        },
        JobScope {
            pipeline_id: 3,
            pipeline_live: false,
            jobs: &apac,
        },
    ];

    let outcome = deploy::outcome(&scopes, &rules, false, None);
    let marker = outcome.marker().expect("something deployed");
    assert_eq!(marker.id, 20, "07:00 ran before 08:00");
    assert_eq!(marker.pipeline_id, 2);

    assert_eq!(
        deploy::marker_scope_failures(&scopes, &rules, marker),
        ["verify:eu"],
        "the marker's OWN pipeline, and only it"
    );
    assert_eq!(
        deploy::post_deploy_failures(&scopes, &rules, marker),
        ["verify:apac"],
        "and anything anywhere that started after it"
    );
}

/// GitLab's job list excludes superseded attempts by default, and nothing here
/// asks for them.
///
/// ⚠️ The model has no `retried` field, so if a payload ever did carry both
/// attempts of a retried job, the failed one would count as a failure and a
/// pipeline somebody fixed by pressing "retry" would stay red. That is not a
/// defect today — it is a dependency on a default — and this is the test that
/// says so out loud and fails if the assumption is ever made explicit in the
/// wrong direction.
#[tokio::test]
async fn nothing_asks_gitlab_for_retried_job_attempts() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let config = support::config_with(&[]);
    let (mut poller, transport) = support::fixture_poller(&config, &dir);
    poller.tick().await;

    let asked: Vec<String> = transport
        .seen()
        .into_iter()
        .filter(|p| p.contains("/jobs?"))
        .collect();
    assert!(!asked.is_empty());
    assert!(
        asked.iter().all(|p| !p.contains("include_retried")),
        "asking for them would change what every verdict means: {asked:#?}"
    );

    // What it would mean, spelled out: two attempts of one job, and the engine
    // has no way to tell which is current.
    let rules = rules_for("");
    let both = [
        job(
            41,
            "verify:web_types",
            "failed",
            Some("2026-09-17T06:00:00Z"),
        ),
        job(
            42,
            "verify:web_types",
            "success",
            Some("2026-09-17T06:30:00Z"),
        ),
    ];
    let classes: Vec<&str> = both
        .iter()
        .map(|j| bridgewatch_core::verdict::job::classify(j, &rules).as_str())
        .collect();
    assert_eq!(
        classes,
        ["blocking_failure", "passed"],
        "the superseded attempt still reads as a failure, which is why the \
         request must not ask for it"
    );
}

/// A job that never ran sorts BEFORE the marker, so it is a marker-scope failure
/// rather than a post-deploy one.
///
/// ⛔ `(None, id) < (Some(_), id)` for every job that ran, which is the second
/// half of the pre-marker defect: a failed job with a null `started_at` — a
/// `.pre` job, anything skipped by an earlier failure and then reported as
/// failed — fell off the post-deploy list by construction and had no policy at
/// all, so it read as a clean deploy.
#[test]
fn a_failure_that_never_started_is_in_the_marker_s_scope_not_after_it() {
    use bridgewatch_core::verdict::deploy::{self, JobScope};

    let rules = rules_for(r#"deploy_markers = ["deploy:origins"]"#);
    let jobs = vec![
        job(
            10,
            "deploy:origins",
            "success",
            Some("2026-09-17T07:00:00Z"),
        ),
        job(11, "build:mirror", "failed", None),
    ];
    let scopes = [JobScope {
        pipeline_id: 2,
        pipeline_live: false,
        jobs: &jobs,
    }];

    let outcome = deploy::outcome(&scopes, &rules, false, None);
    let marker = outcome.marker().expect("it deployed");
    assert!(
        deploy::post_deploy_failures(&scopes, &rules, marker).is_empty(),
        "nothing started after it"
    );
    assert_eq!(
        deploy::marker_scope_failures(&scopes, &rules, marker),
        ["build:mirror"],
        "but something in its own pipeline is broken, and it has to be reported \
         somewhere or the deploy reads as clean"
    );
}

/// ⛔ The tray rule and the SELECTION rule are two different questions, and the
/// CLI used to ask the wrong one. `icon_from_watches` folds primary watches
/// only, which is what keeps an hourly schedule that is red by design out of
/// the tray; `icon_from_selection` answers "how are the watches I named", where
/// a selection holding no primary watch has to be answered from all of them or
/// a verdict that was read perfectly well comes back as `unknown`.
#[tokio::test]
async fn a_selection_with_no_primary_watch_is_answered_from_all_of_it() {
    let config = support::config_with(&[]);
    let dir = support::fixtures_dir().join("mixed-primary-green-secondary-red");
    let (mut poller, _) = support::fixture_poller(&config, &dir);
    let watches = poller.tick().await.snapshot.watches;

    let by_id = |id: &str| {
        watches
            .iter()
            .filter(|w| w.id == id)
            .cloned()
            .collect::<Vec<_>>()
    };

    // The tray: a green push wins, because the red schedule is rows only.
    assert_eq!(
        Snapshot::icon_from_watches(&watches).as_str(),
        "deployed",
        "the whole reason the secondary role exists"
    );
    // Naming the schedule makes it the subject, and it is red.
    assert_eq!(
        Snapshot::icon_from_selection(&by_id("hourly")).as_str(),
        "failed"
    );
    // Naming it ALONGSIDE a primary does not let it outvote the primary.
    assert_eq!(Snapshot::icon_from_selection(&watches).as_str(), "deployed");
    // A watch that matched nothing has no verdict, and "could not see" must
    // stay distinguishable from "saw green".
    assert_eq!(
        Snapshot::icon_from_selection(&by_id("preflights")).as_str(),
        "unknown"
    );
    assert_eq!(Snapshot::icon_from_selection(&[]).as_str(), "unknown");
}

// ---------------------------------------------------------------------------
// "Deployed" only once no configured marker can still move
// ---------------------------------------------------------------------------

/// ⛔ A passed marker is not "deployed" while another configured marker is
/// still queued. Recorded on a real push: `deploy:marketing` finished about
/// 30 s in, `deploy:origins` sat `created` behind the website chain, and the
/// tray drew the filled check (and fired `deployed`) twenty minutes before the
/// fleet had the new code, because the first configured pattern with a
/// success won without asking whether any other marker was still coming.
#[test]
fn a_passed_marker_does_not_read_live_while_another_configured_marker_is_still_queued() {
    use bridgewatch_core::verdict::deploy::{self, JobScope};

    let rules = rules_for(r#"deploy_markers = ["deploy:origins", "deploy:marketing"]"#);
    let jobs = vec![
        job(
            10,
            "deploy:marketing",
            "success",
            Some("2026-09-17T07:00:00Z"),
        ),
        job(11, "deploy:origins", "created", None),
    ];
    let scopes = [JobScope {
        pipeline_id: 2,
        pipeline_live: true,
        jobs: &jobs,
    }];

    let outcome = deploy::outcome(&scopes, &rules, false, None);
    assert_eq!(
        outcome,
        deploy::DeployOutcome::InProgress {
            marker: "deploy:origins".into()
        },
        "the marker still to run is the one being waited on"
    );
}

/// ⛔ The hold is about a marker that can still MOVE, and a job left `created`
/// inside a pipeline that has settled cannot. GitLab does leave them: a job
/// whose `needs:` names an optional manual job nobody pressed sits `created`
/// for good. Holding on it would keep a finished pipeline from ever reading
/// "deployed".
#[test]
fn a_marker_left_created_in_a_settled_pipeline_does_not_hold_back_a_passed_one() {
    use bridgewatch_core::verdict::deploy::{self, JobScope};

    let rules = rules_for(r#"deploy_markers = ["deploy:origins", "deploy:marketing"]"#);
    let marketing = vec![job(
        10,
        "deploy:marketing",
        "success",
        Some("2026-09-17T07:00:00Z"),
    )];
    let origins = vec![job(20, "deploy:origins", "created", None)];
    let scopes = |origins_live: bool| {
        [
            JobScope {
                pipeline_id: 2,
                pipeline_live: false,
                jobs: &marketing,
            },
            JobScope {
                pipeline_id: 3,
                pipeline_live: origins_live,
                jobs: &origins,
            },
        ]
    };

    let settled = deploy::outcome(&scopes(false), &rules, false, None);
    assert_eq!(
        settled.marker().map(|m| m.id),
        Some(10),
        "its pipeline finished, so the created job will never run: {settled:?}"
    );

    assert_eq!(
        deploy::outcome(&scopes(true), &rules, false, None),
        deploy::DeployOutcome::InProgress {
            marker: "deploy:origins".into()
        },
        "while its pipeline is live, it is still coming"
    );
}

/// The same hold for a marker whose status this build cannot read, inside a
/// pipeline that is still live: it may yet succeed or fail, so the one that
/// already passed cannot be the answer yet.
#[test]
fn a_passed_marker_does_not_read_live_beside_an_unreadable_marker_in_a_live_pipeline() {
    use bridgewatch_core::verdict::deploy::{self, JobScope};

    let rules = rules_for(r#"deploy_markers = ["deploy:origins", "deploy:marketing"]"#);
    let jobs = vec![
        job(
            10,
            "deploy:marketing",
            "success",
            Some("2026-09-17T07:00:00Z"),
        ),
        job(11, "deploy:origins", "some_future_status", None),
    ];
    let live = [JobScope {
        pipeline_id: 2,
        pipeline_live: true,
        jobs: &jobs,
    }];
    assert_eq!(
        deploy::outcome(&live, &rules, false, None).word(),
        "in_progress"
    );

    // Settled, the unreadable one cannot move any more, and the success stands.
    let settled = [JobScope {
        pipeline_id: 2,
        pipeline_live: false,
        jobs: &jobs,
    }];
    assert_eq!(
        deploy::outcome(&settled, &rules, false, None)
            .marker()
            .map(|m| m.name.as_str()),
        Some("deploy:marketing")
    );
}

/// A live bridge whose child has not been read yet can hold a marker nobody
/// has seen, and the outcome waits for it. The caller names the bridge.
#[test]
fn a_passed_marker_does_not_read_live_while_a_bridge_that_could_carry_one_is_unread() {
    use bridgewatch_core::verdict::deploy::{self, JobScope};

    let rules = rules_for(r#"deploy_markers = ["deploy:origins", "deploy:marketing"]"#);
    let jobs = vec![job(
        10,
        "deploy:marketing",
        "success",
        Some("2026-09-17T07:00:00Z"),
    )];
    let scopes = [JobScope {
        pipeline_id: 2,
        pipeline_live: false,
        jobs: &jobs,
    }];

    assert_eq!(
        deploy::outcome(&scopes, &rules, false, Some("trigger:website")),
        deploy::DeployOutcome::InProgress {
            marker: "trigger:website".into()
        }
    );
    assert!(
        deploy::outcome(&scopes, &rules, false, None).is_live(),
        "and without one, a lone success is the deploy"
    );
}

/// Once nothing can move, WHICH marker is reported keeps the old rule: config
/// order first, then the earliest start. A gate or a skipped marker cannot
/// move without a human, so it does not hold the answer back.
#[test]
fn once_every_marker_is_settled_config_order_still_picks_the_reported_one() {
    use bridgewatch_core::verdict::deploy::{self, JobScope};

    let rules = rules_for(r#"deploy_markers = ["deploy:origins", "deploy:marketing"]"#);
    let jobs = vec![
        job(
            10,
            "deploy:marketing",
            "success",
            Some("2026-09-17T07:00:00Z"),
        ),
        job(
            11,
            "deploy:origins",
            "success",
            Some("2026-09-17T08:00:00Z"),
        ),
        job(12, "deploy:origins", "manual", None),
        job(13, "deploy:marketing", "skipped", None),
    ];
    let scopes = [JobScope {
        pipeline_id: 2,
        pipeline_live: true,
        jobs: &jobs,
    }];
    let outcome = deploy::outcome(&scopes, &rules, false, None);
    assert_eq!(
        outcome.marker().map(|m| m.id),
        Some(11),
        "deploy:origins is listed first, so it wins although it ran later"
    );
}

/// A pipeline detail built from JSON, for driving `evaluate_pipeline` directly
/// with a shape no recording holds.
fn detail_from(value: serde_json::Value) -> bridgewatch_core::model::PipelineDetail {
    serde_json::from_value(value).expect("a pipeline detail")
}

fn bridge_json(id: u64, name: &str, status: &str, child: Option<(u64, &str)>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "status": status,
        "downstream_pipeline": child.map(|(cid, cstatus)| serde_json::json!({
            "id": cid, "status": cstatus
        })),
    })
}

fn job_json(id: u64, name: &str, status: &str, started_at: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id, "name": name, "status": status, "started_at": started_at
    })
}

/// End to end through the icon rules: a marketing child that already shipped
/// and a website trigger that has not created its child yet. The website child
/// is where `deploy:origins` will be, and nothing has read it.
#[test]
fn a_shipped_marker_beside_an_unborn_marker_lane_reads_running_not_deployed() {
    use bridgewatch_core::verdict::{DetailSource, evaluate_pipeline};

    let config = support::config_with(&[]);
    let watch = &config.watches[0];
    let rules = bridgewatch_core::config::WatchRules::compile(watch).unwrap();
    let detail = detail_from(serde_json::json!({
        "pipeline": { "id": 1, "ref": "main", "status": "running", "sha": "abc" },
        "jobs": [],
        "bridges": [
            bridge_json(100, "trigger:marketing", "success", Some((2, "success"))),
            bridge_json(101, "trigger:website", "created", None),
        ],
        "child_jobs": {
            "2": [job_json(20, "deploy:marketing", "success", Some("2026-09-17T07:00:00Z"))]
        }
    }));

    let (view, _) = evaluate_pipeline(&detail, DetailSource::Fetched, watch, &rules, None);
    assert_eq!(view.deploy, "in_progress");
    assert_eq!(view.state.as_str(), "running");
    assert!(view.failures.is_empty(), "{:?}", view.failures);
}

// ---------------------------------------------------------------------------
// A pipeline that failed before it had a job
// ---------------------------------------------------------------------------

/// ⛔ A GitHub run whose workflow file does not parse is `completed` with
/// `conclusion: startup_failure` and **zero jobs**; one that hit the run limit
/// before a job reported can be `timed_out` with none either. Both fold onto
/// `failed`. The rules read jobs and bridges, both lists were empty, and the
/// run fell through every rule to `succeeded_no_deploy`: a broken workflow
/// drew the outline check and `check` exited 0. The GitLab half of this is the
/// `synth-pipeline-failed-no-jobs` fixture.
///
/// ⚠️ **Hand-built**, as `tests/github.rs` is: GitHub has no recorder yet
/// (CONTRIBUTING § Fixtures), so the run carries only the keys the engine reads,
/// with invented values.
#[test]
fn a_github_run_that_failed_before_any_job_is_failed_not_green() {
    use bridgewatch_core::status::Status;
    use bridgewatch_core::verdict::{DetailSource, evaluate_pipeline};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/github.toml");
    let raw = std::fs::read_to_string(&path).expect("examples/github.toml exists");
    let loaded =
        bridgewatch_core::config::parse_str(&raw, &path).expect("the GitHub example loads");
    let watch = &loaded.config.watches[0];
    let rules = bridgewatch_core::config::WatchRules::compile(watch).unwrap();

    for conclusion in ["startup_failure", "timed_out"] {
        let mut detail = detail_from(serde_json::json!({
            "pipeline": { "id": 4001, "ref": "main", "status": "completed", "sha": "0f1e2d3c" },
            "jobs": [],
            "bridges": [],
            "child_jobs": {}
        }));
        detail.pipeline.status = Status::from_github("completed", Some(conclusion));

        let (view, _) = evaluate_pipeline(&detail, DetailSource::Fetched, watch, &rules, None);
        assert_eq!(view.state.as_str(), "failed", "{conclusion}");
        assert_eq!(view.failures, ["pipeline failed"], "{conclusion}");
        assert!(!view.live, "{conclusion}");

        // Unread is not the same thing: with no detail the answer stays
        // `unknown`, because an empty list then means "not looked at".
        let (unread, _) =
            evaluate_pipeline(&detail, DetailSource::Unavailable, watch, &rules, None);
        assert_eq!(unread.state.as_str(), "unknown", "{conclusion}");
    }

    // The gate shape is untouched: `action_required` with no jobs is parked.
    let mut parked = detail_from(serde_json::json!({
        "pipeline": { "id": 4002, "ref": "main", "status": "completed", "sha": "0f1e2d3c" },
        "jobs": [],
        "bridges": [],
            "child_jobs": {}
    }));
    parked.pipeline.status = Status::from_github("completed", Some("action_required"));
    let (view, _) = evaluate_pipeline(&parked, DetailSource::Fetched, watch, &rules, None);
    assert_eq!(view.state.as_str(), "parked_gate");
    assert!(view.failures.is_empty());
}

/// The guard reads the RAW job list, not the classified one. A pipeline whose
/// jobs all exist but are all hidden by `ignore` has been read, and the user
/// said what those jobs are worth; the pipeline's own `failed` must not
/// overrule that.
#[test]
fn a_failed_pipeline_whose_jobs_are_all_ignored_is_not_red_by_its_own_status() {
    use bridgewatch_core::verdict::{DetailSource, evaluate_pipeline};

    let config = support::config_with(&[Edit::Set {
        path: "watches.0.jobs.lint".into(),
        value: bridgewatch_core::config::edit::EditValue::String("ignore".into()),
    }]);
    let watch = &config.watches[0];
    let rules = bridgewatch_core::config::WatchRules::compile(watch).unwrap();
    let detail = detail_from(serde_json::json!({
        "pipeline": { "id": 1, "ref": "main", "status": "failed", "sha": "abc" },
        "jobs": [job_json(10, "lint", "failed", Some("2026-09-17T07:00:00Z"))],
        "bridges": [],
            "child_jobs": {}
    }));

    let (view, _) = evaluate_pipeline(&detail, DetailSource::Fetched, watch, &rules, None);
    assert!(view.failures.is_empty(), "{:?}", view.failures);
    assert_ne!(view.state.as_str(), "failed");
}

// ---------------------------------------------------------------------------
// A trigger job that has not created its child yet is not dead
// ---------------------------------------------------------------------------

/// ⛔ GitLab creates the trigger job first and its child a few seconds later.
/// In that window `downstream_pipeline` is null, and reading "no child" as
/// `dead` whatever the trigger's own status said turned every push red for
/// its first poll and fired `blocking_failure` for a pipeline that was fine.
#[test]
fn a_trigger_job_that_has_not_created_its_child_yet_reads_by_its_own_status() {
    use bridgewatch_core::verdict::bridge;

    let rules = rules_for("");
    let cases = [
        ("created", "running"),
        ("pending", "running"),
        ("running", "running"),
        ("waiting_for_resource", "running"),
        ("manual", "awaiting_gate"),
        ("skipped", "skipped"),
        ("canceled", "canceled"),
        // Settled with no child is the parse failure the verdict exists for,
        // and its own status is frequently `success`.
        ("success", "dead"),
        ("failed", "dead"),
        ("some_future_status", "dead"),
    ];
    for (status, want) in cases {
        let b: bridgewatch_core::model::Bridge =
            serde_json::from_value(bridge_json(1, "trigger:website", status, None)).unwrap();
        let got = bridge::verdict(&b, None, &[], &rules);
        assert_eq!(got.word(), want, "trigger status {status}");
    }
}

// ---------------------------------------------------------------------------
// A skipped pipeline carries no news
// ---------------------------------------------------------------------------

/// ⛔ A push that touches only files no job's rules match makes GitLab create
/// a `skipped` pipeline. It is settled, so it took the one `show.settled` slot,
/// the real pipeline before it was dropped, and the icon (newest id) went grey
/// and fired `finished: canceled` for a push that changed nothing.
#[tokio::test]
async fn a_skipped_pipeline_does_not_take_the_icon_from_the_real_one_before_it() {
    let dir = support::fixtures_dir().join("synth-skipped-newest");
    let snapshot = snapshot_for(&dir, &[]).await;
    let watch = snapshot
        .watches
        .iter()
        .find(|w| w.id == "main-push")
        .unwrap();

    let ids: Vec<u64> = watch.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, [2859138213], "the skipped row is not listed");
    assert_eq!(
        watch.icon_state.map(|s| s.as_str().to_string()),
        Some("succeeded_no_deploy".into()),
        "the icon is the real pipeline's"
    );
    assert_eq!(
        watch.row_state().map(|s| s.as_str()),
        Some("succeeded_no_deploy")
    );
}
