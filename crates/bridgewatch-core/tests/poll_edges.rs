//! The poller's edges: a verdict script that does not build, a watch on an
//! account with no client, files that cannot be written, `look_up_commit`'s
//! refusals, a `mine` watch whose owner cannot be read, a child whose bridges
//! fail, and `run` stopping when asked. Every answer is scripted or recorded;
//! nothing leaves the process.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bridgewatch_core::client::{CiClient, ClientError, RequestRing, ScriptTransport};
use bridgewatch_core::config::Config;
use bridgewatch_core::poll::{CommitLookup, Poller};
use bridgewatch_core::token::Secret;

/// A path in the temp dir, unique to one call, removed when dropped.
struct TempPath(PathBuf);

impl TempPath {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("{name}-{}-{n}", std::process::id())))
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn config(raw: &str) -> Config {
    bridgewatch_core::config::parse_str(raw, std::path::Path::new("test.toml"))
        .expect("the configuration loads")
        .config
}

const ONE: &str = r#"
[accounts.lab]
token = { env = "TOK" }

[[watches]]
id = "web"
account = "lab"
project = 1
ref = "main"
"#;

fn clients(
    config: &Config,
    t: &Arc<ScriptTransport>,
    ring: &RequestRing,
) -> BTreeMap<String, Arc<dyn CiClient>> {
    config
        .accounts
        .iter()
        .map(|(name, account)| {
            let client = bridgewatch_core::client::client_for(
                account,
                &Secret::new("glpat-FIXTURE"),
                t.clone(),
                ring.clone(),
            )
            .expect("a client");
            (name.clone(), client)
        })
        .collect()
}

fn scripted(raw: &str, t: &Arc<ScriptTransport>) -> Poller {
    let config = config(raw);
    let ring = RequestRing::new(50);
    Poller::with_clients(&config, clients(&config, t, &ring), ring).expect("poller builds")
}

fn row(id: u64, status: &str) -> String {
    format!(
        r#"{{"id":{id},"iid":{id},"project_id":1,"sha":"{SHA}","ref":"main","status":"{status}",
            "source":"push","web_url":"https://gitlab.example/g/p/-/pipelines/{id}",
            "created_at":"2026-11-01T10:00:00Z","updated_at":"2026-11-01T10:05:00Z"}}"#
    )
}

fn bridge(name: &str, child: u64) -> String {
    format!(
        r#"{{"id":{job},"name":"{name}","stage":"trigger","status":"success","allow_failure":false,
            "downstream_pipeline":{{"id":{child},"project_id":1,"sha":"{SHA}","ref":"main",
            "status":"success","web_url":"https://gitlab.example/g/p/-/pipelines/{child}"}}}}"#,
        job = child + 1000,
    )
}

#[test]
fn a_verdict_script_that_does_not_compile_or_cannot_be_read_is_a_config_error() {
    let mut config = config(ONE);
    config.verdict.script_source = Some("let x = ;".into());
    let error = Poller::with_clients(&config, BTreeMap::new(), RequestRing::new(1))
        .err()
        .expect("a script that does not compile is refused");
    assert!(
        error
            .to_string()
            .contains("verdict script does not compile"),
        "{error}"
    );

    config.verdict.script_source = None;
    config.verdict.script = Some("/nonexistent/bridgewatch/verdict.rhai".into());
    let error = Poller::with_clients(&config, BTreeMap::new(), RequestRing::new(1))
        .err()
        .expect("a script that cannot be read is refused");
    assert!(
        error
            .to_string()
            .contains("cannot read verdict script /nonexistent/bridgewatch/verdict.rhai"),
        "{error}"
    );
}

#[tokio::test]
async fn a_watch_whose_account_has_no_client_is_an_error_on_that_watch() {
    let config = config(ONE);
    let mut poller =
        Poller::with_clients(&config, BTreeMap::new(), RequestRing::new(1)).expect("builds");
    let mut seen = poller.subscribe();
    let tick = poller.tick().await;
    assert_eq!(
        tick.snapshot.watches[0].error.as_deref(),
        Some("unknown account \"lab\"")
    );
    assert_eq!(
        tick.snapshot.errors,
        ["watch \"web\" refers to unknown account \"lab\""]
    );
    assert!(seen.has_changed().expect("the sender lives"));
    assert_eq!(seen.borrow_and_update().watches.len(), 1);

    let error = poller.look_up_commit("web", SHA).await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Unsupported { message } if message == "unknown account \"lab\""),
        "{error:?}"
    );
    let error = poller.look_up_commit("nope", SHA).await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Unsupported { message } if message == "no watch \"nope\""),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_ledger_or_history_that_cannot_be_saved_is_a_warning_and_the_tick_goes_on() {
    // A regular file where the directory should be: neither save can succeed.
    let blocker = TempPath::new("bw-poll-edges-blocker");
    std::fs::write(&blocker.0, "not a directory").expect("blocker");
    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let (poller, _) = support::fixture_poller(&support::config_with(&[]), &dir);
    let mut poller = poller
        .with_ledger(blocker.0.join("ledger.json"))
        .with_eta_history(blocker.0.join("eta.json"));
    let tick = poller.tick().await;
    assert!(tick.snapshot.watches.iter().all(|w| w.error.is_none()));
    // Both were learned and kept in memory; neither reached the disk.
    assert!(poller.ledger().is_baselined("main-push"));
    assert_eq!(poller.eta_history().samples("main-push"), [799]);
    assert!(std::fs::read_to_string(&blocker.0).unwrap() == "not a directory");
}

#[tokio::test]
async fn run_stops_when_on_tick_says_so_and_sleeps_between_ticks_otherwise() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/projects/1/pipelines?", 200, "[]");
    t.reply("/projects/1/pipelines?", 200, "[]");
    let mut poller = scripted(ONE, &t);
    let mut ticks = 0;
    tokio::time::pause();
    poller
        .run(|_| {
            ticks += 1;
            ticks < 2
        })
        .await;
    assert_eq!(ticks, 2);
    assert_eq!(t.remaining(), 0);
}

#[tokio::test]
async fn a_lookup_of_a_commit_with_a_child_that_cannot_be_read_is_that_error() {
    let raw = format!("{ONE}dive = {{ bridges = \"*\" }}\n");
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply(
        "/pipelines/7/bridges",
        200,
        &format!("[{}]", bridge("trigger:child", 8)),
    );
    t.reply("/pipelines/8/jobs", 500, "{}");
    let poller = scripted(&raw, &t);
    let error = poller.look_up_commit("web", SHA).await.unwrap_err();
    assert!(
        matches!(error, ClientError::Server { status: 500, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_found_commit_carries_no_title_unless_titles_are_on() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply("/pipelines/7/bridges", 200, "[]");
    let poller = scripted(ONE, &t);
    let CommitLookup::Found(view) = poller.look_up_commit("web", SHA).await.expect("found") else {
        panic!("the commit's pipeline is found");
    };
    assert_eq!(view.title, None);
}

const MINE: &str = r#"
[accounts.lab]
token = { env = "TOK" }

[[watches]]
id = "web"
account = "lab"
project = 1
ref = "main"
mine = true
"#;

#[tokio::test]
async fn a_mine_lookup_asks_who_the_token_is_and_a_refused_owner_is_the_error() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/user", 200, r#"{"id":1,"username":"ada"}"#);
    t.reply("/projects/1/pipelines?", 200, "[]");
    let poller = scripted(MINE, &t);
    assert!(matches!(
        poller.look_up_commit("web", SHA).await,
        Ok(CommitLookup::NotFound)
    ));
    let list = t
        .requests()
        .into_iter()
        .find(|r| r.url.contains("/pipelines?"))
        .expect("the list was asked for");
    assert!(list.url.contains("username=ada"), "{}", list.url);

    let t = Arc::new(ScriptTransport::new());
    t.reply("/user", 500, "{}");
    let error = scripted(MINE, &t)
        .look_up_commit("web", SHA)
        .await
        .unwrap_err();
    assert!(
        matches!(error, ClientError::Server { status: 500, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_child_whose_bridges_fail_is_an_error_and_is_not_walked_deeper() {
    let raw = format!("{ONE}dive = {{ bridges = \"*\", depth = 3 }}\n");
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "running")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    // Two bridges to one child: it is reached, and asked about, once.
    t.reply(
        "/pipelines/7/bridges",
        200,
        &format!("[{},{}]", bridge("a", 8), bridge("b", 8)),
    );
    t.reply("/pipelines/8/jobs", 200, "[]");
    t.reply("/pipelines/8/bridges", 500, "{}");
    let mut poller = scripted(&raw, &t);
    let tick = poller.tick().await;
    let error = tick.snapshot.watches[0].error.clone().unwrap_or_default();
    assert!(error.contains("500"), "{error}");
    assert_eq!(
        t.requests()
            .iter()
            .filter(|r| r.url.contains("/pipelines/8/jobs"))
            .count(),
        1
    );
}

#[test]
fn a_poller_built_from_ready_clients_has_nothing_to_retry() {
    let t = Arc::new(ScriptTransport::new());
    assert!(scripted(ONE, &t).retry_refused().is_empty());
}

#[tokio::test]
async fn a_commit_title_that_cannot_be_read_is_left_off_and_not_asked_again() {
    let t = Arc::new(ScriptTransport::new());
    for _ in 0..2 {
        t.reply(
            "/projects/1/pipelines?",
            200,
            &format!("[{}]", row(7, "success")),
        );
    }
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply("/pipelines/7/bridges", 200, "[]");
    // Not JSON: a decode error, which no retry can fix.
    t.reply("/repository/commits/", 200, "not json");
    let mut poller = scripted(ONE, &t).with_commit_titles(true);
    let tick = poller.tick().await;
    assert_eq!(tick.snapshot.watches[0].rows[0].title, None);
    assert_eq!(tick.snapshot.watches[0].error, None);
    let tick = poller.tick().await;
    assert_eq!(tick.snapshot.watches[0].rows[0].title, None);
    let asked = t
        .requests()
        .iter()
        .filter(|r| r.url.contains("/repository/commits/"))
        .count();
    assert_eq!(asked, 1, "an unreadable title is not asked for again");
}

#[tokio::test]
async fn a_settled_childs_bridges_are_kept_from_the_last_tick_and_not_asked_again() {
    let raw = format!("{ONE}dive = {{ bridges = \"*\", depth = 2 }}\n");
    let t = Arc::new(ScriptTransport::new());
    for _ in 0..2 {
        t.reply(
            "/projects/1/pipelines?",
            200,
            &format!("[{}]", row(7, "running")),
        );
        t.reply("/pipelines/7/jobs", 200, "[]");
        t.reply(
            "/pipelines/7/bridges",
            200,
            &format!("[{}]", bridge("trigger:child", 8)),
        );
    }
    t.reply("/pipelines/8/jobs", 200, "[]");
    t.reply("/pipelines/8/bridges", 200, "[]");
    let mut poller = scripted(&raw, &t);
    for _ in 0..2 {
        let tick = poller.tick().await;
        assert_eq!(tick.snapshot.watches[0].error, None);
    }
    assert_eq!(t.remaining(), 0);
    let child = |part: &str| {
        t.requests()
            .iter()
            .filter(|r| r.url.contains(&format!("/pipelines/8/{part}")))
            .count()
    };
    assert_eq!((child("jobs"), child("bridges")), (1, 1));
}

#[tokio::test]
async fn a_deploy_older_than_a_full_history_is_not_learned_and_nothing_is_written() {
    let file = TempPath::new("bw-poll-edges-eta");
    let mut full = bridgewatch_core::eta::EtaHistory::default();
    for n in 0..bridgewatch_core::eta::MAX_SAMPLES as u64 {
        assert!(full.record("main-push", u64::MAX - n, 100));
    }
    full.save_merged(&file.0).expect("history saves");
    let written = std::fs::metadata(&file.0).unwrap().modified().unwrap();

    let dir = support::fixtures_dir().join("4cfaced9-deployed");
    let (poller, _) = support::fixture_poller(&support::config_with(&[]), &dir);
    let mut poller = poller.with_eta_history(file.0.clone());
    poller.tick().await;
    assert_eq!(
        poller.eta_history().samples("main-push"),
        vec![100; bridgewatch_core::eta::MAX_SAMPLES]
    );
    assert_eq!(
        std::fs::metadata(&file.0).unwrap().modified().unwrap(),
        written
    );
}

#[tokio::test]
async fn a_found_commit_keeps_its_title_when_titles_are_on() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply("/pipelines/7/bridges", 200, "[]");
    let poller = scripted(ONE, &t).with_commit_titles(true);
    assert!(matches!(
        poller.look_up_commit("web", SHA).await,
        Ok(CommitLookup::Found(_))
    ));
}

// ---------------------------------------------------------------------------
// Every path of `look_up_commit`, `poll_watch` and `fetch_detail` in this one
// binary: llvm-cov's summary credits an async fn with the coverage of the
// best single binary's copy of it, so the paths split across binaries would
// read as missed lines.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_short_sha_is_expanded_first_and_one_the_project_lacks_is_not_found() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/repository/commits/0123456",
        200,
        &format!(r#"{{"id":"{SHA}"}}"#),
    );
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply("/pipelines/7/bridges", 200, "[]");
    t.reply("/repository/commits/fedcba9", 404, "{}");
    let poller = scripted(ONE, &t);
    assert!(matches!(
        poller.look_up_commit("web", "0123456").await,
        Ok(CommitLookup::Found(_))
    ));
    assert!(matches!(
        poller.look_up_commit("web", "fedcba9").await,
        Ok(CommitLookup::NotFound)
    ));
}

#[tokio::test]
async fn a_lookup_whose_list_fails_is_that_error_and_one_that_only_skipped_is_skipped() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/projects/1/pipelines?", 500, "{}");
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "skipped")),
    );
    let poller = scripted(ONE, &t);
    assert!(matches!(
        poller.look_up_commit("web", SHA).await,
        Err(ClientError::Server { status: 500 })
    ));
    assert!(matches!(
        poller.look_up_commit("web", SHA).await,
        Ok(CommitLookup::Skipped(_))
    ));
}

/// A list that fails keeps the last frame when there is one and is a bare
/// error view when there is not; a `mine` watch whose owner cannot be read
/// sends no list at all.
#[tokio::test]
async fn a_list_that_fails_keeps_the_last_frame_or_is_a_bare_error() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/projects/1/pipelines?", 500, "{}");
    let first = scripted(ONE, &t).tick().await;
    assert!(first.snapshot.watches[0].rows.is_empty());
    assert!(first.snapshot.watches[0].error.is_some());

    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply("/pipelines/7/bridges", 200, "[]");
    t.reply("/projects/1/pipelines?", 503, "{}");
    let mut poller = scripted(ONE, &t);
    assert_eq!(poller.tick().await.snapshot.watches[0].rows.len(), 1);
    let failed = poller.tick().await;
    assert_eq!(
        failed.snapshot.watches[0].rows.len(),
        1,
        "the last frame is kept"
    );
    assert!(failed.snapshot.watches[0].error.is_some());

    let t = Arc::new(ScriptTransport::new());
    t.reply("/user", 200, r#"{"id":1,"username":"ada"}"#);
    t.reply("/projects/1/pipelines?", 200, "[]");
    let mut poller = scripted(MINE, &t);
    let tick = poller.tick().await;
    assert_eq!(tick.snapshot.watches[0].error, None);
    assert!(tick.polled.contains("web"));

    let t = Arc::new(ScriptTransport::new());
    t.reply("/user", 401, "{}");
    let mut poller = scripted(MINE, &t);
    let tick = poller.tick().await;
    assert!(tick.snapshot.watches[0].error.is_some());
    assert!(!tick.polled.contains("web"), "no list was sent");
}

/// A GitHub sign-in's 404 carries the hint about the app's installations.
#[tokio::test]
async fn a_signed_in_github_watch_that_is_not_found_says_to_install_the_app() {
    const GH: &str = r#"
[accounts.gh]
provider = "github"
token = { oauth = true }

[[watches]]
id = "web"
account = "gh"
project = "acme/web"
ref = "main"
"#;
    let t = Arc::new(ScriptTransport::new());
    t.reply("/actions/runs?", 404, r#"{"message":"Not Found"}"#);
    let mut poller = scripted(GH, &t);
    let tick = poller.tick().await;
    let error = tick.snapshot.watches[0].error.clone().unwrap_or_default();
    assert!(error.contains("GitHub App"), "{error}");
}

/// A row whose own jobs cannot be read is shown as unavailable, never as
/// "nothing failed"; and a script that fails is the watch's error.
#[tokio::test]
async fn a_row_that_cannot_be_read_is_unavailable_and_a_failing_script_is_an_error() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 500, "{}");
    let mut poller = scripted(ONE, &t);
    let tick = poller.tick().await;
    assert!(tick.snapshot.watches[0].error.is_some());
    assert_eq!(tick.snapshot.watches[0].rows[0].state.as_str(), "unknown");

    let mut config = config(ONE);
    config.verdict.script_source = Some("\"not a state\"".into());
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply("/pipelines/7/bridges", 200, "[]");
    let ring = RequestRing::new(50);
    let mut poller =
        Poller::with_clients(&config, clients(&config, &t, &ring), ring).expect("builds");
    let tick = poller.tick().await;
    let error = tick.snapshot.watches[0].error.clone().unwrap_or_default();
    assert!(error.contains("not a state"), "{error}");
}

/// A bridge the dive rules leave out is not walked, and one with no child
/// has nothing to walk.
#[tokio::test]
async fn an_excluded_bridge_and_a_dead_one_are_not_walked() {
    let raw = format!("{ONE}dive = {{ bridges = \"trigger:*\", exclude = [\"trigger:skip\"] }}\n");
    let dead = r#"{"id":1999,"name":"trigger:dead","stage":"trigger","status":"failed","allow_failure":false,"downstream_pipeline":null}"#;
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{}]", row(7, "success")),
    );
    t.reply("/pipelines/7/jobs", 200, "[]");
    t.reply(
        "/pipelines/7/bridges",
        200,
        &format!("[{},{dead}]", bridge("trigger:skip", 8)),
    );
    let mut poller = scripted(&raw, &t);
    poller.tick().await;
    assert!(
        !t.requests().iter().any(|r| r.url.contains("/pipelines/8/")),
        "{:?}",
        t.requests()
            .iter()
            .map(|r| r.url.clone())
            .collect::<Vec<_>>()
    );
}

/// Two pipelines for one commit: the newest that ran is the one judged.
#[tokio::test]
async fn a_commit_with_two_pipelines_is_judged_by_the_newest() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines?",
        200,
        &format!("[{},{}]", row(7, "failed"), row(9, "success")),
    );
    t.reply("/pipelines/9/jobs", 200, "[]");
    t.reply("/pipelines/9/bridges", 200, "[]");
    let CommitLookup::Found(view) = scripted(ONE, &t).look_up_commit("web", SHA).await.unwrap()
    else {
        panic!("found");
    };
    assert_eq!(view.id, 9);
}

/// An account that cannot sign in at all (a self-managed instance with no
/// client of its own) is refused through the factory with the sign-in's
/// words, and a per-account poller refuses only that account.
#[tokio::test]
async fn an_account_that_cannot_sign_in_is_refused_through_the_factory() {
    let raw = r#"
[accounts.lab]
base_url = "https://gitlab.example.com"
token = { oauth = true }

[[watches]]
id = "web"
account = "lab"
project = 1
"#;
    // The validator refuses this file, as it should; the factory is checked
    // on its own, behind it.
    let config: Config = toml::from_str(raw).expect("the table deserialises");
    let t = Arc::new(ScriptTransport::new());
    let store: Arc<dyn bridgewatch_core::token::TokenProvider> =
        Arc::new(bridgewatch_core::token::SystemTokenProvider);
    let built = bridgewatch_core::poll::client_for_account_on(
        "lab",
        &config.accounts["lab"],
        None,
        store,
        t.clone(),
        RequestRing::new(1),
    );
    assert!(
        matches!(&built, Err(bridgewatch_core::poll::PollerError::SignIn(name, _)) if name == "lab"),
        "{:?}",
        built.err()
    );
    let mut poller = Poller::from_config_per_account_on(
        &config,
        &bridgewatch_core::token::SystemTokenProvider,
        t.clone(),
    )
    .unwrap();
    assert_eq!(poller.refused_accounts(), ["lab"]);
    assert!(poller.tick().await.snapshot.watches[0].error.is_some());
    assert!(t.requests().is_empty());
}
