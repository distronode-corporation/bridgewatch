//! Signing in with GitHub and GitLab, and keeping the sign-in fresh.
//!
//! ⛔ Nothing here reaches a network or a keychain. Every request is answered
//! by a [`ScriptTransport`] with bodies written from the providers' documented
//! shapes, holding only the keys that are decoded and invented values; every
//! sign-in is stored in [`support::MemoryStore`].
//!
//! The logs of a whole sign-in and refresh are held to "no secret" in
//! `tests/oauth_log.rs`, a binary of its own because it captures logs.

mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bridgewatch_core::client::{
    CiClient, ClientError, HttpRequest, HttpResponse, RequestRing, ScriptTransport, Transport,
    client_for,
};
use bridgewatch_core::config::{
    self, Account, OAuthSource, ProjectRef, Provider, Severity, TokenSource,
};
use bridgewatch_core::oauth::{
    self, BuiltinClients, Endpoints, OAuthError, OAuthSession, OAuthTransport, Progress, TokenSet,
    store,
};
use bridgewatch_core::token::{self, Secret, TokenError};
use support::MemoryStore;

/// A build whose applications are registered. The shipped build's are all
/// `None`, and the tests that care say which they use.
const REGISTERED: BuiltinClients = BuiltinClients {
    github_com: Some("Iv23liBRIDGEWATCH"),
    github_app_slug: Some("bridgewatch-ci"),
    gitlab_com: Some("gl-app-0001"),
};

const GH_DEVICE: &str = r#"{"device_code":"gh-DEVICE-CODE-0001","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#;
const GH_TOKEN: &str = r#"{"access_token":"ghu_ACCESS0001","expires_in":28800,"refresh_token":"ghr_REFRESH0001","refresh_token_expires_in":15897600,"token_type":"bearer","scope":""}"#;
const GH_USER: &str = r#"{"id":583231,"login":"octocat"}"#;
const GL_DEVICE: &str = r#"{"device_code":"gl-DEVICE-CODE-0001","user_code":"0A44L90H","verification_uri":"https://gitlab.com/oauth/device","verification_uri_complete":"https://gitlab.com/oauth/device?user_code=0A44L90H","expires_in":300,"interval":5}"#;
const GL_TOKEN_1: &str = r#"{"access_token":"gl-ACCESS-0001","token_type":"Bearer","expires_in":7200,"refresh_token":"gl-REFRESH-0001","scope":"read_api","created_at":1000}"#;
const GL_TOKEN_2: &str = r#"{"access_token":"gl-ACCESS-0002","token_type":"Bearer","expires_in":7200,"refresh_token":"gl-REFRESH-0002","scope":"read_api","created_at":9000}"#;
const GL_USER: &str = r#"{"id":7,"username":"alice","name":"Alice Doe","bot":false}"#;

fn oauth_account(provider: Provider, base_url: Option<&str>, client_id: Option<&str>) -> Account {
    let mut account = Account::for_provider(provider);
    if let Some(base) = base_url {
        account.base_url = base.to_string();
    }
    account.token = TokenSource::Oauth(OAuthSource {
        client_id: client_id.map(str::to_string),
    });
    account
}

fn source(account: &Account) -> OAuthSource {
    match &account.token {
        TokenSource::Oauth(s) => s.clone(),
        other => panic!("not an oauth account: {other:?}"),
    }
}

/// A sleep that records what it was asked for and returns at once.
fn recording_sleep(slept: Arc<Mutex<Vec<u64>>>) -> impl Fn(Duration) -> std::future::Ready<()> {
    move |d| {
        slept.lock().unwrap().push(d.as_secs());
        std::future::ready(())
    }
}

fn body_of(request: &HttpRequest) -> String {
    request.body.clone().unwrap_or_default()
}

fn header(request: &HttpRequest, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

// ---------------------------------------------------------------------------
// The device flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_github_sign_in_waits_through_pending_and_slow_down_and_stores_the_set() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/login/device/code", 200, GH_DEVICE);
    // GitHub answers a pending poll with 200 and an error body.
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
    t.reply("/login/oauth/access_token", 200, GH_TOKEN);
    t.reply("api.github.com/user", 200, GH_USER);

    let account = oauth_account(Provider::Github, None, None);
    let endpoints = Endpoints::for_account(&account);
    assert_eq!(
        endpoints.device_code_url,
        "https://github.com/login/device/code"
    );
    assert_eq!(
        endpoints.token_url,
        "https://github.com/login/oauth/access_token"
    );
    let client_id = oauth::client_id_for(&account, &source(&account), &REGISTERED).unwrap();
    assert_eq!(client_id, "Iv23liBRIDGEWATCH");

    let device = oauth::start(t.as_ref(), &endpoints, &client_id)
        .await
        .unwrap();
    assert_eq!(device.user_code, "WDJB-MJHT");
    assert_eq!(device.verification_uri, "https://github.com/login/device");

    let slept = Arc::new(Mutex::new(Vec::new()));
    let mut seen = Vec::new();
    let reply = oauth::wait_for_token(
        t.as_ref(),
        &endpoints,
        &client_id,
        &device,
        recording_sleep(slept.clone()),
        std::future::pending(),
        &mut |p| seen.push(p),
    )
    .await
    .expect("the third poll is approved");
    // The interval, then the interval again, then five more after slow_down
    // (which also named 10 itself).
    assert_eq!(*slept.lock().unwrap(), vec![5, 5, 10]);
    assert_eq!(
        seen,
        vec![
            Progress::Waiting { polls: 1 },
            Progress::SlowedDown { interval: 10 }
        ]
    );

    let memory = MemoryStore::new();
    let (set, signed_in) = oauth::complete_sign_in(
        "gh",
        &account,
        &client_id,
        reply,
        t.clone(),
        memory.as_ref(),
        1_000,
    )
    .await
    .expect("stored");
    assert_eq!(signed_in.login.as_deref(), Some("octocat"));
    assert_eq!(set.expires_at, Some(1_000 + 28_800));
    assert_eq!(set.refresh_expires_at, Some(1_000 + 15_897_600));
    assert!(
        set.scopes.is_empty(),
        "a GitHub App has permissions, not scopes"
    );

    let requests = t.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(body_of(&requests[0]), "client_id=Iv23liBRIDGEWATCH");
    assert_eq!(
        header(&requests[0], "accept").as_deref(),
        Some("application/json")
    );
    for poll in &requests[1..4] {
        assert_eq!(poll.method, "POST");
        let body = body_of(poll);
        assert!(body.contains("client_id=Iv23liBRIDGEWATCH"), "{body}");
        assert!(body.contains("device_code=gh-DEVICE-CODE-0001"), "{body}");
        assert!(
            body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"),
            "{body}"
        );
        assert!(!body.contains("client_secret"), "{body}");
    }
    assert_eq!(requests[4].method, "GET");
    assert_eq!(
        header(&requests[4], "authorization").as_deref(),
        Some("Bearer ghu_ACCESS0001")
    );
    assert_eq!(t.remaining(), 0);
}

/// The stored document's field names are the layout; a rename would strand
/// every stored sign-in.
#[tokio::test]
async fn the_token_set_is_stored_as_one_json_document_under_a_bridgewatch_entry() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/oauth/authorize_device", 200, GL_DEVICE);
    t.reply("/oauth/token", 200, GL_TOKEN_1);
    t.reply("gitlab.com/api/v4/user", 200, GL_USER);
    let account = oauth_account(Provider::Gitlab, None, None);
    let memory = MemoryStore::new();
    sign_in("gl", &account, &REGISTERED, t.clone(), &memory).await;

    let raw = memory
        .get("bridgewatch:oauth:gl", "oauth")
        .expect("the entry the module documents");
    let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let mut keys: Vec<&str> = doc
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "access_token",
            "base_url",
            "client_id",
            "expires_at",
            "login",
            "obtained_at",
            "refresh_expires_at",
            "refresh_token",
            "scopes",
            "version"
        ]
    );
    assert_eq!(doc["scopes"], serde_json::json!(["read_api"]));
    assert_eq!(doc["login"], "alice");
    assert_eq!(doc["base_url"], "https://gitlab.com");
    assert_eq!(doc["client_id"], "gl-app-0001");
    // Nothing under the `own` entry's address.
    assert!(memory.get("bridgewatch:gl", "token").is_none());
}

async fn sign_in(
    name: &str,
    account: &Account,
    builtin: &BuiltinClients,
    t: Arc<ScriptTransport>,
    memory: &MemoryStore,
) -> TokenSet {
    let endpoints = Endpoints::for_account(account);
    let client_id = oauth::client_id_for(account, &source(account), builtin).unwrap();
    let device = oauth::start(t.as_ref(), &endpoints, &client_id)
        .await
        .unwrap();
    let reply = oauth::wait_for_token(
        t.as_ref(),
        &endpoints,
        &client_id,
        &device,
        |_| std::future::ready(()),
        std::future::pending(),
        &mut |_| {},
    )
    .await
    .unwrap();
    oauth::complete_sign_in(name, account, &client_id, reply, t, memory, 1_000)
        .await
        .unwrap()
        .0
}

/// GitLab answers a pending poll with 400, asks for `read_api`, and is sent
/// the OAuth token as a bearer, never as `PRIVATE-TOKEN`, which GitLab reads
/// for personal access tokens only.
#[tokio::test]
async fn a_gitlab_sign_in_asks_for_read_api_and_reads_a_400_pending_as_pending() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("/oauth/authorize_device", 200, GL_DEVICE);
    t.reply("/oauth/token", 400, r#"{"error":"authorization_pending","error_description":"The authorization request is still pending."}"#);
    t.reply("/oauth/token", 200, GL_TOKEN_1);
    t.reply("gitlab.com/api/v4/user", 200, GL_USER);
    let account = oauth_account(Provider::Gitlab, None, None);
    let memory = MemoryStore::new();
    let set = sign_in("gl", &account, &REGISTERED, t.clone(), &memory).await;
    assert_eq!(set.login.as_deref(), Some("alice"));
    assert_eq!(set.expires_at, Some(1_000 + 7_200));

    let requests = t.requests();
    assert_eq!(requests[0].url, "https://gitlab.com/oauth/authorize_device");
    assert_eq!(
        body_of(&requests[0]),
        "client_id=gl-app-0001&scope=read_api"
    );
    assert_eq!(requests[2].url, "https://gitlab.com/oauth/token");
    let user = &requests[3];
    assert_eq!(
        header(user, "authorization").as_deref(),
        Some("Bearer gl-ACCESS-0001")
    );
    assert!(
        header(user, "private-token").is_none(),
        "{:?}",
        user.headers
    );
}

#[tokio::test]
async fn an_expired_code_and_a_refusal_each_end_the_wait_in_plain_words() {
    for (body, status, expect) in [
        (r#"{"error":"expired_token"}"#, 400, "expired"),
        (r#"{"error":"access_denied"}"#, 400, "denied"),
        (r#"{"error":"access_denied"}"#, 200, "denied"),
    ] {
        let t = ScriptTransport::new();
        t.reply("/oauth/authorize_device", 200, GL_DEVICE);
        t.reply("/oauth/token", status, body);
        let account = oauth_account(Provider::Gitlab, None, Some("mine"));
        let endpoints = Endpoints::for_account(&account);
        let device = oauth::start(&t, &endpoints, "mine").await.unwrap();
        let err = oauth::wait_for_token(
            &t,
            &endpoints,
            "mine",
            &device,
            |_| std::future::ready(()),
            std::future::pending(),
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), expect, "{err}");
        match expect {
            "expired" => assert!(err.to_string().contains("start the sign-in again"), "{err}"),
            _ => assert!(err.to_string().contains("cancelled on gitlab.com"), "{err}"),
        }
    }
}

/// `expires_in` is honoured on our side too: a server that keeps saying
/// "pending" is not polled past the code's life.
#[tokio::test]
async fn the_wait_stops_when_the_code_runs_out_without_asking_again() {
    let t = ScriptTransport::new();
    t.reply(
        "/login/device/code",
        200,
        r#"{"device_code":"d","user_code":"u","verification_uri":"https://github.com/login/device","expires_in":10,"interval":5}"#,
    );
    t.reply(
        "/login/oauth/access_token",
        200,
        r#"{"error":"authorization_pending"}"#,
    );
    t.reply(
        "/login/oauth/access_token",
        200,
        r#"{"error":"authorization_pending"}"#,
    );
    t.reply("/login/oauth/access_token", 200, GH_TOKEN);
    let account = oauth_account(Provider::Github, None, Some("c"));
    let endpoints = Endpoints::for_account(&account);
    let device = oauth::start(&t, &endpoints, "c").await.unwrap();
    let err = oauth::wait_for_token(
        &t,
        &endpoints,
        "c",
        &device,
        |_| std::future::ready(()),
        std::future::pending(),
        &mut |_| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(err, OAuthError::Expired), "{err}");
    assert_eq!(t.requests().len(), 3, "the device request and two polls");
}

#[tokio::test]
async fn cancelling_stops_the_wait_before_the_next_poll() {
    let t = ScriptTransport::new();
    t.reply("/login/device/code", 200, GH_DEVICE);
    let account = oauth_account(Provider::Github, None, Some("c"));
    let endpoints = Endpoints::for_account(&account);
    let device = oauth::start(&t, &endpoints, "c").await.unwrap();
    let err = oauth::wait_for_token(
        &t,
        &endpoints,
        "c",
        &device,
        |_| std::future::pending::<()>(),
        std::future::ready(()),
        &mut |_| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(err, OAuthError::Cancelled), "{err}");
    assert_eq!(t.requests().len(), 1, "nothing was polled");
}

/// A dropped connection for one poll does not lose a sign-in the user is
/// halfway through.
#[tokio::test]
async fn one_failed_poll_is_retried_rather_than_ending_the_sign_in() {
    let t = ScriptTransport::new();
    t.reply("/login/device/code", 200, GH_DEVICE);
    t.reply("/login/oauth/access_token", 502, "Bad Gateway");
    t.reply("/login/oauth/access_token", 200, GH_TOKEN);
    let account = oauth_account(Provider::Github, None, Some("c"));
    let endpoints = Endpoints::for_account(&account);
    let device = oauth::start(&t, &endpoints, "c").await.unwrap();
    oauth::wait_for_token(
        &t,
        &endpoints,
        "c",
        &device,
        |_| std::future::ready(()),
        std::future::pending(),
        &mut |_| {},
    )
    .await
    .expect("the second poll is approved");
}

#[tokio::test]
async fn a_disabled_device_flow_says_where_to_turn_it_on() {
    let t = ScriptTransport::new();
    t.reply("/login/device/code", 200, GH_DEVICE);
    t.reply(
        "/login/oauth/access_token",
        200,
        r#"{"error":"device_flow_disabled"}"#,
    );
    let account = oauth_account(Provider::Github, None, Some("c"));
    let endpoints = Endpoints::for_account(&account);
    let device = oauth::start(&t, &endpoints, "c").await.unwrap();
    let err = oauth::wait_for_token(
        &t,
        &endpoints,
        "c",
        &device,
        |_| std::future::ready(()),
        std::future::pending(),
        &mut |_| {},
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Enable Device Flow"), "{err}");
}

/// The verification page is opened in the user's browser, so a server naming
/// a page on any other host is refused before anything can open it.
#[tokio::test]
async fn a_verification_page_on_another_host_is_refused() {
    let t = ScriptTransport::new();
    t.reply(
        "/login/device/code",
        200,
        r#"{"device_code":"d","user_code":"u","verification_uri":"https://github.com.evil.example/login/device","expires_in":900,"interval":5}"#,
    );
    let account = oauth_account(Provider::Github, None, Some("c"));
    let err = oauth::start(&t, &Endpoints::for_account(&account), "c")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not opened"), "{err}");
}

// ---------------------------------------------------------------------------
// Hosts and client ids
// ---------------------------------------------------------------------------

#[test]
fn enterprise_and_self_managed_sign_in_on_their_own_hosts() {
    let ghes = oauth_account(Provider::Github, Some("https://ghe.acme.com"), Some("c"));
    let e = Endpoints::for_account(&ghes);
    assert_eq!(e.device_code_url, "https://ghe.acme.com/login/device/code");
    assert_eq!(e.token_url, "https://ghe.acme.com/login/oauth/access_token");
    assert_eq!(e.host, "ghe.acme.com");

    let gl = oauth_account(
        Provider::Gitlab,
        Some("https://git.example.com/gitlab/"),
        Some("c"),
    );
    let e = Endpoints::for_account(&gl);
    assert_eq!(
        e.device_code_url,
        "https://git.example.com/gitlab/oauth/authorize_device"
    );
    assert_eq!(e.token_url, "https://git.example.com/gitlab/oauth/token");
}

/// GitHub Enterprise Server and self-managed GitLab never borrow the built-in
/// application, and validation says so as an error naming what to write.
#[test]
fn enterprise_and_self_managed_need_a_client_id_of_their_own() {
    for (provider, base) in [
        (Provider::Github, "https://ghe.acme.com"),
        (Provider::Gitlab, "https://gitlab.example.com"),
    ] {
        let account = oauth_account(provider, Some(base), None);
        let err = oauth::client_id_for(&account, &source(&account), &REGISTERED).unwrap_err();
        assert!(matches!(err, OAuthError::NoClientId(_)), "{err}");
        assert!(
            err.to_string().contains("that server's own application"),
            "{err}"
        );
        assert!(err.to_string().contains("client_id"), "{err}");

        let mine = oauth_account(provider, Some(base), Some("my-app"));
        assert_eq!(
            oauth::client_id_for(&mine, &source(&mine), &REGISTERED).unwrap(),
            "my-app"
        );
    }

    let raw = r#"
        [accounts.ghe]
        provider = "github"
        base_url = "https://ghe.acme.com"
        api_path = "/api/v3"
        token = { oauth = true }

        [accounts.lab]
        base_url = "https://gitlab.example.com"
        token = { oauth = { client_id = "my-app" } }
    "#;
    let config: bridgewatch_core::Config = toml::from_str(raw).unwrap();
    let errors: Vec<_> = config::validate(&config)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].path, "accounts.ghe.token");
    assert!(
        errors[0].message.contains("that server's own application"),
        "{errors:?}"
    );
}

/// The shipped build has no applications registered. Until it does,
/// github.com and gitlab.com need a typed client id too, and say it is THIS
/// BUILD that lacks one.
#[test]
fn the_shipped_build_signs_in_with_the_registered_applications() {
    // Registered 2026-09-19: the GitHub App `bridgewatch-ci` owned by
    // distronode-corporation (Actions: read, Metadata: read, device flow on,
    // expiring user tokens, installed on the org), and the distronode-corporation
    // group application on gitlab.com (not confidential, device authorization
    // grant on, read_api). Client ids are public identifiers, not secrets.
    let shipped = BuiltinClients::shipped();
    assert_eq!(shipped.github_com, Some("Iv23linCrkhVaN0BiYBU"));
    assert_eq!(shipped.github_app_slug, Some("bridgewatch-ci"));
    assert_eq!(
        shipped.gitlab_com,
        Some("34359b8c0970a5166211c80fe800514c8753dafd81da90f298026ba28e5a4d5a")
    );
    for provider in [Provider::Github, Provider::Gitlab] {
        let account = oauth_account(provider, None, None);
        let on = oauth::availability(provider, &account.base_url, None, &shipped);
        assert!(on.available && on.builtin, "{on:?}");
        assert!(oauth::client_id_for(&account, &source(&account), &shipped).is_ok());
    }
}

#[test]
fn a_registered_build_offers_sign_in_on_the_hosted_instances_only() {
    let gh = oauth::availability(
        Provider::Github,
        "https://api.github.com",
        None,
        &REGISTERED,
    );
    assert!(gh.available && gh.builtin, "{gh:?}");
    assert_eq!(gh.host, "github.com");
    assert_eq!(
        gh.install_url.as_deref(),
        Some("https://github.com/apps/bridgewatch-ci/installations/new")
    );
    let gl = oauth::availability(Provider::Gitlab, "https://gitlab.com/", None, &REGISTERED);
    assert!(
        gl.available && gl.builtin && gl.install_url.is_none(),
        "{gl:?}"
    );

    let ghes = oauth::availability(Provider::Github, "https://ghe.acme.com", None, &REGISTERED);
    assert!(
        !ghes.available && ghes.needs_client_id && ghes.install_url.is_none(),
        "{ghes:?}"
    );
    let self_managed = oauth::availability(
        Provider::Gitlab,
        "https://gitlab.example.com",
        None,
        &REGISTERED,
    );
    assert!(
        !self_managed.available && self_managed.needs_client_id,
        "{self_managed:?}"
    );
    // A look-alike is not github.com.
    let fake = oauth::availability(
        Provider::Github,
        "https://api.github.com.evil.example",
        None,
        &REGISTERED,
    );
    assert!(!fake.builtin, "{fake:?}");
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[test]
fn the_oauth_source_is_written_as_true_or_a_client_id() {
    let parse = |t: &str| -> Result<TokenSource, String> {
        let raw = format!("[accounts.a]\ntoken = {t}\n");
        toml::from_str::<bridgewatch_core::Config>(&raw)
            .map(|c| c.accounts["a"].token.clone())
            .map_err(|e| e.to_string())
    };
    assert_eq!(
        parse("{ oauth = true }").unwrap(),
        TokenSource::Oauth(OAuthSource::default())
    );
    assert_eq!(
        parse("{ oauth = {} }").unwrap(),
        TokenSource::Oauth(OAuthSource::default())
    );
    assert_eq!(
        parse(r#"{ oauth = { client_id = "abc" } }"#).unwrap(),
        TokenSource::Oauth(OAuthSource {
            client_id: Some("abc".into())
        })
    );
    assert!(
        parse("{ oauth = false }")
            .unwrap_err()
            .contains("oauth = false selects no source")
    );
    assert!(parse(r#"{ oauth = { client_secret = "x" } }"#).is_err());
    assert!(
        parse(r#"{ env = "T", oauth = true }"#)
            .unwrap_err()
            .contains("names exactly one of keyring, env, command, own or oauth")
    );

    // And it is written back the way it was read.
    let mut config = bridgewatch_core::Config::default();
    let mut account = Account {
        token: TokenSource::Oauth(OAuthSource::default()),
        ..Account::default()
    };
    config.accounts.insert("a".into(), account.clone());
    assert!(toml::to_string(&config).unwrap().contains("oauth = true"));
    account.token = TokenSource::Oauth(OAuthSource {
        client_id: Some("abc".into()),
    });
    config.accounts.insert("a".into(), account);
    let text = toml::to_string(&config).unwrap();
    assert!(text.contains("client_id = \"abc\""), "{text}");
}

#[test]
fn a_wizard_that_signs_in_writes_only_where_the_sign_in_comes_from() {
    let answers = bridgewatch_core::wizard::WizardAnswers {
        token: TokenSource::Oauth(OAuthSource {
            client_id: Some("gl-app-self".into()),
        }),
        base_url: "https://gitlab.example.com".into(),
        project: Some(ProjectRef::Id(42)),
        ..Default::default()
    };
    let built = bridgewatch_core::wizard::build_config(&answers, None).expect("builds");
    assert!(
        built
            .toml
            .contains("token = { oauth = { client_id = \"gl-app-self\" } }"),
        "{}",
        built.toml
    );

    // Without a client id, this build signs in with the registered gitlab.com
    // application, so the account step has nothing to object to.
    let answers = bridgewatch_core::wizard::WizardAnswers {
        token: TokenSource::Oauth(OAuthSource::default()),
        project: Some(ProjectRef::Id(42)),
        ..Default::default()
    };
    let issues = bridgewatch_core::wizard::validate_answers(&answers);
    assert!(!issues.iter().any(|i| i.field == "token"), "{issues:?}");
}

#[test]
fn resolving_an_oauth_source_reads_the_stored_access_token_or_says_to_sign_in() {
    let memory = MemoryStore::new();
    let source = TokenSource::Oauth(OAuthSource::default());
    let err = token::resolve(&source, "gl", memory.as_ref()).unwrap_err();
    assert!(matches!(err, TokenError::NotSignedIn(_)), "{err}");
    assert!(
        err.to_string()
            .contains("bridgewatch auth login --account gl"),
        "{err}"
    );

    store::save(
        "gl",
        &set("gl-ACCESS-0001", Some("r"), Some(10), "https://gitlab.com"),
        memory.as_ref(),
    )
    .unwrap();
    assert_eq!(
        token::resolve(&source, "gl", memory.as_ref())
            .unwrap()
            .expose(),
        "gl-ACCESS-0001"
    );
}

// ---------------------------------------------------------------------------
// The live credential: refresh ahead of expiry, on a 401, and GitLab rotation
// ---------------------------------------------------------------------------

fn set(access: &str, refresh: Option<&str>, expires_at: Option<u64>, base: &str) -> TokenSet {
    TokenSet {
        access_token: Secret::new(access),
        refresh_token: refresh.map(Secret::new),
        expires_at,
        refresh_expires_at: None,
        scopes: vec!["read_api".into()],
        login: Some("alice".into()),
        client_id: "gl-app-0001".into(),
        base_url: base.into(),
        obtained_at: 1_000,
    }
}

struct Clock(Arc<AtomicU64>);

impl Clock {
    fn at(now: u64) -> Self {
        Self(Arc::new(AtomicU64::new(now)))
    }
    fn clock(&self) -> oauth::Clock {
        let now = self.0.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    }
    fn set(&self, now: u64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

fn gitlab_session(
    t: Arc<dyn Transport>,
    memory: &Arc<MemoryStore>,
    clock: &Clock,
) -> (Arc<OAuthSession>, Account) {
    let account = oauth_account(Provider::Gitlab, None, None);
    let session = OAuthSession::new(
        "gl",
        &account,
        &source(&account),
        &REGISTERED,
        t,
        memory.clone(),
    )
    .unwrap()
    .with_clock(clock.clock());
    (Arc::new(session), account)
}

/// A client exactly as the poller builds one for an account that signs in.
fn client_over(
    account: &Account,
    inner: Arc<dyn Transport>,
    session: Arc<OAuthSession>,
) -> Arc<dyn CiClient> {
    client_for(
        &oauth::bearer_account(account),
        &Secret::new(""),
        Arc::new(OAuthTransport::new(inner, session)),
        RequestRing::new(8),
    )
    .unwrap()
}

fn stored(memory: &MemoryStore, account: &str) -> TokenSet {
    store::load(account, memory)
        .unwrap()
        .expect("a stored sign-in")
}

#[tokio::test]
async fn a_token_about_to_expire_is_refreshed_before_the_request_that_would_send_it() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(10_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/oauth/token", 200, GL_TOKEN_2);
    t.reply("/api/v4/user", 200, GL_USER);
    let clock = Clock::at(10_000 - 100);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    let client = client_over(&account, t.clone(), session);

    client
        .current_user()
        .await
        .expect("answered with the new token");
    let requests = t.requests();
    assert_eq!(requests.len(), 2, "one refresh, one request");
    let refresh = body_of(&requests[0]);
    assert_eq!(
        refresh,
        "client_id=gl-app-0001&grant_type=refresh_token&refresh_token=gl-REFRESH-0001"
    );
    assert!(!refresh.contains("client_secret"), "{refresh}");
    assert_eq!(
        header(&requests[1], "authorization").as_deref(),
        Some("Bearer gl-ACCESS-0002")
    );

    let now = stored(&memory, "gl");
    assert_eq!(now.access_token.expose(), "gl-ACCESS-0002");
    assert_eq!(
        now.refresh_token.as_ref().map(Secret::expose),
        Some("gl-REFRESH-0002")
    );
    assert_eq!(now.expires_at, Some(10_000 - 100 + 7_200));
    assert_eq!(now.login.as_deref(), Some("alice"), "who signed in is kept");
}

#[tokio::test]
async fn a_token_with_time_left_is_sent_as_it_is() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(10_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/api/v4/user", 200, GL_USER);
    let clock = Clock::at(1_000);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    client_over(&account, t.clone(), session)
        .current_user()
        .await
        .unwrap();
    assert_eq!(t.requests().len(), 1);
    assert_eq!(
        header(&t.requests()[0], "authorization").as_deref(),
        Some("Bearer gl-ACCESS-0001")
    );
}

#[tokio::test]
async fn a_401_refreshes_once_and_retries_once() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(10_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/api/v4/user", 401, r#"{"message":"401 Unauthorized"}"#);
    t.reply("/oauth/token", 200, GL_TOKEN_2);
    t.reply("/api/v4/user", 200, GL_USER);
    // Well inside the token's life, but long after it was obtained.
    let clock = Clock::at(5_000);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    let user = client_over(&account, t.clone(), session)
        .current_user()
        .await
        .unwrap();
    assert_eq!(user.username, "alice");
    let urls: Vec<String> = t.requests().iter().map(|r| r.url.clone()).collect();
    assert_eq!(
        urls,
        [
            "https://gitlab.com/api/v4/user",
            "https://gitlab.com/oauth/token",
            "https://gitlab.com/api/v4/user"
        ]
    );
}

/// Once, not in a loop: a token the server still refuses after a successful
/// refresh is the server's answer.
#[tokio::test]
async fn a_second_401_is_the_answer_and_is_not_refreshed_again() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(10_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/api/v4/user", 401, "{}");
    t.reply("/oauth/token", 200, GL_TOKEN_2);
    t.reply("/api/v4/user", 401, "{}");
    let clock = Clock::at(5_000);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    let err = client_over(&account, t.clone(), session)
        .current_user()
        .await
        .unwrap_err();
    assert!(
        matches!(err, ClientError::Auth { status: 401, .. }),
        "{err}"
    );
    assert_eq!(
        t.requests()
            .iter()
            .filter(|r| r.url.ends_with("/oauth/token"))
            .count(),
        1
    );
}

/// GitLab's `/personal_access_tokens/self` refuses every OAuth token. On a
/// token obtained moments ago that 401 is the endpoint's answer, not an
/// expiry, and refreshing on it would spend a rotating refresh token for
/// nothing each time "Test connection" is pressed.
#[tokio::test]
async fn a_401_on_a_token_obtained_moments_ago_is_passed_through_without_a_refresh() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(10_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/personal_access_tokens/self", 401, "{}");
    let clock = Clock::at(1_000 + 30);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    let err = client_over(&account, t.clone(), session)
        .token_self()
        .await
        .unwrap_err();
    assert!(
        matches!(err, ClientError::Auth { status: 401, .. }),
        "{err}"
    );
    assert_eq!(t.requests().len(), 1, "no refresh, no retry");
    assert_eq!(
        stored(&memory, "gl").access_token.expose(),
        "gl-ACCESS-0001"
    );
}

/// Watches the store from inside the transport: when a request carrying a
/// token goes out, is that token already written down?
#[derive(Debug)]
struct StoreAtSend {
    inner: Arc<ScriptTransport>,
    memory: Arc<MemoryStore>,
    stored_when_sent: Mutex<Vec<(String, Option<String>)>>,
}

#[async_trait::async_trait]
impl Transport for StoreAtSend {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        if request.url.contains("/api/v4/") {
            let sent = header(&request, "authorization").unwrap_or_default();
            let stored = store::load("gl", self.memory.as_ref())
                .unwrap()
                .map(|s| s.access_token.expose().to_string());
            self.stored_when_sent.lock().unwrap().push((sent, stored));
        }
        self.inner.execute(request).await
    }
}

/// ⛔ GitLab's refresh spends the old pair the moment it answers. So the new
/// pair must be in the credential store BEFORE the first request that uses it,
/// or a crash between the two would leave a store holding tokens that no
/// longer work.
#[tokio::test]
async fn a_rotated_gitlab_pair_is_stored_before_its_first_use() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(5_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let script = Arc::new(ScriptTransport::new());
    script.reply("/oauth/token", 200, GL_TOKEN_2);
    script.reply("/api/v4/user", 200, GL_USER);
    let watching = Arc::new(StoreAtSend {
        inner: script.clone(),
        memory: memory.clone(),
        stored_when_sent: Mutex::new(Vec::new()),
    });
    let clock = Clock::at(5_000);
    let (session, account) = gitlab_session(watching.clone(), &memory, &clock);
    client_over(&account, watching.clone(), session)
        .current_user()
        .await
        .unwrap();

    let seen = watching.stored_when_sent.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![(
            "Bearer gl-ACCESS-0002".to_string(),
            Some("gl-ACCESS-0002".to_string())
        )],
        "the request carried the new token, and the store already held it"
    );
    let now = stored(&memory, "gl");
    assert_eq!(
        now.refresh_token.as_ref().map(Secret::expose),
        Some("gl-REFRESH-0002"),
        "and the new refresh token with it, in the same write"
    );

    // A restart reads exactly that, and carries on without a refresh.
    let t = Arc::new(ScriptTransport::new());
    t.reply("/api/v4/user", 200, GL_USER);
    let (restarted, account) = gitlab_session(t.clone(), &memory, &clock);
    client_over(&account, t.clone(), restarted)
        .current_user()
        .await
        .unwrap();
    assert_eq!(
        header(&t.requests()[0], "authorization").as_deref(),
        Some("Bearer gl-ACCESS-0002")
    );
}

/// When the store refuses the write, the new pair is still used (the old one
/// is already spent), the write is retried, and a restart before it lands
/// ends in "sign in again" rather than a loop.
#[tokio::test]
async fn a_refreshed_pair_the_store_refused_is_used_and_saved_as_soon_as_it_can_be() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(5_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    memory.refuse_writes(true);
    let t = Arc::new(ScriptTransport::new());
    t.reply("/oauth/token", 200, GL_TOKEN_2);
    t.reply("/api/v4/user", 200, GL_USER);
    t.reply("/api/v4/user", 200, GL_USER);
    let clock = Clock::at(5_000);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    let client = client_over(&account, t.clone(), session);
    client
        .current_user()
        .await
        .expect("the new pair works in memory");
    assert_eq!(
        stored(&memory, "gl").access_token.expose(),
        "gl-ACCESS-0001",
        "the write failed"
    );

    // A restart NOW would hold the spent pair: its refresh is refused, and that
    // is "sign in again", said once.
    let spent = Arc::new(ScriptTransport::new());
    spent.reply("/oauth/token", 400, r#"{"error":"invalid_grant","error_description":"The provided authorization grant is invalid"}"#);
    let (restarted, account_b) = gitlab_session(spent.clone(), &memory, &clock);
    let err = client_over(&account_b, spent.clone(), restarted.clone())
        .current_user()
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::SignInAgain {
                never_signed_in: false,
                ..
            }
        ),
        "{err}"
    );

    // The running session saves as soon as the store takes it.
    memory.refuse_writes(false);
    client.current_user().await.unwrap();
    assert_eq!(
        stored(&memory, "gl").access_token.expose(),
        "gl-ACCESS-0002"
    );
    assert_eq!(
        stored(&memory, "gl")
            .refresh_token
            .as_ref()
            .map(Secret::expose),
        Some("gl-REFRESH-0002")
    );
}

#[tokio::test]
async fn a_refused_refresh_says_sign_in_again_once_and_stops_asking() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(5_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/oauth/token", 400, r#"{"error":"invalid_grant"}"#);
    let clock = Clock::at(6_000);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    let client = client_over(&account, t.clone(), session);

    let err = client.current_user().await.unwrap_err();
    assert!(err.is_fatal(), "the poller does not retry it at pace");
    let text = err.to_string();
    assert!(
        text.contains("GitLab sign-in of account \"gl\" has expired or was revoked"),
        "{text}"
    );
    assert!(
        text.contains("bridgewatch auth login --account gl"),
        "{text}"
    );
    assert!(text.contains("Settings"), "{text}");

    for _ in 0..3 {
        assert!(matches!(
            client.current_user().await.unwrap_err(),
            ClientError::SignInAgain { .. }
        ));
    }
    assert_eq!(
        t.requests().len(),
        1,
        "no second refresh, and nothing sent with a dead token"
    );

    // Signing in again (anywhere) is picked up on the next request.
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0009",
            Some("gl-REFRESH-0009"),
            Some(20_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    t.reply("/api/v4/user", 200, GL_USER);
    client
        .current_user()
        .await
        .expect("the new sign-in is used");
}

#[tokio::test]
async fn github_refreshes_without_a_secret_and_reads_bad_refresh_token_as_sign_in_again() {
    let memory = MemoryStore::new();
    let account = oauth_account(Provider::Github, None, None);
    let mut expired = set(
        "ghu_OLD",
        Some("ghr_OLD"),
        Some(1_000),
        "https://api.github.com",
    );
    expired.client_id = "Iv23liBRIDGEWATCH".into();
    store::save("gh", &expired, memory.as_ref()).unwrap();
    let t = Arc::new(ScriptTransport::new());
    // GitHub reports a refused refresh with 200 and an error body.
    t.reply(
        "/login/oauth/access_token",
        200,
        r#"{"error":"bad_refresh_token"}"#,
    );
    let session = OAuthSession::new(
        "gh",
        &account,
        &source(&account),
        &REGISTERED,
        t.clone(),
        memory.clone(),
    )
    .unwrap()
    .with_clock(Clock::at(2_000).clock());
    let err = client_over(&account, t.clone(), Arc::new(session))
        .current_user()
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("GitHub sign-in of account \"gh\" has expired"),
        "{err}"
    );
    let body = body_of(&t.requests()[0]);
    assert_eq!(
        body,
        "client_id=Iv23liBRIDGEWATCH&grant_type=refresh_token&refresh_token=ghr_OLD"
    );
}

#[tokio::test]
async fn an_account_never_signed_in_sends_nothing_and_says_how_to_sign_in() {
    let memory = MemoryStore::new();
    let t = Arc::new(ScriptTransport::new());
    let clock = Clock::at(1_000);
    let (session, account) = gitlab_session(t.clone(), &memory, &clock);
    let err = client_over(&account, t.clone(), session)
        .current_user()
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::SignInAgain {
                never_signed_in: true,
                ..
            }
        ),
        "{err}"
    );
    assert!(
        err.to_string().contains("is not signed in to GitLab"),
        "{err}"
    );
    assert!(t.requests().is_empty());
}

/// The CLI and the tray can hold the same sign-in. When one refreshes (and on
/// GitLab thereby spends the other's copy), the other adopts what was stored
/// instead of refreshing with a spent token.
#[tokio::test]
async fn a_refresh_made_by_another_process_is_adopted_rather_than_repeated() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(5_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let clock = Clock::at(1_000);

    let tray_t = Arc::new(ScriptTransport::new());
    tray_t.reply("/api/v4/user", 200, GL_USER);
    tray_t.reply("/api/v4/user", 200, GL_USER);
    let (tray, account) = gitlab_session(tray_t.clone(), &memory, &clock);
    let tray_client = client_over(&account, tray_t.clone(), tray);
    tray_client.current_user().await.unwrap();

    // The CLI refreshes as the token nears its end.
    clock.set(4_900);
    let cli_t = Arc::new(ScriptTransport::new());
    cli_t.reply("/oauth/token", 200, GL_TOKEN_2);
    cli_t.reply("/api/v4/user", 200, GL_USER);
    let (cli, account) = gitlab_session(cli_t.clone(), &memory, &clock);
    client_over(&account, cli_t.clone(), cli)
        .current_user()
        .await
        .unwrap();

    // The tray, holding the spent pair, has no refresh scripted at all.
    tray_client
        .current_user()
        .await
        .expect("adopted the CLI's refresh");
    let last = tray_t.requests().pop().unwrap();
    assert_eq!(
        header(&last, "authorization").as_deref(),
        Some("Bearer gl-ACCESS-0002")
    );
    assert!(
        tray_t
            .requests()
            .iter()
            .all(|r| !r.url.ends_with("/oauth/token"))
    );
}

/// ⛔ A stored sign-in is never sent to another host: `base_url` can change
/// under it, and the token would follow.
#[tokio::test]
async fn a_sign_in_stored_for_another_host_is_never_sent() {
    let memory = MemoryStore::new();
    store::save(
        "gl",
        &set(
            "gl-ACCESS-0001",
            Some("gl-REFRESH-0001"),
            Some(99_000),
            "https://gitlab.com",
        ),
        memory.as_ref(),
    )
    .unwrap();
    let t = Arc::new(ScriptTransport::new());
    let account = oauth_account(
        Provider::Gitlab,
        Some("https://gitlab.evil.example"),
        Some("gl-app-0001"),
    );
    let session = OAuthSession::new(
        "gl",
        &account,
        &source(&account),
        &REGISTERED,
        t.clone(),
        memory.clone(),
    )
    .unwrap();
    let err = client_over(&account, t.clone(), Arc::new(session))
        .current_user()
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::SignInAgain { .. }), "{err}");
    assert!(t.requests().is_empty(), "{:?}", t.requests());
}

// ---------------------------------------------------------------------------
// The poller
// ---------------------------------------------------------------------------

fn github_oauth_config() -> bridgewatch_core::Config {
    toml::from_str(
        r#"
        [accounts.gh]
        provider = "github"
        token = { oauth = { client_id = "Iv23liMINE" } }

        [[watches]]
        id = "ci"
        account = "gh"
        project = "acme-corp/monorepo"
        deploy_markers = ["publish"]
        "#,
    )
    .unwrap()
}

fn poller_over(
    config: &bridgewatch_core::Config,
    t: Arc<ScriptTransport>,
    memory: &Arc<MemoryStore>,
) -> bridgewatch_core::poll::Poller {
    let ring = RequestRing::new(8);
    let mut clients: std::collections::BTreeMap<String, Arc<dyn CiClient>> = Default::default();
    for (name, account) in &config.accounts {
        let TokenSource::Oauth(source) = &account.token else {
            unreachable!()
        };
        let transport = oauth::transport_for(
            name,
            account,
            source,
            &REGISTERED,
            t.clone(),
            memory.clone(),
        )
        .unwrap();
        clients.insert(
            name.clone(),
            client_for(
                &oauth::bearer_account(account),
                &Secret::new(""),
                transport,
                ring.clone(),
            )
            .unwrap(),
        );
    }
    bridgewatch_core::poll::Poller::with_clients(config, clients, ring).unwrap()
}

#[tokio::test]
async fn a_watch_on_an_account_that_is_not_signed_in_says_so_in_the_popover() {
    let config = github_oauth_config();
    let t = Arc::new(ScriptTransport::new());
    let mut poller = poller_over(&config, t.clone(), &MemoryStore::new());
    let tick = poller.tick().await;
    let error = tick.snapshot.watches[0]
        .error
        .clone()
        .expect("an error on the watch");
    assert!(
        error.contains("account \"gh\" is not signed in to GitHub"),
        "{error}"
    );
    assert!(t.requests().is_empty());
}

/// A GitHub App's user token sees a repository only where the app is
/// installed, so a 404 on a signing-in GitHub account says to install it.
#[tokio::test]
async fn a_404_on_a_signed_in_github_account_says_to_install_the_app() {
    let config = github_oauth_config();
    let memory = MemoryStore::new();
    let mut signed = set(
        "ghu_A",
        Some("ghr_A"),
        Some(u64::MAX / 2),
        "https://api.github.com",
    );
    signed.client_id = "Iv23liMINE".into();
    store::save("gh", &signed, memory.as_ref()).unwrap();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/actions/runs", 404, r#"{"message":"Not Found"}"#);
    let mut poller = poller_over(&config, t.clone(), &memory);
    let tick = poller.tick().await;
    let error = tick.snapshot.watches[0]
        .error
        .clone()
        .expect("an error on the watch");
    assert!(error.starts_with("not found:"), "{error}");
    // The registered app's slug turns the hint into a link.
    assert!(
        error.contains("install it on the owner of this repository")
            && error.contains("https://github.com/apps/bridgewatch-ci/installations/new"),
        "{error}"
    );

    // With a registered app, the install page is named.
    let account = &config.accounts["gh"];
    let mut hosted = account.clone();
    hosted.token = TokenSource::Oauth(OAuthSource::default());
    let hint = oauth::not_found_hint(&hosted, &REGISTERED).unwrap();
    assert!(
        hint.contains("https://github.com/apps/bridgewatch-ci/installations/new"),
        "{hint}"
    );
    // And a GitLab account, or a token of any other source, gains nothing.
    assert!(oauth::not_found_hint(&Account::default(), &REGISTERED).is_none());
    let mut pat = hosted.clone();
    pat.token = TokenSource::Env("GH".into());
    assert!(oauth::not_found_hint(&pat, &REGISTERED).is_none());
}

// ---------------------------------------------------------------------------
// Nothing secret in a rendering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_debug_or_error_rendering_carries_a_code_or_a_token() {
    let t = ScriptTransport::new();
    t.reply("/login/device/code", 200, GH_DEVICE);
    let account = oauth_account(Provider::Github, None, Some("c"));
    let device = oauth::start(&t, &Endpoints::for_account(&account), "c")
        .await
        .unwrap();
    let the_set = set(
        "ghu_ACCESS0001",
        Some("ghr_REFRESH0001"),
        Some(1),
        "https://api.github.com",
    );
    let request = t.requests().pop().unwrap();
    let rendered = format!(
        "{device:?} {device:#?} {the_set:?} {the_set:#?} {request:?} {:?}",
        the_set.status()
    );
    for secret in [
        "gh-DEVICE-CODE-0001",
        "WDJB-MJHT",
        "ghu_ACCESS0001",
        "ghr_REFRESH0001",
    ] {
        assert!(!rendered.contains(secret), "{secret} in {rendered}");
    }
    assert!(rendered.contains("<redacted>"), "{rendered}");

    // A corrupt stored document is described by position, never quoted.
    let err = TokenSet::from_json(r#"{"version":1,"access_token":"ghu_LEAK","client_id":5}"#)
        .unwrap_err();
    assert!(!err.to_string().contains("ghu_LEAK"), "{err}");
}

/// The production transport really sends a form POST, which nothing else in
/// the tree does.
#[tokio::test]
async fn the_real_transport_posts_the_form_with_its_content_type() {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut head = String::new();
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).unwrap();
        head.push_str(&String::from_utf8_lossy(&body));
        tx.send(head).unwrap();
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
        );
    });
    let transport =
        bridgewatch_core::client::ReqwestTransport::new(Duration::from_secs(5)).unwrap();
    let response = transport
        .execute(HttpRequest {
            method: "POST",
            url: format!("{base}/login/oauth/access_token"),
            path: "/login/oauth/access_token".into(),
            headers: vec![(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            body: Some("client_id=abc&grant_type=refresh_token".into()),
        })
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    let seen = rx.recv().unwrap();
    assert!(
        seen.starts_with("POST /login/oauth/access_token HTTP/1.1"),
        "{seen}"
    );
    assert!(
        seen.to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded"),
        "{seen}"
    );
    assert!(
        seen.ends_with("client_id=abc&grant_type=refresh_token"),
        "{seen}"
    );
}
