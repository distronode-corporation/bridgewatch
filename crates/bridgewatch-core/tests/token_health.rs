//! F3: "my builds" (`mine = true`) and token health, against hand-written
//! answers in the documented API shapes. No network, no keychain: a script
//! answers every request, and an in-memory store holds every credential.
//!
//! The shapes:
//! - GitLab `GET /user`: `{"id","username","name","bot"}`;
//!   `GET /personal_access_tokens/self`: `{"id","name","scopes","expires_at":"YYYY-MM-DD","active","revoked"}`;
//!   a 401 for an expired or revoked token: `{"error":"invalid_token","error_description":"Token is expired. ..."}`
//!   / `"Token was revoked. ..."`; any other: `{"message":"401 Unauthorized"}`.
//! - GitHub `GET /user`: `{"login","id"}`; every response of a token with an
//!   expiry carries `github-authentication-token-expiration: 2026-11-10 12:00:00 UTC`;
//!   a 401 is `{"message":"Bad credentials"}`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use bridgewatch_core::client::{HttpResponse, ScriptTransport};
use bridgewatch_core::health::{ExpiryOf, HealthState, TokenHealth};
use bridgewatch_core::notify::{Notification, NotifyKind};
use bridgewatch_core::oauth::store::{self, TokenSet};
use bridgewatch_core::poll::{Poller, Tick};
use bridgewatch_core::token::{Secret, TokenError, TokenProvider};

const HOUR: i64 = 3600;

/// A credential store in memory, shared by clones (the poller keeps one).
#[derive(Clone, Default)]
struct Memory(Arc<Mutex<BTreeMap<String, String>>>);

impl TokenProvider for Memory {
    fn keyring_get(&self, service: &str, user: &str) -> Result<String, TokenError> {
        self.0
            .lock()
            .unwrap()
            .get(service)
            .cloned()
            .ok_or_else(|| TokenError::NoEntry {
                service: service.into(),
                user: user.into(),
            })
    }
    fn keyring_set(&self, service: &str, _: &str, secret: &str) -> Result<(), TokenError> {
        self.0.lock().unwrap().insert(service.into(), secret.into());
        Ok(())
    }
    fn keyring_delete(&self, service: &str, _: &str) -> Result<(), TokenError> {
        self.0.lock().unwrap().remove(service);
        Ok(())
    }
    fn env(&self, name: &str) -> Option<String> {
        (name == "TOK").then(|| "tok-FIXTURE".to_string())
    }
    fn run(&self, _: &[String]) -> Result<String, TokenError> {
        Err(TokenError::Empty)
    }
}

/// The poller's clock, movable by the test.
#[derive(Clone)]
struct Clock(Arc<AtomicI64>);

impl Clock {
    fn at(secs: i64) -> Self {
        Self(Arc::new(AtomicI64::new(secs)))
    }
    fn set(&self, secs: i64) {
        self.0.store(secs, Ordering::SeqCst);
    }
    fn reader(&self) -> impl Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync + 'static {
        let at = self.0.clone();
        move || chrono::DateTime::from_timestamp(at.load(Ordering::SeqCst), 0).unwrap()
    }
}

fn config(raw: &str) -> bridgewatch_core::Config {
    toml::from_str(raw).expect("the table deserialises")
}

fn poller(raw: &str, store: &Memory, t: &Arc<ScriptTransport>, clock: &Clock) -> Poller {
    Poller::from_config_per_account_on(&config(raw), store, t.clone())
        .expect("the poller builds")
        .with_clock(clock.reader())
}

fn paths(t: &ScriptTransport) -> Vec<String> {
    t.requests().iter().map(|r| r.path.clone()).collect()
}

fn count(t: &ScriptTransport, fragment: &str) -> usize {
    t.requests()
        .iter()
        .filter(|r| r.path.contains(fragment))
        .count()
}

fn token_notes(tick: &Tick) -> Vec<&Notification> {
    tick.notifications
        .iter()
        .filter(|n| n.kind == NotifyKind::Token)
        .collect()
}

fn health<'a>(tick: &'a Tick, account: &str) -> Option<&'a TokenHealth> {
    tick.snapshot.tokens.iter().find(|h| h.account == account)
}

/// `n` answers of `body` for every request whose URL contains `fragment`.
fn replies(t: &ScriptTransport, fragment: &str, n: usize, response: HttpResponse) {
    for _ in 0..n {
        t.reply_with(fragment, response.clone());
    }
}

/// A real "now" for the tests whose expiry rides in a header: the header is
/// read against the wall clock (see `health::HEADER_NOW_SLACK_SECS`).
fn wall() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `2026-11-10 12:00:00 UTC`, the header's spelling.
fn header_time(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .unwrap()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string()
}

const GITLAB_MINE: &str = r#"
    [accounts.lab]
    token = { env = "TOK" }

    [[watches]]
    id = "mine"
    account = "lab"
    project = 1
    mine = true

    [[watches]]
    id = "also-mine"
    account = "lab"
    project = 2
    mine = true

    [[watches]]
    id = "everyone"
    account = "lab"
    project = 3
"#;

const GITLAB_USER: &str = r#"{"id":7,"username":"alice","name":"Alice","bot":false}"#;
const GITLAB_SELF_NO_EXPIRY: &str =
    r#"{"id":9,"name":"bw","scopes":["read_api"],"expires_at":null,"active":true,"revoked":false}"#;

// ---------------------------------------------------------------------------
// mine = true
// ---------------------------------------------------------------------------

/// GitLab is asked with `username=`, the owner is looked up ONCE for both
/// `mine` watches and every tick after, and a watch without `mine` is asked
/// as it always was.
#[tokio::test]
async fn gitlab_mine_sends_username_and_asks_who_once() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/user", 200, GITLAB_USER);
    t.reply("/personal_access_tokens/self", 200, GITLAB_SELF_NO_EXPIRY);
    for p in 1..=3 {
        replies(
            &t,
            &format!("/projects/{p}/pipelines"),
            3,
            HttpResponse::plain(200, "[]"),
        );
    }
    let mut p = poller(GITLAB_MINE, &Memory::default(), &t, &Clock::at(wall()));
    for _ in 0..3 {
        let tick = p.tick().await;
        assert!(
            tick.snapshot.watches.iter().all(|w| w.error.is_none()),
            "{:?}",
            tick.snapshot.watches
        );
    }

    assert_eq!(count(&t, "/user"), 1, "{:?}", paths(&t));
    for project in [1, 2] {
        let lists: Vec<String> = paths(&t)
            .into_iter()
            .filter(|p| p.starts_with(&format!("/projects/{project}/pipelines")))
            .collect();
        assert_eq!(lists.len(), 3);
        assert!(
            lists.iter().all(|p| p.contains("username=alice")),
            "{lists:?}"
        );
    }
    let everyone: Vec<String> = paths(&t)
        .into_iter()
        .filter(|p| p.starts_with("/projects/3/pipelines"))
        .collect();
    assert!(
        everyone.iter().all(|p| !p.contains("username=")),
        "{everyone:?}"
    );
}

/// A failed owner lookup is that watch's error, sends no list unfiltered,
/// and is asked again (not remembered) next tick.
#[tokio::test]
async fn a_failed_owner_lookup_is_the_watchs_error_and_is_asked_again() {
    let raw = r#"
        [accounts.lab]
        token = { env = "TOK" }
        rate_limit_backoff = { max_secs = 1 }

        [[watches]]
        id = "mine"
        account = "lab"
        project = 1
        mine = true
        poll = { live_secs = 1, idle_secs = 1 }

        [[watches]]
        id = "also-mine"
        account = "lab"
        project = 2
        mine = true
    "#;
    let t = Arc::new(ScriptTransport::new());
    t.reply("/personal_access_tokens/self", 200, GITLAB_SELF_NO_EXPIRY);
    t.reply("/user", 503, "{}");
    t.reply("/user", 200, GITLAB_USER);
    for project in [1, 2] {
        replies(
            &t,
            &format!("/projects/{project}/pipelines"),
            2,
            HttpResponse::plain(200, "[]"),
        );
    }
    let mut p = poller(raw, &Memory::default(), &t, &Clock::at(wall()));
    let first = p.tick().await;
    assert!(first.snapshot.watches[0].error.is_some());
    assert_eq!(count(&t, "/projects/1/pipelines"), 0, "no unfiltered list");
    assert!(first.list_errors.contains_key("mine"));
    assert!(first.snapshot.watches[1].error.is_some());
    assert_eq!(
        count(&t, "/user"),
        1,
        "one lookup for both watches: {:?}",
        paths(&t)
    );
    assert!(
        first.polled.is_empty(),
        "no list went out: {:?}",
        first.polled
    );
    // Not remembered: the next tick, once the backoff (1 s here) has
    // passed, asks again, and the lists go out held to that owner.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = p.tick().await;
    assert_eq!(count(&t, "/user"), 2, "{:?}", paths(&t));
    assert!(second.polled.contains("mine"), "{:?}", second.polled);
    for project in [1, 2] {
        let lists: Vec<String> = paths(&t)
            .into_iter()
            .filter(|p| p.starts_with(&format!("/projects/{project}/pipelines")))
            .collect();
        assert_eq!(lists.len(), 1, "{lists:?}");
        assert!(lists[0].contains("username=alice"), "{lists:?}");
    }
    // And remembered from then on: a third tick asks nobody.
    p.tick().await;
    assert_eq!(count(&t, "/user"), 2, "{:?}", paths(&t));
}

const GITHUB_MINE: &str = r#"
    [accounts.gh]
    provider = "github"
    token = { env = "TOK" }

    [[watches]]
    id = "mine"
    account = "gh"
    project = "octo/app"
    mine = true
"#;

/// GitHub is asked with `actor=`, the owner once.
#[tokio::test]
async fn github_mine_sends_actor_and_asks_who_once() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/user", 200, r#"{"login":"octocat","id":1}"#);
    replies(
        &t,
        "/actions/runs",
        3,
        HttpResponse::plain(200, r#"{"total_count":0,"workflow_runs":[]}"#),
    );
    let mut p = poller(GITHUB_MINE, &Memory::default(), &t, &Clock::at(wall()));
    for _ in 0..3 {
        let tick = p.tick().await;
        assert!(tick.snapshot.watches[0].error.is_none(), "{tick:?}");
    }
    assert_eq!(count(&t, "/user"), 1, "{:?}", paths(&t));
    let runs: Vec<String> = paths(&t)
        .into_iter()
        .filter(|p| p.contains("/actions/runs"))
        .collect();
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|p| p.contains("actor=octocat")), "{runs:?}");
    // GitHub's expiry rides on every response: nothing is asked for it.
    assert_eq!(paths(&t).len(), 4, "{:?}", paths(&t));
}

fn github_sign_in(now: i64, refresh_ends: i64, access_ends: i64) -> TokenSet {
    TokenSet {
        access_token: Secret::new("ghu_FIXTURE"),
        refresh_token: Some(Secret::new("ghr_FIXTURE")),
        expires_at: Some(access_ends as u64),
        refresh_expires_at: Some(refresh_ends as u64),
        scopes: Vec::new(),
        login: Some("octocat".into()),
        client_id: bridgewatch_core::oauth::clients::GITHUB_COM
            .unwrap()
            .to_string(),
        base_url: "https://api.github.com".into(),
        obtained_at: (now - HOUR) as u64,
    }
}

const GITHUB_SIGNED_IN: &str = r#"
    [accounts.gh]
    provider = "github"
    token = { oauth = true }

    [[watches]]
    id = "mine"
    account = "gh"
    project = "octo/app"
    mine = true
"#;

/// A sign-in names its user: `mine` asks nobody, and the sign-in's 8-hour
/// access token (stated in the expiry header too) is never read as a
/// token about to expire.
#[tokio::test]
async fn a_sign_in_names_the_user_and_its_access_token_is_not_a_warning() {
    let now = wall();
    let store = Memory::default();
    store::save(
        "gh",
        &github_sign_in(now, now + 180 * 24 * HOUR, now + 7 * HOUR),
        &store,
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    let mut with_header = HttpResponse::plain(200, r#"{"total_count":0,"workflow_runs":[]}"#);
    with_header.token_expiration = Some(header_time(now + 7 * HOUR));
    replies(&t, "/actions/runs", 2, with_header);
    let mut p = poller(GITHUB_SIGNED_IN, &store, &t, &Clock::at(now));
    for _ in 0..2 {
        let tick = p.tick().await;
        assert!(tick.snapshot.watches[0].error.is_none(), "{tick:?}");
        assert!(
            tick.snapshot.tokens.is_empty(),
            "{:?}",
            tick.snapshot.tokens
        );
        assert!(token_notes(&tick).is_empty());
    }
    assert_eq!(count(&t, "/user"), 0, "{:?}", paths(&t));
    assert!(
        paths(&t).iter().all(|p| p.contains("actor=octocat")),
        "{:?}",
        paths(&t)
    );
}

// ---------------------------------------------------------------------------
// Token health: expiry warnings, each once
// ---------------------------------------------------------------------------

const GITLAB_ONE: &str = r#"
    [accounts.lab]
    token = { env = "TOK" }

    [[watches]]
    id = "web"
    account = "lab"
    project = 1
"#;

/// 2026-11-20 00:00 UTC, the expiry the next tests' token states.
const EXPIRES: i64 = 1_795_132_800;

/// A GitLab token's expiry is asked ONCE; the strip says it from 72 h, and
/// a notification is sent at 72 h and at 24 h, each once, however many ticks
/// pass.
#[tokio::test]
async fn a_gitlab_expiry_warns_once_at_72h_and_once_at_24h() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/personal_access_tokens/self",
        200,
        r#"{"id":9,"name":"bw","scopes":["read_api"],"expires_at":"2026-11-20","active":true,"revoked":false}"#,
    );
    replies(
        &t,
        "/projects/1/pipelines",
        20,
        HttpResponse::plain(200, "[]"),
    );
    let clock = Clock::at(EXPIRES - 100 * HOUR);
    let mut p = poller(GITLAB_ONE, &Memory::default(), &t, &clock);

    // 100 h out: nothing to say.
    let tick = p.tick().await;
    assert!(
        tick.snapshot.tokens.is_empty(),
        "{:?}",
        tick.snapshot.tokens
    );
    assert!(token_notes(&tick).is_empty());

    let mut said = Vec::new();
    for (left, ticks) in [(71, 3), (30, 2), (23, 3), (1, 2)] {
        clock.set(EXPIRES - left * HOUR);
        for _ in 0..ticks {
            let tick = p.tick().await;
            let h = health(&tick, "lab").expect("a warning in the strip");
            assert_eq!(h.state, HealthState::Expiring);
            assert_eq!(h.of, ExpiryOf::Token);
            assert_eq!(h.expires_at, Some(EXPIRES));
            assert_eq!(h.warn_hours, Some(if left > 24 { 72 } else { 24 }));
            assert!(h.message.contains("2026-11-20 00:00 UTC"), "{}", h.message);
            said.extend(token_notes(&tick).iter().map(|n| n.key.clone()));
        }
    }
    assert_eq!(
        said,
        [
            format!("lab|expiring|72|{EXPIRES}"),
            format!("lab|expiring|24|{EXPIRES}"),
        ],
        "one notification at 72 h, one at 24 h"
    );
    assert_eq!(
        count(&t, "/personal_access_tokens/self"),
        1,
        "asked once for the life of the poller"
    );
    assert_eq!(count(&t, "/user"), 0, "nobody asks who without mine");
}

/// No new request per poll: after the first tick's one expiry request, each
/// tick sends exactly what it sent before F3 (one list per watch).
#[tokio::test]
async fn token_health_adds_no_request_per_poll() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/personal_access_tokens/self", 200, GITLAB_SELF_NO_EXPIRY);
    replies(
        &t,
        "/projects/1/pipelines",
        5,
        HttpResponse::plain(200, "[]"),
    );
    let mut p = poller(GITLAB_ONE, &Memory::default(), &t, &Clock::at(wall()));
    let mut per_tick = Vec::new();
    for _ in 0..5 {
        let before = t.requests().len();
        p.tick().await;
        per_tick.push(t.requests().len() - before);
    }
    assert_eq!(per_tick, [2, 1, 1, 1, 1], "{:?}", paths(&t));
}

/// A token first seen inside 24 h is ONE notification (the 24 h one), not
/// the 72 h one and the 24 h one at once.
#[tokio::test]
async fn a_token_first_seen_inside_24h_is_one_notification() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/personal_access_tokens/self",
        200,
        r#"{"id":9,"name":"bw","scopes":["read_api"],"expires_at":"2026-11-20","active":true}"#,
    );
    replies(
        &t,
        "/projects/1/pipelines",
        2,
        HttpResponse::plain(200, "[]"),
    );
    let mut p = poller(
        GITLAB_ONE,
        &Memory::default(),
        &t,
        &Clock::at(EXPIRES - 5 * HOUR),
    );
    let first = p.tick().await;
    let keys: Vec<&str> = token_notes(&first).iter().map(|n| n.key.as_str()).collect();
    assert_eq!(keys, [format!("lab|expiring|24|{EXPIRES}").as_str()]);
    assert!(token_notes(&p.tick().await).is_empty());
}

/// GitHub's expiry header is read off the poll's own responses; a header
/// that reads as "now" (go-github #3708) is not believed.
#[tokio::test]
async fn a_github_expiry_header_is_read_from_the_poll_and_a_now_header_is_not() {
    let now = wall();
    let t = Arc::new(ScriptTransport::new());
    let runs = r#"{"total_count":0,"workflow_runs":[]}"#;
    let mut bogus = HttpResponse::plain(200, runs);
    bogus.token_expiration = Some(header_time(now));
    t.reply_with("/actions/runs", bogus);
    let mut real = HttpResponse::plain(200, runs);
    real.token_expiration = Some(header_time(now + 50 * HOUR));
    t.reply_with("/actions/runs", real);
    let raw = GITHUB_MINE.replace("mine = true", "");
    let mut p = poller(&raw, &Memory::default(), &t, &Clock::at(now));

    let first = p.tick().await;
    assert!(
        first.snapshot.tokens.is_empty(),
        "{:?}",
        first.snapshot.tokens
    );
    let second = p.tick().await;
    let h = health(&second, "gh").expect("expiring");
    assert_eq!((h.state, h.warn_hours), (HealthState::Expiring, Some(72)));
    assert_eq!(token_notes(&second).len(), 1);
    assert_eq!(paths(&t).len(), 2, "no request of its own: {:?}", paths(&t));
}

// ---------------------------------------------------------------------------
// Token health: invalid, revoked, expired
// ---------------------------------------------------------------------------

/// GitLab's 401 says which: revoked, expired, or neither (invalid). Each is
/// one notification, not repeated on the ticks after.
#[tokio::test]
async fn gitlab_401s_tell_revoked_expired_and_invalid_apart() {
    let refusal = |description: Option<&str>| match description {
        Some(d) => HttpResponse::plain(
            401,
            &format!(r#"{{"error":"invalid_token","error_description":"{d}"}}"#),
        ),
        None => HttpResponse::plain(401, r#"{"message":"401 Unauthorized"}"#),
    };
    for (description, state) in [
        (
            Some("Token was revoked. You have to re-authorize from the user."),
            HealthState::Revoked,
        ),
        (
            Some("Token is expired. You can either do re-authorization or token refresh."),
            HealthState::Expired,
        ),
        (None, HealthState::Invalid),
    ] {
        let t = Arc::new(ScriptTransport::new());
        t.reply_with("/personal_access_tokens/self", refusal(description));
        replies(&t, "/projects/1/pipelines", 3, refusal(description));
        let mut p = poller(GITLAB_ONE, &Memory::default(), &t, &Clock::at(wall()));

        let first = p.tick().await;
        let h = health(&first, "lab").unwrap_or_else(|| panic!("{state:?}: {first:?}"));
        assert_eq!(h.state, state, "{description:?}");
        assert_eq!(token_notes(&first).len(), 1, "{state:?}");
        assert!(
            h.message.contains("save a new token in Settings"),
            "{}",
            h.message
        );
        for _ in 0..2 {
            let tick = p.tick().await;
            assert_eq!(health(&tick, "lab").map(|h| h.state), Some(state));
            assert!(token_notes(&tick).is_empty(), "said once: {state:?}");
        }
    }
}

/// GitHub's 401 says "Bad credentials" whatever the cause: invalid, unless a
/// known expiry has passed, which makes it expired.
#[tokio::test]
async fn a_github_401_is_invalid_unless_its_known_expiry_has_passed() {
    let now = wall();
    let t = Arc::new(ScriptTransport::new());
    let mut ok = HttpResponse::plain(200, r#"{"total_count":0,"workflow_runs":[]}"#);
    ok.token_expiration = Some(header_time(now + 3 * HOUR));
    t.reply_with("/actions/runs", ok);
    for _ in 0..3 {
        t.reply("/actions/runs", 401, r#"{"message":"Bad credentials"}"#);
    }
    let raw = GITHUB_MINE.replace("mine = true", "");
    let clock = Clock::at(now);
    let mut p = poller(&raw, &Memory::default(), &t, &clock);
    let first = p.tick().await;
    assert_eq!(
        health(&first, "gh").map(|h| h.state),
        Some(HealthState::Expiring)
    );

    // Refused before the stated expiry: invalid.
    clock.set(now + HOUR);
    let second = p.tick().await;
    assert!(second.polled.contains("mine"));
    assert_eq!(
        health(&second, "gh").map(|h| h.state),
        Some(HealthState::Invalid)
    );

    // The same refusal once the expiry has passed: expired.
    clock.set(now + 4 * HOUR);
    let tick = p.tick().await;
    let h = health(&tick, "gh").unwrap();
    assert_eq!(h.state, HealthState::Expired);
    assert!(h.message.contains("expired"), "{}", h.message);
}

/// A sign-in whose refresh is refused: revoked while its refresh token had
/// time left, expired once that time has passed. Inside 24 h of the refresh
/// token's end it warns, as a sign-in.
#[tokio::test]
async fn a_sign_in_tells_revoked_from_expired_and_warns_before_its_end() {
    let now = wall();
    // Warns: the access token is fine, the refresh token ends in 20 h.
    {
        let store = Memory::default();
        store::save(
            "gh",
            &github_sign_in(now, now + 20 * HOUR, now + 7 * HOUR),
            &store,
        )
        .unwrap();
        let t = Arc::new(ScriptTransport::new());
        t.reply(
            "/actions/runs",
            200,
            r#"{"total_count":0,"workflow_runs":[]}"#,
        );
        let raw = GITHUB_SIGNED_IN.replace("mine = true", "");
        let mut p = poller(&raw, &store, &t, &Clock::at(now));
        let tick = p.tick().await;
        let h = health(&tick, "gh").expect("a warning");
        assert_eq!(
            (h.state, h.of, h.warn_hours),
            (HealthState::Expiring, ExpiryOf::SignIn, Some(24))
        );
        assert!(
            h.message
                .ends_with("; before then, sign in again in Settings (Accounts)."),
            "{}",
            h.message
        );
        assert_eq!(token_notes(&tick).len(), 1);
    }
    for (refresh_ends, state) in [
        (now + 30 * 24 * HOUR, HealthState::Revoked),
        (now - HOUR, HealthState::Expired),
    ] {
        let store = Memory::default();
        // The access token has run out, so the first request refreshes.
        store::save("gh", &github_sign_in(now, refresh_ends, now - 60), &store).unwrap();
        let t = Arc::new(ScriptTransport::new());
        t.reply(
            "/login/oauth/access_token",
            200,
            r#"{"error":"bad_refresh_token","error_description":"The refresh token passed is incorrect or expired."}"#,
        );
        let raw = GITHUB_SIGNED_IN.replace("mine = true", "");
        let mut p = poller(&raw, &store, &t, &Clock::at(now));
        let tick = p.tick().await;
        let h = health(&tick, "gh").unwrap_or_else(|| panic!("{state:?}: {tick:?}"));
        assert_eq!((h.state, h.of), (state, ExpiryOf::SignIn));
        assert!(h.message.contains("sign in again"), "{}", h.message);
        assert_eq!(token_notes(&tick).len(), 1);
    }
}

/// The warnings survive a restart: the ledger they are recorded in is the
/// persisted one, so a new poller does not say them again.
#[tokio::test]
async fn a_warning_is_not_repeated_after_a_restart() {
    let dir = std::env::temp_dir().join(format!("bw-f3-ledger-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = dir.join("notify.json");
    let _ = std::fs::remove_file(&ledger);
    let said = |t: &Arc<ScriptTransport>| {
        t.reply(
            "/personal_access_tokens/self",
            200,
            r#"{"id":9,"name":"bw","scopes":["read_api"],"expires_at":"2026-11-20","active":true}"#,
        );
        t.reply("/projects/1/pipelines", 200, "[]");
    };
    let mut keys = Vec::new();
    for _ in 0..2 {
        let t = Arc::new(ScriptTransport::new());
        said(&t);
        let mut p = poller(
            GITLAB_ONE,
            &Memory::default(),
            &t,
            &Clock::at(EXPIRES - 10 * HOUR),
        )
        .with_ledger(ledger.clone());
        let tick = p.tick().await;
        keys.push(token_notes(&tick).len());
    }
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(keys, [1, 0]);
}

/// A credential accepted again forgets its "revoked" and "invalid", so the
/// next refusal is said again; an expiry key is per expiry, so a replaced
/// token with a new date warns again.
#[test]
fn a_fixed_token_is_warned_about_again_and_a_new_expiry_is_a_new_warning() {
    use bridgewatch_core::notify::{NotifyLedger, token_notifications};
    let revoked = TokenHealth {
        account: "lab".into(),
        state: HealthState::Revoked,
        of: ExpiryOf::Token,
        expires_at: None,
        warn_hours: None,
        message: "m".into(),
    };
    let mut ledger = NotifyLedger::default();
    let said = |h: &[TokenHealth], ok: &[String], l: &mut NotifyLedger| {
        token_notifications(h, ok, l, 0).len()
    };
    assert_eq!(said(std::slice::from_ref(&revoked), &[], &mut ledger), 1);
    assert_eq!(said(std::slice::from_ref(&revoked), &[], &mut ledger), 0);
    assert_eq!(said(&[], &["lab".into()], &mut ledger), 0);
    assert_eq!(
        said(&[revoked], &[], &mut ledger),
        1,
        "said again after a fix"
    );

    let expiring = |at: i64| TokenHealth {
        state: HealthState::Expiring,
        expires_at: Some(at),
        warn_hours: Some(72),
        account: "lab".into(),
        of: ExpiryOf::Token,
        message: "m".into(),
    };
    assert_eq!(said(&[expiring(100 * HOUR)], &[], &mut ledger), 1);
    assert_eq!(
        said(&[expiring(100 * HOUR)], &["lab".into()], &mut ledger),
        0
    );
    assert_eq!(
        said(&[expiring(200 * HOUR)], &[], &mut ledger),
        1,
        "a new token"
    );
}

/// A GitHub token first seen refused, with its expiry header already in the
/// past, is expired rather than invalid: the header is read on a 401 too.
#[tokio::test]
async fn a_github_401_carrying_a_past_expiry_is_expired() {
    let now = wall();
    let t = Arc::new(ScriptTransport::new());
    let mut refused = HttpResponse::plain(401, r#"{"message":"Bad credentials"}"#);
    refused.token_expiration = Some(header_time(now - 2 * HOUR));
    t.reply_with("/actions/runs", refused);
    let raw = GITHUB_MINE.replace("mine = true", "");
    let mut p = poller(&raw, &Memory::default(), &t, &Clock::at(now));
    let tick = p.tick().await;
    assert_eq!(
        health(&tick, "gh").map(|h| h.state),
        Some(HealthState::Expired)
    );
}

/// `/personal_access_tokens/self` refusing a credential it cannot describe
/// (a CI job token) says nothing about the credential: no health is
/// reported, even on a tick whose own list did not get an answer.
#[tokio::test]
async fn a_refused_expiry_lookup_is_not_a_refused_token() {
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/personal_access_tokens/self",
        401,
        r#"{"message":"401 Unauthorized"}"#,
    );
    t.reply("/projects/1/pipelines", 503, "{}");
    let mut p = poller(GITLAB_ONE, &Memory::default(), &t, &Clock::at(wall()));
    let tick = p.tick().await;
    assert!(
        tick.snapshot.tokens.is_empty(),
        "{:?}",
        tick.snapshot.tokens
    );
    assert!(token_notes(&tick).is_empty());
}

/// A credential accepted while its expiry warns is still accepted: a later
/// refusal is said again; and an expiry with no date is forgotten on
/// recovery, so the next one is said too. An accepted token past its date
/// is titled "expires now", never "expires in 0s".
#[test]
fn accepted_while_expiring_still_clears_refusals_and_titles_say_now() {
    use bridgewatch_core::notify::{NotifyLedger, token_notifications};
    let h = |state, expires_at: Option<i64>, warn_hours| TokenHealth {
        account: "lab".into(),
        state,
        of: ExpiryOf::Token,
        expires_at,
        warn_hours,
        message: "m".into(),
    };
    let mut ledger = NotifyLedger::default();
    let lab = ["lab".to_string()];
    let invalid = h(HealthState::Invalid, None, None);
    assert_eq!(
        token_notifications(std::slice::from_ref(&invalid), &[], &mut ledger, 0).len(),
        1
    );
    let expiring = h(HealthState::Expiring, Some(10), Some(24));
    let said = token_notifications(std::slice::from_ref(&expiring), &lab, &mut ledger, 10);
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].title, "lab: the token expires now");
    assert_eq!(
        token_notifications(std::slice::from_ref(&invalid), &[], &mut ledger, 20).len(),
        1
    );

    let dateless = h(HealthState::Expired, None, None);
    assert_eq!(
        token_notifications(std::slice::from_ref(&dateless), &[], &mut ledger, 0).len(),
        1
    );
    assert_eq!(token_notifications(&[], &lab, &mut ledger, 0).len(), 0);
    assert_eq!(
        token_notifications(&[dateless], &[], &mut ledger, 0).len(),
        1
    );
}

/// Seer and cubic, #67: a persisted "revoked" survives a restart whose tick
/// had no answer (a dropped connection), so the same refusal afterwards is
/// not announced a second time.
#[tokio::test]
async fn a_persisted_refusal_survives_a_tick_without_an_answer() {
    let raw = r#"
        [accounts.lab]
        token = { env = "TOK" }
        rate_limit_backoff = { max_secs = 1 }

        [[watches]]
        id = "web"
        account = "lab"
        project = 1
        poll = { live_secs = 1, idle_secs = 1 }
    "#;
    let dir = std::env::temp_dir().join(format!("bw-f3-kept-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = dir.join("notify.json");
    let revoked = || {
        HttpResponse::plain(
            401,
            r#"{"error":"invalid_token","error_description":"Token was revoked. You have to re-authorize from the user."}"#,
        )
    };

    // Before the restart: revoked, said once, and persisted.
    let t = Arc::new(ScriptTransport::new());
    t.reply_with("/personal_access_tokens/self", revoked());
    t.reply_with("/projects/1/pipelines", revoked());
    let mut p = poller(raw, &Memory::default(), &t, &Clock::at(wall())).with_ledger(ledger.clone());
    assert_eq!(token_notes(&p.tick().await).len(), 1);

    // After it: the first tick gets no answer at all (no reply scripted is a
    // transport error), then the token is refused again.
    let t = Arc::new(ScriptTransport::new());
    let mut p = poller(raw, &Memory::default(), &t, &Clock::at(wall())).with_ledger(ledger.clone());
    let offline = p.tick().await;
    assert!(token_notes(&offline).is_empty());
    let kept = std::fs::read_to_string(&ledger).unwrap();
    assert!(kept.contains("lab|revoked"), "the key is kept: {kept}");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    t.reply_with("/projects/1/pipelines", revoked());
    let again = p.tick().await;
    assert!(again.polled.contains("web"));
    assert_eq!(
        health(&again, "lab").map(|h| h.state),
        Some(HealthState::Revoked)
    );
    assert!(token_notes(&again).is_empty(), "not announced twice");
    std::fs::remove_dir_all(&dir).ok();
}

/// cubic, #67: a 200 whose body does not decode still accepted the
/// credential; the expiry lookup's failure must not put an earlier refusal
/// back.
#[tokio::test]
async fn a_malformed_expiry_answer_keeps_the_acceptance() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/personal_access_tokens/self", 200, "not json");
    let mut p = poller(GITLAB_ONE, &Memory::default(), &t, &Clock::at(wall()));
    // Earlier, a list was refused.
    t.reply(
        "/projects/1/pipelines",
        401,
        r#"{"message":"401 Unauthorized"}"#,
    );
    let client = p.clients()["lab"].clone();
    let _ = client
        .list_pipelines(
            &bridgewatch_core::config::ProjectRef::Id(1),
            &bridgewatch_core::client::ListQuery::exact("main", None, 1),
        )
        .await;
    assert!(client.credential().refused.is_some());
    // Then the expiry lookup answers 200 with a body that will not decode.
    assert!(client.read_token_expiry().await.is_err());
    assert_eq!(client.credential().refused, None, "the 200 accepted it");
    let _ = &mut p;
}

/// cubic, #67: a GitLab sign-in that states no end, refused as revoked, is
/// told to sign in again, not to save a token.
#[tokio::test]
async fn a_revoked_sign_in_without_an_end_says_sign_in_again() {
    let now = wall();
    let raw = r#"
        [accounts.gl]
        token = { oauth = { client_id = "gl-app-0001" } }

        [[watches]]
        id = "web"
        account = "gl"
        project = 1
    "#;
    let store = Memory::default();
    store::save(
        "gl",
        &TokenSet {
            access_token: Secret::new("gl-oauth-FIXTURE"),
            refresh_token: Some(Secret::new("gl-refresh-FIXTURE")),
            expires_at: Some((now + 2 * HOUR) as u64),
            refresh_expires_at: None,
            scopes: vec!["read_api".into()],
            login: Some("alice".into()),
            client_id: "gl-app-0001".into(),
            base_url: "https://gitlab.com".into(),
            // Too new to have run out: the 401 is passed through unrefreshed.
            obtained_at: (now - 5) as u64,
        },
        &store,
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/projects/1/pipelines",
        401,
        r#"{"error":"invalid_token","error_description":"Token was revoked. You have to re-authorize from the user."}"#,
    );
    let mut p = poller(raw, &store, &t, &Clock::at(now));
    let tick = p.tick().await;
    let h = health(&tick, "gl").unwrap_or_else(|| panic!("{tick:?}"));
    assert_eq!((h.state, h.of), (HealthState::Revoked, ExpiryOf::SignIn));
    assert!(h.message.contains("sign in again"), "{}", h.message);
    assert!(!h.message.contains("save a new token"), "{}", h.message);
}
