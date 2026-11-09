//! Commit titles on pipeline rows (`Poller::with_commit_titles`), on GitLab,
//! where each title is a request of its own, against a scripted transport with
//! hand-written answers in the documented shapes:
//!
//! - `GET /projects/:id/pipelines`: the list rows (`id`, `sha`, `ref`,
//!   `status`, `source`, `web_url`, `created_at`, `updated_at`);
//! - `GET /projects/:id/repository/commits/:sha`: `{"id": "<sha>", "title":
//!   "<first line>", ...}`, or a 404 for a commit the project does not have.
//!
//! The GitHub half, where the run carries `display_title`, is in `github.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use bridgewatch_core::client::{CiClient, RequestRing, ScriptTransport};
use bridgewatch_core::config::Config;
use bridgewatch_core::poll::{Poller, TITLES_PER_TICK};
use bridgewatch_core::token::Secret;

/// Five settled pipelines, newest first, every one shown.
const CONFIG: &str = r#"
[accounts.lab]
token = { env = "TOK" }

[[watches]]
id = "web"
account = "lab"
project = 1
ref = "main"
show = { max_rows = 5, settled = 5 }
"#;

fn sha(n: u64) -> String {
    format!("{n:x}").repeat(40)[..40].to_string()
}

fn list() -> String {
    let rows: Vec<String> = (1..=5)
        .rev()
        .map(|n| {
            format!(
                r#"{{"id":{id},"iid":{n},"project_id":1,"sha":"{sha}","ref":"main","status":"success",
                    "source":"push","web_url":"https://gitlab.example/g/p/-/pipelines/{id}",
                    "created_at":"2026-11-0{n}T10:00:00Z","updated_at":"2026-11-0{n}T10:05:00Z"}}"#,
                id = 100 + n,
                sha = sha(n),
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

fn config() -> Config {
    bridgewatch_core::config::parse_str(CONFIG, std::path::Path::new("test.toml"))
        .expect("the configuration loads")
        .config
}

/// A poller on the script, with every tick's list and each pipeline's detail
/// answered `ticks` times.
fn poller(t: &Arc<ScriptTransport>, ticks: usize, titles: bool) -> Poller {
    for _ in 0..ticks {
        t.reply("/projects/1/pipelines?", 200, &list());
    }
    for n in 1..=5 {
        t.reply(&format!("/pipelines/{}/jobs", 100 + n), 200, "[]");
        t.reply(&format!("/pipelines/{}/bridges", 100 + n), 200, "[]");
    }
    let config = config();
    let ring = RequestRing::new(50);
    let mut clients: BTreeMap<String, Arc<dyn CiClient>> = BTreeMap::new();
    for (name, account) in &config.accounts {
        clients.insert(
            name.clone(),
            bridgewatch_core::client::client_for(
                account,
                &Secret::new("glpat-FIXTURE"),
                t.clone(),
                ring.clone(),
            )
            .expect("a GitLab client"),
        );
    }
    Poller::with_clients(&config, clients, ring)
        .expect("the poller builds")
        .with_commit_titles(titles)
}

fn title_requests(t: &ScriptTransport) -> Vec<String> {
    t.requests()
        .into_iter()
        .map(|r| r.path)
        .filter(|p| p.contains("/repository/commits/"))
        .collect()
}

fn commit_reply(t: &ScriptTransport, n: u64, title: &str) {
    t.reply(
        &format!("/repository/commits/{}", sha(n)),
        200,
        &format!(
            r#"{{"id":"{}","title":{},"message":"ignored"}}"#,
            sha(n),
            serde_json::json!(title)
        ),
    );
}

#[tokio::test]
async fn titles_are_asked_newest_first_a_few_per_tick_and_once_per_commit() {
    let t = Arc::new(ScriptTransport::new());
    for n in 1..=5 {
        commit_reply(&t, n, &format!("Commit number {n}"));
    }
    let mut p = poller(&t, 3, true);

    let first = p.tick().await.snapshot;
    assert_eq!(
        title_requests(&t).len(),
        TITLES_PER_TICK,
        "{:?}",
        title_requests(&t)
    );
    let titles = |s: &bridgewatch_core::verdict::Snapshot| -> Vec<Option<String>> {
        s.watches[0].rows.iter().map(|r| r.title.clone()).collect()
    };
    assert_eq!(
        titles(&first),
        [
            Some("Commit number 5".to_string()),
            Some("Commit number 4".to_string()),
            Some("Commit number 3".to_string()),
            None,
            None
        ],
        "the newest rows first"
    );
    assert!(first.watches[0].error.is_none());

    let second = p.tick().await.snapshot;
    assert_eq!(title_requests(&t).len(), 5);
    assert!(
        titles(&second).iter().all(Option::is_some),
        "{:?}",
        titles(&second)
    );

    p.tick().await;
    assert_eq!(
        title_requests(&t).len(),
        5,
        "a commit's title is asked once"
    );
    assert_eq!(t.remaining(), 0);
}

#[tokio::test]
async fn off_by_default_no_title_is_asked_and_none_is_serialised() {
    let t = Arc::new(ScriptTransport::new());
    let mut p = poller(&t, 1, false);
    let snapshot = p.tick().await.snapshot;
    assert!(title_requests(&t).is_empty());
    assert!(snapshot.watches[0].rows.iter().all(|r| r.title.is_none()));
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("\"title\""), "{json}");
}

#[tokio::test]
async fn a_rate_limit_stops_this_ticks_asking_and_a_404_is_not_asked_again() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        &format!("/repository/commits/{}", sha(5)),
        429,
        r#"{"message":"429 Too Many Requests"}"#,
    );
    let mut p = poller(&t, 3, true);

    let first = p.tick().await.snapshot;
    assert_eq!(
        title_requests(&t).len(),
        1,
        "the 429 ends the tick's asking"
    );
    assert!(
        first.watches[0].error.is_none(),
        "a title is never an error on the watch"
    );
    assert!(first.watches[0].rows.iter().all(|r| r.title.is_none()));

    // Next tick: 5 is asked again (and found), 4 has no such commit (a 404),
    // 3 answers.
    commit_reply(&t, 5, "Fix the thing");
    t.reply(
        &format!("/repository/commits/{}", sha(4)),
        404,
        r#"{"message":"404 Commit Not Found"}"#,
    );
    commit_reply(&t, 3, "Line one\n\nThe body is not shown");
    let second = p.tick().await.snapshot;
    let rows = &second.watches[0].rows;
    assert_eq!(rows[0].title.as_deref(), Some("Fix the thing"));
    assert_eq!(rows[1].title, None);
    assert_eq!(rows[2].title.as_deref(), Some("Line one"));

    // Third tick: 2 and 1 are asked, and 4 (the 404) is not.
    commit_reply(&t, 2, "two");
    commit_reply(&t, 1, "one");
    p.tick().await;
    let asked = title_requests(&t);
    assert_eq!(
        asked.iter().filter(|p| p.contains(&sha(4))).count(),
        1,
        "{asked:?}"
    );
    assert_eq!(t.remaining(), 0, "every scripted answer was used");
}

#[test]
fn a_title_is_its_first_line_cut_to_a_row() {
    let long = "x".repeat(500);
    let cut = bridgewatch_core::model::first_line(&long).unwrap();
    assert_eq!(
        cut.chars().count(),
        bridgewatch_core::model::FAILURE_LINE_MAX
    );
    assert_eq!(
        bridgewatch_core::model::first_line("\n  \n Subject \nbody"),
        Some("Subject".into())
    );
}
