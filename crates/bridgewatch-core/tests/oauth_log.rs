//! No secret of a sign-in reaches a log line.
//!
//! A whole GitHub sign-in (device code, pending, slow down, approval, `/user`)
//! and a whole GitLab refresh-and-rotate are run with every level captured,
//! including their failure paths, and the text is searched for every device
//! code, user code, access token and refresh token the script handed out.
//!
//! ⛔ EVERY TEST IN THIS FILE CAPTURES LOGS. The binary's one subscriber is
//! installed by the first `support::with_log`, and a callsite reached before
//! it is cached as disabled for the life of the process (the mechanism is in
//! `tests/support/mod.rs`), so a test here that ran any of this code outside a
//! capture could silently empty another test's.

mod support;

use std::sync::Arc;

use bridgewatch_core::client::{RequestRing, ScriptTransport, client_for};
use bridgewatch_core::config::{Account, OAuthSource, Provider, TokenSource};
use bridgewatch_core::oauth::{
    self, BuiltinClients, Endpoints, OAuthSession, OAuthTransport, store,
};
use bridgewatch_core::token::Secret;
use support::{MemoryStore, runtime, with_log};

const BUILTIN: BuiltinClients = BuiltinClients {
    github_com: Some("Iv23liBRIDGEWATCH"),
    github_app_slug: Some("bridgewatch-ci"),
    gitlab_com: Some("gl-app-0001"),
};

/// Every value the scripts below hand out that must never be logged.
const SECRETS: &[&str] = &[
    "gh-DEVICE-CODE-SECRET",
    "USER-CODE-SECRET",
    "ghu_ACCESS_SECRET",
    "ghr_REFRESH_SECRET",
    "gl-ACCESS-SECRET-1",
    "gl-REFRESH-SECRET-1",
    "gl-ACCESS-SECRET-2",
    "gl-REFRESH-SECRET-2",
];

fn assert_clean(log: &str) {
    for secret in SECRETS {
        assert!(!log.contains(secret), "{secret} reached the log:\n{log}");
    }
}

fn account(provider: Provider) -> Account {
    Account {
        token: TokenSource::Oauth(OAuthSource::default()),
        ..Account::for_provider(provider)
    }
}

#[test]
fn a_whole_github_sign_in_logs_no_code_and_no_token() {
    support::start_capture();
    let runtime = runtime();
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/login/device/code",
        200,
        r#"{"device_code":"gh-DEVICE-CODE-SECRET","user_code":"USER-CODE-SECRET","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
    );
    t.reply(
        "/login/oauth/access_token",
        200,
        r#"{"error":"authorization_pending"}"#,
    );
    t.reply(
        "/login/oauth/access_token",
        200,
        r#"{"error":"slow_down","interval":10}"#,
    );
    t.reply(
        "/login/oauth/access_token",
        200,
        r#"{"access_token":"ghu_ACCESS_SECRET","expires_in":28800,"refresh_token":"ghr_REFRESH_SECRET","token_type":"bearer","scope":""}"#,
    );
    // `/user` fails, which is the path that logs a warning with an error in it.
    t.reply("api.github.com/user", 500, "{}");
    let memory = MemoryStore::new();
    let account = account(Provider::Github);

    let (result, log) = with_log(|| {
        runtime.block_on(async {
            let endpoints = Endpoints::for_account(&account);
            let device = oauth::start(t.as_ref(), &endpoints, "Iv23liBRIDGEWATCH").await?;
            let reply = oauth::wait_for_token(
                t.as_ref(),
                &endpoints,
                "Iv23liBRIDGEWATCH",
                &device,
                |_| std::future::ready(()),
                std::future::pending(),
                &mut |_| {},
            )
            .await?;
            oauth::complete_sign_in(
                "gh",
                &account,
                "Iv23liBRIDGEWATCH",
                reply,
                t.clone(),
                memory.as_ref(),
                1_000,
            )
            .await
        })
    });
    let (set, _) = result.expect("signed in");
    assert_eq!(set.access_token.expose(), "ghu_ACCESS_SECRET");
    assert!(
        log.contains("oauth request"),
        "the requests ARE logged, by path:\n{log}"
    );
    assert!(log.contains("signed in, but could not ask who"), "{log}");
    assert_clean(&log);
}

#[test]
fn a_gitlab_refresh_rotation_and_a_refused_refresh_log_no_token() {
    support::start_capture();
    let runtime = runtime();
    let memory = MemoryStore::new();
    let first = oauth::TokenSet {
        access_token: Secret::new("gl-ACCESS-SECRET-1"),
        refresh_token: Some(Secret::new("gl-REFRESH-SECRET-1")),
        expires_at: Some(1_000),
        refresh_expires_at: None,
        scopes: vec!["read_api".into()],
        login: Some("alice".into()),
        client_id: "gl-app-0001".into(),
        base_url: "https://gitlab.com".into(),
        obtained_at: 0,
    };
    store::save("gl", &first, memory.as_ref()).unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply(
        "/oauth/token",
        200,
        r#"{"access_token":"gl-ACCESS-SECRET-2","expires_in":7200,"refresh_token":"gl-REFRESH-SECRET-2","scope":"read_api"}"#,
    );
    t.reply("/api/v4/user", 200, r#"{"id":7,"username":"alice"}"#);
    t.reply("/oauth/token", 400, r#"{"error":"invalid_grant"}"#);
    let account = account(Provider::Gitlab);
    let now = Arc::new(std::sync::atomic::AtomicU64::new(2_000));
    let session = OAuthSession::new(
        "gl",
        &account,
        &OAuthSource::default(),
        &BUILTIN,
        t.clone(),
        memory.clone(),
    )
    .unwrap()
    .with_clock({
        let now = now.clone();
        Arc::new(move || now.load(std::sync::atomic::Ordering::SeqCst))
    });
    let client = client_for(
        &oauth::bearer_account(&account),
        &Secret::new(""),
        Arc::new(OAuthTransport::new(t.clone(), Arc::new(session))),
        RequestRing::new(4),
    )
    .unwrap();

    // A store that refuses the rotated pair (the path that warns), then a
    // second refresh, hours later, that the server refuses.
    memory.refuse_writes(true);
    let (result, log) = with_log(|| {
        runtime.block_on(async {
            client.current_user().await.expect("the rotated pair works");
            now.store(2_000 + 7_200, std::sync::atomic::Ordering::SeqCst);
            client.current_user().await
        })
    });
    let err = result.unwrap_err();
    assert!(err.to_string().contains("sign in again"), "{err}");
    assert!(
        log.contains("could not save the refreshed sign-in"),
        "{log}"
    );
    assert!(log.contains("could not be refreshed"), "{log}");
    assert_clean(&log);
    assert_clean(&format!("{err} {err:?}"));
}
