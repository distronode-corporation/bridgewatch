//! The setup wizard's steps, against a scripted transport and a fake keyring.
//!
//! Nothing here touches a network, a real credential store or a file: the
//! transport answers by path, the keyring probe is a fake that can only say
//! "exists" or "does not", and `build_config` returns text.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bridgewatch_core::client::http::{HttpRequest, HttpResponse, Transport};
use bridgewatch_core::client::{
    ClientError, FixtureTransport, GitHubClient, GitLabClient, RequestRing,
};
use bridgewatch_core::config::{self, Account, ProjectRef, Provider, Role, TokenSource};
use bridgewatch_core::model::{Job, User};
use bridgewatch_core::token::Secret;
use bridgewatch_core::wizard::{
    self, CliTokenDetection, KeyringProbe, MarkerScope, NotifyAnswers, ProjectListing, TokenKind,
    WizardAnswers, WizardError, WizardStep,
};

const TOKEN: &str = "glpat-WIZARDSECRET";

// ---------------------------------------------------------------------------
// A transport that answers by path
// ---------------------------------------------------------------------------

/// Answers each request from the first route whose key the path starts with.
/// A route may hold several pages; each request to it takes the next one and
/// the last repeats. Unrouted paths are 404.
/// One scripted answer: status, body, `x-next-page`.
type Page = (u16, String, Option<String>);

#[derive(Debug, Default)]
struct Routed {
    routes: Mutex<Vec<(String, Vec<Page>)>>,
    seen: Mutex<Vec<String>>,
    counts: Mutex<HashMap<String, usize>>,
    /// `X-OAuth-Scopes`, which is the ONLY thing GitHub says about a token and
    /// which a fine-grained token does not get at all. `None` is that case.
    oauth_scopes: Mutex<Option<String>>,
}

impl Routed {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn route(self: &Arc<Self>, prefix: &str, status: u16, body: &str) -> Arc<Self> {
        self.pages(prefix, vec![(status, body, None)])
    }
    fn pages(self: &Arc<Self>, prefix: &str, pages: Vec<(u16, &str, Option<&str>)>) -> Arc<Self> {
        self.routes.lock().unwrap().push((
            prefix.to_string(),
            pages
                .into_iter()
                .map(|(s, b, n)| (s, b.to_string(), n.map(str::to_string)))
                .collect(),
        ));
        self.clone()
    }
    fn oauth_scopes(self: &Arc<Self>, raw: &str) -> Arc<Self> {
        *self.oauth_scopes.lock().unwrap() = Some(raw.to_string());
        self.clone()
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Transport for Routed {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        self.seen.lock().unwrap().push(request.path.clone());
        let routes = self.routes.lock().unwrap();
        let hit = routes
            .iter()
            .find(|(prefix, _)| request.path.starts_with(prefix.as_str()));
        let (status, body, next_page) = match hit {
            None => (404, "{\"message\":\"404 Not Found\"}".to_string(), None),
            Some((prefix, pages)) => {
                let mut counts = self.counts.lock().unwrap();
                let n = counts.entry(prefix.clone()).or_insert(0);
                let page = pages[(*n).min(pages.len() - 1)].clone();
                *n += 1;
                page
            }
        };
        Ok(HttpResponse {
            status,
            body,
            next_page,
            ratelimit_remaining: None,
            ratelimit_reset: None,
            retry_after: None,
            etag: None,
            link: None,
            oauth_scopes: self.oauth_scopes.lock().unwrap().clone(),
        })
    }
}

fn client(transport: Arc<dyn Transport>) -> GitLabClient {
    GitLabClient::new(
        &Account::default(),
        &Secret::new(TOKEN),
        transport,
        RequestRing::new(20),
    )
}

/// ⛔ `Account::for_provider`, not `Account::default()`: the plain default is a
/// GitHub account pointed at gitlab.com with `/api/v4` and `PRIVATE-TOKEN`, and
/// the paths this scripted transport is routed on would then be GitLab's.
fn gh_client(transport: Arc<dyn Transport>) -> GitHubClient {
    GitHubClient::new(
        &Account::for_provider(Provider::Github),
        &Secret::new(GH_TOKEN),
        transport,
        RequestRing::new(20),
    )
}

const PERSONAL_USER: &str =
    r#"{"id":1,"username":"sean","name":"Sean","bot":false,"email":"x@example.invalid"}"#;
const READ_API_TOKEN: &str = r#"{"id":9,"name":"bridgewatch","scopes":["read_api"],"expires_at":"2027-01-01","active":true}"#;

/// Invented, and shaped like a classic GitHub token only so the refusal tests
/// can prove it never reaches a message. Nothing reads a real credential here.
const GH_TOKEN: &str = "ghp_WIZARDSECRET";
const GH_USER: &str = r#"{"id":11,"login":"octo","name":"Octo Cat","type":"User"}"#;

// ---------------------------------------------------------------------------
// test_connection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_personal_token_names_its_user_and_has_nothing_to_warn_about() {
    let t = Routed::new().route("/user", 200, PERSONAL_USER).route(
        "/personal_access_tokens/self",
        200,
        READ_API_TOKEN,
    );
    let id = wizard::test_connection(&client(t.clone()), Provider::Gitlab)
        .await
        .unwrap();

    assert_eq!(id.username, "sean");
    assert_eq!(id.name.as_deref(), Some("Sean"));
    assert!(!id.bot);
    assert_eq!(id.token, TokenKind::Personal);
    assert_eq!(id.scopes, ["read_api"]);
    assert_eq!(id.expires_at.as_deref(), Some("2027-01-01"));
    assert!(id.warnings.is_empty(), "{:?}", id.warnings);
    assert!(id.can_list_projects());
    assert_eq!(t.seen(), ["/user", "/personal_access_tokens/self"]);
}

/// A project access token authenticates as `project_<id>_bot_<hex>`. The
/// wizard names the kind, reads the project id out of the name, and warns that
/// the project list is not available.
#[tokio::test]
async fn a_project_token_is_named_and_warned_about() {
    let t = Routed::new()
        .route(
            "/user",
            200,
            r#"{"id":5,"username":"project_82468124_bot_3f2a","name":"bridgewatch","bot":true}"#,
        )
        .route("/personal_access_tokens/self", 200, READ_API_TOKEN);
    let id = wizard::test_connection(&client(t), Provider::Gitlab)
        .await
        .unwrap();

    assert!(id.bot);
    assert_eq!(
        id.token,
        TokenKind::Project {
            project_id: Some(82468124)
        }
    );
    assert!(!id.can_list_projects());
    assert_eq!(id.warnings.len(), 1, "{:?}", id.warnings);
    assert!(
        id.warnings[0].contains("project access token") && id.warnings[0].contains("82468124"),
        "{}",
        id.warnings[0]
    );
}

#[test]
fn token_kinds_are_read_from_the_bot_user_name() {
    let user = |name: &str, bot: bool| User {
        id: 1,
        username: name.into(),
        name: None,
        bot,
    };
    use wizard::classify_token;
    assert_eq!(
        classify_token(&user("group_77_bot_ab12", true), true),
        TokenKind::Group { group_id: Some(77) }
    );
    assert_eq!(
        classify_token(&user("project_12_bot", true), true),
        TokenKind::Project {
            project_id: Some(12)
        }
    );
    assert_eq!(
        classify_token(&user("service_account_group_1_abc", true), true),
        TokenKind::ServiceAccount
    );
    // A human named project_1_botany is not a bot: the flag decides.
    assert_eq!(
        classify_token(&user("project_1_botany", false), true),
        TokenKind::Personal
    );
    assert_eq!(
        classify_token(&user("sean", false), false),
        TokenKind::Unknown,
        "an OAuth or job token: /personal_access_tokens/self did not know it"
    );
}

/// A token without `read_api` loads `/user` fine and then 403s every pipeline
/// request, so the wizard says so before the user finishes.
#[tokio::test]
async fn a_token_without_read_api_is_warned_about() {
    let t = Routed::new().route("/user", 200, PERSONAL_USER).route(
        "/personal_access_tokens/self",
        200,
        r#"{"id":9,"scopes":["read_user"],"active":true}"#,
    );
    let id = wizard::test_connection(&client(t), Provider::Gitlab)
        .await
        .unwrap();
    assert_eq!(id.warnings.len(), 1);
    assert!(id.warnings[0].contains("read_api"), "{}", id.warnings[0]);
}

/// `/personal_access_tokens/self` is best effort: an instance without it still
/// answers who the user is.
#[tokio::test]
async fn an_instance_without_the_token_endpoint_still_connects() {
    let t = Routed::new().route("/user", 200, PERSONAL_USER);
    let id = wizard::test_connection(&client(t), Provider::Gitlab)
        .await
        .unwrap();
    assert_eq!(id.token, TokenKind::Unknown);
    assert!(id.scopes.is_empty());
    assert!(
        id.warnings.is_empty(),
        "no scope warning without scopes to read"
    );
}

/// 401 and 403 are the token's fault and say so; neither message carries the
/// token.
#[tokio::test]
async fn a_refused_token_is_an_unauthorized_error_without_the_token_in_it() {
    for status in [401u16, 403] {
        let t = Routed::new().route("/user", status, "{\"message\":\"401 Unauthorized\"}");
        let err = wizard::test_connection(&client(t), Provider::Gitlab)
            .await
            .unwrap_err();
        assert!(
            matches!(err, WizardError::Unauthorized { status: s, .. } if s == status),
            "{err:?}"
        );
        let text = format!("{err} {err:?}");
        assert!(text.contains("read_api"), "{text}");
        assert!(!text.contains(TOKEN), "{text}");
    }
}

// ---------------------------------------------------------------------------
// list_projects
// ---------------------------------------------------------------------------

#[tokio::test]
async fn projects_are_listed_across_pages_with_the_search_encoded() {
    let t = Routed::new().pages(
        "/projects?",
        vec![
            (
                200,
                r#"[{"id":1,"path_with_namespace":"g/a","name":"a","default_branch":"main"}]"#,
                Some("2"),
            ),
            (
                200,
                r#"[{"id":2,"path_with_namespace":"g/b","default_branch":null}]"#,
                None,
            ),
        ],
    );
    let listing = wizard::list_projects(&client(t.clone()), &TokenKind::Personal, Some("my app"))
        .await
        .unwrap();
    let ProjectListing::Projects {
        projects,
        truncated,
    } = listing
    else {
        panic!("expected projects, got {listing:?}");
    };
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].path, "g/a");
    assert_eq!(projects[1].default_branch, None, "an empty repository");
    assert!(!truncated);

    let seen = t.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].contains("membership=true"), "{}", seen[0]);
    assert!(seen[0].contains("search=my%20app"), "{}", seen[0]);
    assert!(
        seen[0].contains("page=1") && seen[1].contains("page=2"),
        "{seen:?}"
    );
}

/// A member of a large group can see thousands of projects; the picker stops
/// after a few pages and says there are more.
#[tokio::test]
async fn a_long_project_list_is_capped_and_says_so() {
    let t = Routed::new().pages(
        "/projects?",
        vec![
            (200, r#"[{"id":1,"path_with_namespace":"g/a"}]"#, Some("2")),
            (200, r#"[{"id":2,"path_with_namespace":"g/b"}]"#, Some("3")),
            (200, r#"[{"id":3,"path_with_namespace":"g/c"}]"#, Some("4")),
            (200, r#"[{"id":4,"path_with_namespace":"g/d"}]"#, Some("5")),
        ],
    );
    let listing = wizard::list_projects(&client(t.clone()), &TokenKind::Personal, None)
        .await
        .unwrap();
    let ProjectListing::Projects {
        projects,
        truncated,
    } = listing
    else {
        panic!()
    };
    assert_eq!(t.seen().len(), wizard::PROJECT_PAGES as usize);
    assert_eq!(projects.len(), 3);
    assert!(truncated);
    assert!(!t.seen()[0].contains("search="), "no search, no parameter");
}

/// A project token is not asked at all: the answer is "type it", with the
/// token's own project pre-filled.
#[tokio::test]
async fn a_project_token_gets_type_an_id_or_path_without_a_request() {
    let t = Routed::new();
    let listing = wizard::list_projects(
        &client(t.clone()),
        &TokenKind::Project {
            project_id: Some(82468124),
        },
        None,
    )
    .await
    .unwrap();
    let ProjectListing::TypeIdOrPath { reason, suggestion } = listing else {
        panic!("expected TypeIdOrPath, got {listing:?}");
    };
    assert_eq!(suggestion, Some(82468124));
    assert!(reason.contains("Type the project id or path"), "{reason}");
    assert!(t.seen().is_empty(), "nothing was sent");
}

/// A token that may not list projects (403, or 404 on a locked-down
/// instance) still gets somewhere: typing works. A 401 is a real error.
#[tokio::test]
async fn a_refused_listing_falls_back_to_typing_and_a_401_does_not() {
    for status in [403u16, 404] {
        let t = Routed::new().route("/projects?", status, "{}");
        let listing = wizard::list_projects(&client(t), &TokenKind::Personal, None)
            .await
            .unwrap();
        assert!(
            matches!(
                listing,
                ProjectListing::TypeIdOrPath {
                    suggestion: None,
                    ..
                }
            ),
            "{status}: {listing:?}"
        );
    }
    let t = Routed::new().route("/projects?", 401, "{}");
    let err = wizard::list_projects(&client(t), &TokenKind::Personal, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, WizardError::Unauthorized { status: 401, .. }),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// resolve_project
// ---------------------------------------------------------------------------

#[test]
fn what_people_type_for_a_project_is_understood() {
    let p = |s: &str| wizard::parse_project_input(s, Provider::Gitlab);
    assert_eq!(p(" 82468124 ").unwrap(), ProjectRef::Id(82468124));
    for typed in [
        "acme-corp/monorepo",
        "https://gitlab.com/acme-corp/monorepo",
        "https://gitlab.com/acme-corp/monorepo.git",
        "https://gitlab.com/acme-corp/monorepo/",
        "https://gitlab.com/acme-corp/monorepo/-/pipelines/1",
        "/acme-corp/monorepo/",
    ] {
        assert_eq!(
            p(typed).unwrap(),
            ProjectRef::Path("acme-corp/monorepo".into()),
            "{typed}"
        );
    }
    for bad in [
        "",
        "   ",
        "justaname",
        "a//b",
        "https://gitlab.com/",
        "a b/c",
    ] {
        let err = p(bad).unwrap_err();
        let WizardError::Input(issue) = err else {
            panic!("{bad:?}: {err:?}")
        };
        assert_eq!(issue.step, WizardStep::Project, "{bad:?}");
    }
}

#[tokio::test]
async fn a_typed_path_resolves_to_id_path_and_default_branch() {
    let t = Routed::new().route(
        "/projects/g%2Fsub%2Fapp",
        200,
        r#"{"id":42,"path_with_namespace":"g/sub/app","name":"app","default_branch":"trunk",
            "web_url":"https://gitlab.com/g/sub/app"}"#,
    );
    let resolved = wizard::resolve_project(
        &client(t.clone()),
        Provider::Gitlab,
        "https://gitlab.com/g/sub/app.git",
    )
    .await
    .unwrap();
    assert_eq!(resolved.id, 42);
    assert_eq!(resolved.path, "g/sub/app");
    assert_eq!(
        resolved.project,
        ProjectRef::Id(42),
        "GitLab still writes the id, which survives a rename"
    );
    assert_eq!(resolved.default_branch.as_deref(), Some("trunk"));
    assert_eq!(t.seen(), ["/projects/g%2Fsub%2Fapp"]);
}

#[tokio::test]
async fn an_unknown_project_is_not_found_by_name() {
    let err = wizard::resolve_project(&client(Routed::new()), Provider::Gitlab, "123")
        .await
        .unwrap_err();
    let WizardError::NotFound { what } = &err else {
        panic!("{err:?}")
    };
    assert!(what.contains("123"), "{what}");
    assert!(err.to_string().contains("cannot see it"), "{err}");
}

// ---------------------------------------------------------------------------
// suggest_deploy_markers
// ---------------------------------------------------------------------------

fn job(name: &str, stage: &str, status: &str) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": 1, "name": name, "stage": stage, "status": status
    }))
    .unwrap()
}

#[test]
fn deploy_like_names_and_stages_rank_first_and_the_rest_are_dropped() {
    let parent = vec![
        job("build", "build", "success"),
        job("release:notes", "post", "success"),
        job("trigger:website", "deploy", "success"),
    ];
    let child = vec![
        job("deploy:origins", "deploy", "success"),
        job("deploy:production", "deploy", "success"),
        job("verify:web_test", "verify", "success"),
        job("deploy:canary", "deploy", "manual"),
        job("cdn_purge", "deploy", "success"),
    ];
    let ranked = wizard::rank_deploy_markers(&[
        MarkerScope {
            label: "parent",
            jobs: &parent,
        },
        MarkerScope {
            label: "trigger:website",
            jobs: &child,
        },
    ]);
    let names: Vec<&str> = ranked.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "deploy:production", // deploy name + stage + prod + succeeded = 9
            "deploy:origins",    // 8
            "deploy:canary",     // 7: never ran, so no success point
            "trigger:website",   // 4: deploy stage + succeeded
            "cdn_purge",         // 4, but in a later pipeline
            "release:notes",     // 3
        ]
    );
    assert!(!names.contains(&"build") && !names.contains(&"verify:web_test"));
    assert_eq!(ranked[1].pipeline, "trigger:website");
    assert!(ranked[0].reasons.iter().any(|r| r.contains("production")));
}

#[test]
fn a_name_in_two_pipelines_is_offered_once_at_its_best() {
    let a = vec![job("deploy", "test", "failed")];
    let b = vec![job("deploy", "deploy", "success")];
    let ranked = wizard::rank_deploy_markers(&[
        MarkerScope {
            label: "parent",
            jobs: &a,
        },
        MarkerScope {
            label: "trigger:x",
            jobs: &b,
        },
    ]);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].pipeline, "trigger:x");
}

/// Against the real ca41ab28 recording: the latest push pipeline is read,
/// every bridge is walked, and the deploy stage of the website child comes
/// out on top. `trigger:*` jobs are not jobs and are never offered.
#[tokio::test]
async fn suggestions_from_a_recorded_pipeline_walk_the_children() {
    let dir = support::fixtures_dir().join("ca41ab28-deployed-with-failure");
    let transport = Arc::new(FixtureTransport::load(&dir).unwrap());
    let got = wizard::suggest_deploy_markers(
        &client(transport.clone()),
        &ProjectRef::Id(82468124),
        "main",
        None,
    )
    .await
    .unwrap();

    assert_eq!(got.pipeline_id, Some(2857464986));
    assert!(got.unread_children.is_empty());
    let names: Vec<&str> = got.suggestions.iter().map(|s| s.name.as_str()).collect();
    for expected in ["deploy:origins", "deploy:marketing"] {
        assert!(names.contains(&expected), "{names:?}");
    }
    assert!(
        names.iter().all(|n| !n.starts_with("trigger:")),
        "{names:?}"
    );
    assert!(got.suggestions[0].score >= got.suggestions.last().unwrap().score);
    assert!(
        names.iter().position(|n| *n == "deploy:origins").unwrap()
            < names
                .iter()
                .position(|n| n.starts_with("verify"))
                .unwrap_or(usize::MAX),
    );
    let children = transport
        .seen()
        .iter()
        .filter(|p| p.contains("/jobs?") && !p.contains("2857464986"))
        .count();
    assert_eq!(
        children,
        4,
        "one request per child: {:#?}",
        transport.seen()
    );
}

/// The newest push pipeline is used even when an hourly schedule is newer,
/// and a child the token cannot read is reported rather than failing the step.
#[tokio::test]
async fn a_schedule_is_skipped_and_an_unreadable_child_is_reported() {
    let t = Routed::new()
        .route(
            "/projects/1/pipelines?",
            200,
            r#"[{"id":20,"status":"failed","source":"schedule"},
                {"id":10,"status":"success","source":"push","web_url":"https://gl/p/10"}]"#,
        )
        .route(
            "/projects/1/pipelines/10/jobs",
            200,
            r#"[{"id":1,"name":"deploy","stage":"deploy","status":"success"}]"#,
        )
        .route(
            "/projects/1/pipelines/10/bridges",
            200,
            r#"[{"id":2,"name":"trigger:other","status":"success",
                 "downstream_pipeline":{"id":99,"project_id":7,"status":"success"}},
                {"id":3,"name":"trigger:mine","status":"success",
                 "downstream_pipeline":{"id":98,"project_id":1,"status":"success"}}]"#,
        )
        .route("/projects/7/", 403, "{}")
        .route(
            "/projects/1/pipelines/98/jobs",
            200,
            r#"[{"id":4,"name":"publish:npm","stage":"release","status":"success"}]"#,
        );
    let got = wizard::suggest_deploy_markers(&client(t.clone()), &ProjectRef::Id(1), "main", None)
        .await
        .unwrap();
    assert_eq!(got.pipeline_id, Some(10));
    assert_eq!(got.pipeline_url.as_deref(), Some("https://gl/p/10"));
    assert_eq!(got.unread_children, ["trigger:other"]);
    let names: Vec<&str> = got.suggestions.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["deploy", "publish:npm"]);
    assert!(
        t.seen().iter().all(|p| !p.contains("/pipelines/20/")),
        "the schedule was never read: {:?}",
        t.seen()
    );
}

#[tokio::test]
async fn a_ref_with_no_pipelines_suggests_nothing() {
    let t = Routed::new().route("/projects/1/pipelines?", 200, "[]");
    let got = wizard::suggest_deploy_markers(&client(t.clone()), &ProjectRef::Id(1), "main", None)
        .await
        .unwrap();
    assert_eq!(got.pipeline_id, None);
    assert!(got.suggestions.is_empty());
    assert_eq!(t.seen().len(), 1);
}

#[tokio::test]
async fn a_project_the_token_cannot_see_is_an_error_for_the_deploy_step() {
    let t = Routed::new().route("/projects/1/pipelines?", 404, "{}");
    let err = wizard::suggest_deploy_markers(&client(t), &ProjectRef::Id(1), "main", None)
        .await
        .unwrap_err();
    assert!(matches!(err, WizardError::NotFound { .. }), "{err:?}");
}

// ---------------------------------------------------------------------------
// glab's keyring item
// ---------------------------------------------------------------------------

/// A fake store. It records what it was asked and can only say yes, no or
/// "cannot ask": the trait has no way to hand back a value.
#[derive(Default)]
struct FakeProbe {
    present: Vec<(String, String)>,
    broken: bool,
    asked: Mutex<Vec<(String, String)>>,
}

impl KeyringProbe for FakeProbe {
    fn exists(&self, service: &str, user: &str) -> Result<bool, String> {
        self.asked
            .lock()
            .unwrap()
            .push((service.to_string(), user.to_string()));
        if self.broken {
            return Err("secret-tool is not installed".into());
        }
        Ok(self.present.iter().any(|(s, u)| s == service && u == user))
    }
}

#[test]
fn glab_s_item_is_offered_when_it_exists_and_only_then() {
    let probe = FakeProbe {
        present: vec![("glab:gitlab.com:token".into(), String::new())],
        ..FakeProbe::default()
    };
    assert_eq!(
        wizard::detect_cli_token(&probe, Provider::Gitlab, "https://gitlab.com"),
        CliTokenDetection::Found {
            source: TokenSource::Keyring {
                service: "glab:gitlab.com:token".into(),
                user: String::new(),
            }
        }
    );
    assert_eq!(
        wizard::detect_cli_token(&probe, Provider::Gitlab, "https://GitLab.Example.com:8443/"),
        CliTokenDetection::NotFound {
            service: "glab:gitlab.example.com:8443:token".into()
        }
    );
    assert_eq!(
        probe.asked.lock().unwrap()[0],
        ("glab:gitlab.com:token".to_string(), String::new()),
        "glab writes an EMPTY user"
    );

    let broken = FakeProbe {
        broken: true,
        ..FakeProbe::default()
    };
    assert!(matches!(
        wizard::detect_cli_token(&broken, Provider::Gitlab, "https://gitlab.com"),
        CliTokenDetection::Unavailable { .. }
    ));
    assert!(matches!(
        wizard::detect_cli_token(&probe, Provider::Gitlab, "gitlab.com"),
        CliTokenDetection::Unavailable { .. }
    ));
}

// ---------------------------------------------------------------------------
// build_config
// ---------------------------------------------------------------------------

fn answers() -> WizardAnswers {
    WizardAnswers {
        provider: Provider::Gitlab,
        account: "gitlab".into(),
        base_url: "https://gitlab.com".into(),
        token: TokenSource::Keyring {
            service: "glab:gitlab.com:token".into(),
            user: String::new(),
        },
        project: Some(ProjectRef::Id(42)),
        watch_id: "app-main".into(),
        ref_name: "main".into(),
        workflow: None,
        sources: vec!["push".into()],
        deploy_markers: vec!["deploy:production".into()],
        schedule_watch: true,
        preflight_ref: Some("pf/*".into()),
        notify: Some(NotifyAnswers {
            deployed: true,
            blocking_failure: true,
            finished: false,
        }),
        launch_at_login: Some(true),
        live_secs: Some(5),
    }
}

fn load(text: &str) -> config::Config {
    config::parse_str(text, std::path::Path::new("config.toml"))
        .unwrap_or_else(|e| panic!("{e}\n{text}"))
        .config
}

/// From nothing: a file that loads, says what was asked, and round-trips.
#[test]
fn a_fresh_config_loads_and_means_the_answers() {
    let built = wizard::build_config(&answers(), None).unwrap();
    assert!(!built.edited_existing);
    assert!(
        built.toml.starts_with(wizard::NEW_FILE_HEADER),
        "{}",
        built.toml
    );
    assert!(wizard::find_token_prefix(&built.toml).is_none());

    let c = load(&built.toml);
    let account = &c.accounts["gitlab"];
    assert_eq!(account.base_url, "https://gitlab.com");
    assert_eq!(account.token, answers().token);
    let ids: Vec<&str> = c.watches.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["app-main", "app-main-schedule", "app-main-preflight"]);
    let main = &c.watches[0];
    assert_eq!(main.project, ProjectRef::Id(42));
    assert_eq!(main.role, Role::Primary);
    assert_eq!(main.sources, ["push"]);
    assert_eq!(main.deploy_markers, ["deploy:production"]);
    assert_eq!(main.poll.live_secs, 5);
    assert!(!main.notify.finished && main.notify.deployed);
    let schedule = &c.watches[1];
    assert_eq!(schedule.role, Role::Secondary);
    assert_eq!(schedule.sources, ["schedule"]);
    assert_eq!(schedule.dive.only_when.as_deref(), Some("failed"));
    let preflight = &c.watches[2];
    assert_eq!(preflight.ref_pattern, "pf/*");
    assert_eq!(preflight.dive.bridges, "", "one request a tick");
    assert!(c.ui.launch_at_login);

    // Re-running with the same answers on the result changes nothing at all.
    let again = wizard::build_config(&answers(), Some(&built.toml)).unwrap();
    assert!(again.edited_existing);
    assert_eq!(again.toml, built.toml);
}

/// Re-running on the shipped example with answers that describe it is a
/// no-op, byte for byte: the scheduled and preflight watches it already has
/// (`hourly`, `preflights`) are recognised rather than duplicated.
#[test]
fn re_running_on_a_matching_config_changes_nothing() {
    let example = support::example_config_raw();
    let same = WizardAnswers {
        token: TokenSource::Keyring {
            service: "glab:gitlab.com:token".into(),
            user: String::new(),
        },
        project: Some(ProjectRef::Id(82468124)),
        watch_id: "main-push".into(),
        deploy_markers: vec!["deploy:origins".into(), "deploy:marketing".into()],
        notify: Some(NotifyAnswers {
            deployed: true,
            blocking_failure: true,
            finished: true,
        }),
        launch_at_login: Some(false),
        ..answers()
    };
    let built = wizard::build_config(&same, Some(&example)).unwrap();
    assert_eq!(built.toml, example);
}

/// Re-running on an existing file EDITS it: every comment and every other
/// watch survives, the changed values land, and new watches are appended.
#[test]
fn re_running_on_an_existing_config_edits_rather_than_clobbers() {
    let example = support::example_config_raw();
    let changed = WizardAnswers {
        project: Some(ProjectRef::Id(82468124)),
        watch_id: "main-push".into(),
        deploy_markers: vec!["deploy:origins".into()],
        token: TokenSource::Env("BRIDGEWATCH_TOKEN_GITLAB".into()),
        schedule_watch: true,
        preflight_ref: Some("release/*".into()),
        ..answers()
    };
    let built = wizard::build_config(&changed, Some(&example)).unwrap();

    for line in example.lines().filter(|l| l.trim_start().starts_with('#')) {
        assert!(built.toml.contains(line), "comment lost: {line}");
    }
    assert!(
        built
            .toml
            .contains("# names or \"re:\" regex; the first listed with a success = deployed")
    );

    let c = load(&built.toml);
    assert_eq!(
        c.accounts["gitlab"].token,
        TokenSource::Env("BRIDGEWATCH_TOKEN_GITLAB".into()),
        "the source is REPLACED, not merged into a two-source table"
    );
    assert_eq!(
        c.accounts["gitlab"].timeout_secs, 15,
        "the rest of the account kept"
    );
    let ids: Vec<&str> = c.watches.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(
        ids,
        ["main-push", "hourly", "preflights", "main-push-preflight"],
        "hourly already covers schedules; release/* is new"
    );
    assert_eq!(c.watches[0].deploy_markers, ["deploy:origins"]);
    assert_eq!(c.watches[0].jobs.entries().len(), 2, "overrides untouched");
    assert!(!c.watches[0].notify.finished, "the answer landed");
}

/// Answers that cannot be used are refused with every problem and its step,
/// and nothing is produced.
#[test]
fn bad_answers_are_refused_with_every_step_named() {
    let bad = WizardAnswers {
        account: " ".into(),
        base_url: "gitlab.com".into(),
        project: None,
        ref_name: String::new(),
        sources: vec!["pushh".into()],
        deploy_markers: vec!["re:(".into()],
        live_secs: Some(0),
        ..answers()
    };
    let WizardError::Answers(issues) = wizard::build_config(&bad, None).unwrap_err() else {
        panic!()
    };
    let steps: Vec<WizardStep> = issues.iter().map(|i| i.step).collect();
    for step in [
        WizardStep::Account,
        WizardStep::Project,
        WizardStep::Watch,
        WizardStep::Deploy,
        WizardStep::Preferences,
    ] {
        assert!(steps.contains(&step), "{step:?} missing from {issues:#?}");
    }
    assert!(issues.len() >= 7, "{issues:#?}");
}

/// ⛔ The config never carries a token. A token pasted where a variable name
/// or a command belongs is refused at the account step, before any text
/// exists...
#[test]
fn a_token_pasted_into_a_source_is_refused() {
    for token in [
        TokenSource::Env(TOKEN.into()),
        TokenSource::Env("glpat_SHAPED_BUT_VALID_NAME".into()),
        TokenSource::Command(vec!["echo".into(), TOKEN.into()]),
        TokenSource::Keyring {
            service: TOKEN.into(),
            user: String::new(),
        },
    ] {
        let result = wizard::build_config(
            &WizardAnswers {
                token: token.clone(),
                ..answers()
            },
            None,
        );
        match (&token, result) {
            // A valid identifier that merely starts with glpat_ is allowed: it
            // is not a token (tokens use a dash).
            (TokenSource::Env(v), Ok(built)) if v.starts_with("glpat_") => {
                assert!(wizard::find_token_prefix(&built.toml).is_none());
            }
            (_, Err(WizardError::Answers(issues))) => {
                assert!(
                    issues
                        .iter()
                        .any(|i| i.step == WizardStep::Account && i.field == "token"),
                    "{issues:#?}"
                );
                let text = format!("{issues:?}");
                assert!(
                    !text.contains("WIZARDSECRET"),
                    "the message quotes no token"
                );
            }
            (t, other) => panic!("{t:?}: {other:?}"),
        }
    }
}

/// ...and the final text is checked as a whole, so a token that slipped in
/// anywhere else (here, typed as a deploy marker) is refused too.
#[test]
fn a_token_anywhere_in_the_result_is_refused() {
    let err = wizard::build_config(
        &WizardAnswers {
            deploy_markers: vec![TOKEN.into()],
            ..answers()
        },
        None,
    )
    .unwrap_err();
    assert!(
        matches!(err, WizardError::TokenInConfig { prefix: "glpat-" }),
        "{err:?}"
    );
    assert!(!err.to_string().contains("WIZARDSECRET"), "{err}");
}

/// An account name with a dot is one key, not a nested table.
#[test]
fn a_dotted_account_name_is_written_as_one_key() {
    let built = wizard::build_config(
        &WizardAnswers {
            account: "gitlab.example.com".into(),
            base_url: "https://gitlab.example.com".into(),
            ..answers()
        },
        None,
    )
    .unwrap();
    let c = load(&built.toml);
    assert!(
        c.accounts.contains_key("gitlab.example.com"),
        "{}",
        built.toml
    );
    assert_eq!(c.watches[0].account, "gitlab.example.com");
}

/// A config that is not TOML is not overwritten: the wizard refuses to edit it.
#[test]
fn an_unparseable_existing_file_is_refused_not_replaced() {
    let err = wizard::build_config(&answers(), Some("this is [not toml")).unwrap_err();
    assert!(matches!(err, WizardError::Edit(_)), "{err:?}");
}

#[test]
fn account_and_watch_names_are_suggested() {
    use Provider::{Github, Gitlab};
    assert_eq!(
        wizard::suggest_account_name(Gitlab, "https://gitlab.com"),
        "gitlab"
    );
    assert_eq!(
        wizard::suggest_account_name(Gitlab, "https://code.acme.io/"),
        "code"
    );
    assert_eq!(wizard::suggest_account_name(Gitlab, "nonsense"), "gitlab");
    // ⚠ The first label of the API host is `api`, which names nothing; one
    // leading `api.` comes off first, exactly as gh's own service name does.
    assert_eq!(
        wizard::suggest_account_name(Github, "https://api.github.com"),
        "github"
    );
    assert_eq!(
        wizard::suggest_account_name(Github, "https://ghe.acme.com"),
        "ghe"
    );
    assert_eq!(
        wizard::suggest_account_name(Github, "https://api.acme.ghe.com"),
        "acme"
    );
    assert_eq!(wizard::suggest_account_name(Github, "nonsense"), "github");
    assert_eq!(
        wizard::suggest_watch_id("acme-corp/monorepo", "main"),
        "monorepo-main"
    );
    assert_eq!(
        wizard::suggest_watch_id("g/My App", "release/1.x"),
        "my-app-release-1-x"
    );
}

// ---------------------------------------------------------------------------
// The IPC failure shape
// ---------------------------------------------------------------------------

/// The shell sends `WizardFailure` over IPC, so its JSON shape is the
/// contract: a snake_case `kind`, the message, per-step issues for answer
/// errors and load diagnostics for an invalid result. No token, ever.
#[tokio::test]
async fn failures_serialise_with_a_kind_and_the_steps_to_revisit() {
    let bad = WizardAnswers {
        project: None,
        live_secs: Some(0),
        ..answers()
    };
    let err = wizard::build_config(&bad, None).unwrap_err();
    let json = serde_json::to_value(err.to_failure()).unwrap();
    assert_eq!(json["kind"], "answers");
    let steps: Vec<&str> = json["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["step"].as_str().unwrap())
        .collect();
    assert!(steps.contains(&"project"), "{json}");
    assert!(steps.contains(&"preferences"), "{json}");
    assert_eq!(json["diagnostics"], serde_json::json!([]));

    let t = Routed::new().route("/user", 401, "{}");
    let err = wizard::test_connection(&client(t), Provider::Gitlab)
        .await
        .unwrap_err();
    let json = serde_json::to_value(wizard::WizardFailure::from(&err)).unwrap();
    assert_eq!(json["kind"], "unauthorized");
    assert_eq!(json["issues"], serde_json::json!([]));
    assert!(!json.to_string().contains(TOKEN), "{json}");

    let err = wizard::build_config(
        &WizardAnswers {
            deploy_markers: vec![TOKEN.into()],
            ..answers()
        },
        None,
    )
    .unwrap_err();
    let json = serde_json::to_value(err.to_failure()).unwrap();
    assert_eq!(json["kind"], "token_in_config");
    assert!(!json.to_string().contains("WIZARDSECRET"), "{json}");

    let err = wizard::resolve_project(&client(Routed::new()), Provider::Gitlab, "nope")
        .await
        .unwrap_err();
    assert_eq!(err.to_failure().kind, wizard::FailureKind::Answers);
    assert_eq!(err.step_issues()[0].step, WizardStep::Project);
}

/// `WizardAnswers` crosses IPC as JSON in both directions; this is the shape
/// the UI sends, with every token source spelled as the UI spells it.
#[test]
fn answers_arrive_as_json_in_the_documented_shape() {
    let sources = [
        (
            serde_json::json!({"keyring": {"service": "glab:gitlab.com:token", "user": ""}}),
            TokenSource::Keyring {
                service: "glab:gitlab.com:token".into(),
                user: String::new(),
            },
        ),
        (
            serde_json::json!({"env": "GITLAB_TOKEN"}),
            TokenSource::Env("GITLAB_TOKEN".into()),
        ),
        (
            serde_json::json!({"command": ["pass", "gitlab"]}),
            TokenSource::Command(vec!["pass".into(), "gitlab".into()]),
        ),
        (serde_json::json!({"own": true}), TokenSource::Own(true)),
    ];
    for (json, want) in sources {
        let answers: WizardAnswers = serde_json::from_value(serde_json::json!({
            "account": "gitlab",
            "base_url": "https://gitlab.com",
            "token": json,
            "project": 82468124,
            "watch_id": "distronode-com-main",
            "ref_name": "main",
            "sources": ["push"],
            "deploy_markers": ["deploy:origins"],
            "schedule_watch": true,
            "preflight_ref": "pf/*",
            "notify": {"deployed": true, "blocking_failure": true, "finished": false},
            "launch_at_login": true,
            "live_secs": 5
        }))
        .unwrap();
        assert_eq!(answers.token, want);
        assert_eq!(answers.project, Some(ProjectRef::Id(82468124)));
        let back = serde_json::to_value(&answers).unwrap();
        let again: WizardAnswers = serde_json::from_value(back).unwrap();
        assert_eq!(again, answers);
        wizard::build_config(&answers, None).unwrap();
    }
    // Missing keys take the defaults, and a path is a string.
    let partial: WizardAnswers =
        serde_json::from_value(serde_json::json!({"project": "g/p"})).unwrap();
    assert_eq!(partial.project, Some(ProjectRef::Path("g/p".into())));
    assert_eq!(partial.token, TokenSource::Own(true));
    assert_eq!(partial.sources, vec!["push".to_string()]);
}

// ---------------------------------------------------------------------------
// GitHub
// ---------------------------------------------------------------------------

/// The answers a github.com account produces, as the wizard's GitHub path
/// fills them in.
fn gh_answers() -> WizardAnswers {
    WizardAnswers {
        provider: Provider::Github,
        account: "github".into(),
        base_url: "https://api.github.com".into(),
        token: TokenSource::Keyring {
            service: "gh:github.com".into(),
            user: String::new(),
        },
        project: Some(ProjectRef::Path("acme-corp/monorepo".into())),
        watch_id: "monorepo-main".into(),
        ref_name: "main".into(),
        workflow: Some("ci.yml".into()),
        sources: vec!["push".into()],
        deploy_markers: vec!["publish".into()],
        schedule_watch: false,
        preflight_ref: None,
        notify: Some(NotifyAnswers {
            deployed: true,
            blocking_failure: true,
            finished: false,
        }),
        launch_at_login: None,
        live_secs: None,
    }
}

/// A github.com account needs no `base_url`, `api_path` or `header` line: all
/// three default per provider, and writing them would pin today's values into
/// a file that should follow the code.
#[test]
fn a_github_config_writes_provider_and_lets_the_three_defaults_apply() {
    let built = wizard::build_config(&gh_answers(), None).unwrap();
    assert!(
        built.warnings.is_empty(),
        "a config the wizard wrote should need no explaining: {:?}",
        built.warnings
    );
    for absent in ["base_url", "api_path", "header"] {
        assert!(
            !built.toml.contains(absent),
            "{absent} was written:\n{}",
            built.toml
        );
    }
    assert!(
        built.toml.contains("provider = \"github\""),
        "{}",
        built.toml
    );

    let c = load(&built.toml);
    let account = &c.accounts["github"];
    assert_eq!(account.provider, Provider::Github);
    assert_eq!(account.base_url, "https://api.github.com");
    assert_eq!(account.api_path, "");
    assert_eq!(
        account.header,
        bridgewatch_core::config::AuthHeader::AuthorizationBearer
    );
    let watch = &c.watches[0];
    assert_eq!(watch.project, ProjectRef::Path("acme-corp/monorepo".into()));
    assert_eq!(watch.workflow.as_deref(), Some("ci.yml"));
    assert_eq!(watch.sources, ["push"]);
    // GitHub's budget is 5,000 an HOUR per token, not gitlab.com's 2,000 a
    // minute, so a new watch starts slower than GitLab's 5/60.
    assert_eq!(watch.poll.live_secs, wizard::GITHUB_LIVE_SECS);
    assert_eq!(watch.poll.idle_secs, wizard::GITHUB_IDLE_SECS);

    // Re-running on its own output changes nothing.
    let again = wizard::build_config(&gh_answers(), Some(&built.toml)).unwrap();
    assert_eq!(again.toml, built.toml);
}

/// ⛔ The one check that matters most: the text the wizard shows for review
/// loads with NO errors and NO warnings of its own.
#[test]
fn a_github_config_validates_with_no_errors_and_no_warnings() {
    for answers in [
        gh_answers(),
        WizardAnswers {
            schedule_watch: true,
            ..gh_answers()
        },
        WizardAnswers {
            workflow: None,
            ..gh_answers()
        },
    ] {
        let built = wizard::build_config(&answers, None).unwrap();
        let loaded = config::parse_str(&built.toml, std::path::Path::new("config.toml"))
            .unwrap_or_else(|e| panic!("{e}\n{}", built.toml));
        assert!(
            loaded.warnings.is_empty(),
            "{:?}\n{}",
            loaded.warnings,
            built.toml
        );
    }
}

/// GitHub Enterprise Server is the one shape that needs both keys, and the
/// `api.` prefix is what tells it from GHEC data residency.
#[test]
fn an_enterprise_github_host_gets_base_url_and_the_right_api_path() {
    let ghes = WizardAnswers {
        account: "ghe".into(),
        base_url: "https://ghe.acme.com".into(),
        token: TokenSource::Keyring {
            service: "gh:ghe.acme.com".into(),
            user: String::new(),
        },
        ..gh_answers()
    };
    let built = wizard::build_config(&ghes, None).unwrap();
    let c = load(&built.toml);
    assert_eq!(c.accounts["ghe"].base_url, "https://ghe.acme.com");
    assert_eq!(c.accounts["ghe"].api_path, "/api/v3");

    // Data residency: an `api.` host, and therefore no prefix at all.
    let residency = WizardAnswers {
        account: "acme".into(),
        base_url: "https://api.acme.ghe.com".into(),
        ..ghes
    };
    let c = load(&wizard::build_config(&residency, None).unwrap().toml);
    assert_eq!(c.accounts["acme"].base_url, "https://api.acme.ghe.com");
    assert_eq!(c.accounts["acme"].api_path, "");
}

/// ⛔ A GitLab answer set writes no `provider` line at all, so every file
/// written before the key existed still round-trips and nothing in the settings
/// pane shows a key nobody chose.
#[test]
fn a_gitlab_config_gains_no_provider_line() {
    let built = wizard::build_config(&answers(), None).unwrap();
    assert!(!built.toml.contains("provider"), "{}", built.toml);
    assert_eq!(
        load(&built.toml).accounts["gitlab"].provider,
        Provider::Gitlab
    );
}

#[test]
fn what_people_type_for_a_github_repository_is_understood() {
    let p = |s: &str| wizard::parse_project_input(s, Provider::Github);
    for typed in [
        "acme-corp/monorepo",
        " acme-corp/monorepo ",
        "https://github.com/acme-corp/monorepo",
        "https://github.com/acme-corp/monorepo.git",
        "https://github.com/acme-corp/monorepo/",
        // A URL carries more than two segments all the time; a repository is
        // exactly two, so the rest is trimmed.
        "https://github.com/acme-corp/monorepo/actions/runs/123",
        "https://ghe.acme.com/acme-corp/monorepo",
    ] {
        assert_eq!(
            p(typed).unwrap(),
            ProjectRef::Path("acme-corp/monorepo".into()),
            "{typed}"
        );
    }
    // ⛔ A numeric id is refused where it is typed, because `config::validate`
    // calls it an ERROR: writing it would produce a file the wizard's own final
    // check rejects.
    let err = p("82468124").unwrap_err();
    assert!(err.to_string().contains("owner/repo"), "{err}");
    // A TYPED three-segment path is refused rather than silently truncated to
    // something the user did not write.
    for bad in ["", "   ", "justaname", "a//b", "a/b/c", "a b/c"] {
        let WizardError::Input(issue) = p(bad).unwrap_err() else {
            panic!("{bad:?} was accepted")
        };
        assert_eq!(issue.step, WizardStep::Project, "{bad:?}");
    }
}

#[tokio::test]
async fn a_github_repository_resolves_to_the_path_it_must_be_written_as() {
    let t = Routed::new().route(
        "/repos/acme-corp/monorepo",
        200,
        r#"{"id":700,"full_name":"acme-corp/monorepo","name":"monorepo",
            "default_branch":"trunk","html_url":"https://github.com/acme-corp/monorepo"}"#,
    );
    let resolved = wizard::resolve_project(
        &gh_client(t.clone()),
        Provider::Github,
        "https://github.com/acme-corp/monorepo/actions",
    )
    .await
    .unwrap();
    assert_eq!(resolved.path, "acme-corp/monorepo");
    assert_eq!(resolved.default_branch.as_deref(), Some("trunk"));
    // ⛔ The path, never the id: a github watch with a numeric project is a
    // validation ERROR, and the id GitHub reports has no endpoint that takes it.
    assert_eq!(
        resolved.project,
        ProjectRef::Path("acme-corp/monorepo".into())
    );
    assert_eq!(resolved.id, 700, "reported, but not what gets written");
    assert_eq!(t.seen(), ["/repos/acme-corp/monorepo"]);
}

/// A classic token has an `X-OAuth-Scopes` header and can be judged; a
/// fine-grained one has none, and saying nothing is the honest answer.
#[tokio::test]
async fn a_github_token_is_judged_only_when_github_says_what_it_can_do() {
    let classic = Routed::new()
        .route("/user", 200, GH_USER)
        .oauth_scopes("repo, read:org");
    let id = wizard::test_connection(&gh_client(classic), Provider::Github)
        .await
        .unwrap();
    assert_eq!(id.username, "octo");
    assert_eq!(id.scopes, ["repo", "read:org"]);
    assert!(id.warnings.is_empty(), "{:?}", id.warnings);

    let narrow = Routed::new()
        .route("/user", 200, GH_USER)
        .oauth_scopes("read:user");
    let id = wizard::test_connection(&gh_client(narrow), Provider::Github)
        .await
        .unwrap();
    assert_eq!(id.warnings.len(), 1, "{:?}", id.warnings);
    // ⚠ Names `repo`, and says why the two obvious guesses are wrong: there is
    // no actions:read classic scope, and `workflow` is a WRITE scope for
    // workflow files.
    assert!(id.warnings[0].contains("repo scope"), "{}", id.warnings[0]);
    assert!(
        !id.warnings[0].contains("read_api"),
        "GitLab's scope name reached a GitHub message: {}",
        id.warnings[0]
    );

    // No header at all: a fine-grained or App token. Unknown, and NOT warned
    // about, because it is the credential GitHub itself recommends.
    let fine_grained = Routed::new().route("/user", 200, GH_USER);
    let id = wizard::test_connection(&gh_client(fine_grained), Provider::Github)
        .await
        .unwrap();
    assert_eq!(id.token, TokenKind::Unknown);
    assert!(id.scopes.is_empty());
    assert!(id.warnings.is_empty(), "{:?}", id.warnings);
}

/// ⛔ `read_api` is a GitLab scope name and appears nowhere on GitHub. GitLab's
/// sentence is pinned byte for byte, because it is an error people have already
/// learned to act on.
#[tokio::test]
async fn a_refused_token_is_explained_in_its_own_provider_s_words() {
    let t = Routed::new().route("/user", 401, "{}");
    let err = wizard::test_connection(&client(t), Provider::Gitlab)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "GitLab refused the token (401). Check that it has not expired or been revoked, \
         and that it has the read_api scope"
    );

    let t = Routed::new().route("/user", 401, "{}");
    let err = wizard::test_connection(&gh_client(t), Provider::Github)
        .await
        .unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(text.contains("GitHub refused the token (401)"), "{text}");
    assert!(text.contains("Actions: read"), "{text}");
    assert!(!text.contains("read_api"), "{text}");
    assert!(!text.contains(GH_TOKEN), "{text}");

    // And the client's own message, which the CLI and the debug pane print.
    assert_eq!(
        ClientError::Auth {
            status: 403,
            provider: Provider::Gitlab,
        }
        .to_string(),
        "not authorised (403): check the token's scope (read_api) and that it can see this project"
    );
    let github = ClientError::Auth {
        status: 403,
        provider: Provider::Github,
    }
    .to_string();
    assert!(github.contains("repo scope"), "{github}");
    assert!(!github.contains("read_api"), "{github}");
}

/// `gh` keeps two items under `gh:<host>`; the EMPTY-account one is its active
/// slot, the one `gh auth switch` moves, and it is the one to read.
#[test]
fn gh_s_keyring_item_is_looked_for_at_the_web_host_with_an_empty_user() {
    let probe = FakeProbe {
        present: vec![("gh:github.com".into(), String::new())],
        ..FakeProbe::default()
    };
    assert_eq!(
        wizard::detect_cli_token(&probe, Provider::Github, "https://api.github.com"),
        CliTokenDetection::Found {
            source: TokenSource::Keyring {
                service: "gh:github.com".into(),
                user: String::new(),
            }
        }
    );
    assert_eq!(
        probe.asked.lock().unwrap()[0],
        ("gh:github.com".to_string(), String::new()),
        "the ACTIVE-account slot, not the login-keyed one"
    );
    // ⛔ The service is the WEB host: `api.` comes off github.com and off a
    // data-residency host, and GHES has no prefix to strip because its API is
    // a path.
    assert_eq!(
        wizard::gh_service_for("https://api.acme.ghe.com").as_deref(),
        Some("gh:acme.ghe.com")
    );
    assert_eq!(
        wizard::gh_service_for("https://ghe.acme.com").as_deref(),
        Some("gh:ghe.acme.com")
    );
    assert_eq!(wizard::gh_service_for("github.com"), None);
    assert!(matches!(
        wizard::detect_cli_token(&probe, Provider::Github, "https://ghe.acme.com"),
        CliTokenDetection::NotFound { .. }
    ));
}

/// On GitHub the deploy step is one level deep by construction: there are no
/// bridges, so nothing walks anywhere, and the run it learns from is the run
/// the watch will follow.
#[tokio::test]
async fn github_deploy_suggestions_come_from_one_run_s_jobs() {
    let t = Routed::new()
        .route(
            "/repos/acme/web/actions/workflows/ci.yml/runs",
            200,
            r#"{"total_count":1,"workflow_runs":[
                {"id":55,"run_number":7,"head_sha":"abc","head_branch":"main",
                 "status":"completed","conclusion":"success","event":"push",
                 "html_url":"https://github.com/acme/web/actions/runs/55"}]}"#,
        )
        .route(
            "/repos/acme/web/actions/runs/55/jobs",
            200,
            r#"{"total_count":3,"jobs":[
                {"id":1,"name":"build","status":"completed","conclusion":"success"},
                {"id":2,"name":"publish","status":"completed","conclusion":"success"},
                {"id":3,"name":"deploy / production","status":"completed","conclusion":"success"}]}"#,
        );
    let got = wizard::suggest_deploy_markers(
        &gh_client(t.clone()),
        &ProjectRef::Path("acme/web".into()),
        "main",
        Some("ci.yml"),
    )
    .await
    .unwrap();

    assert_eq!(got.pipeline_id, Some(55));
    assert!(got.unread_children.is_empty());
    let names: Vec<&str> = got.suggestions.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["deploy / production", "publish"]);
    assert!(!names.contains(&"build"));
    // ⛔ Exactly two requests, and the FIRST is the chosen workflow's own runs:
    // without it the newest push run of any workflow supplies the names, and a
    // repository whose newest push run is a linter suggests nothing.
    assert_eq!(t.seen().len(), 2, "{:?}", t.seen());
    assert!(
        t.seen()[0].starts_with("/repos/acme/web/actions/workflows/ci.yml/runs"),
        "{:?}",
        t.seen()
    );
    assert!(
        t.seen().iter().all(|p| !p.contains("bridges")),
        "a GitHub run has no bridges and none were asked for: {:?}",
        t.seen()
    );

    // With no workflow chosen, the repository-wide list is used instead.
    let t = Routed::new().route(
        "/repos/acme/web/actions/runs?",
        200,
        r#"{"workflow_runs":[]}"#,
    );
    let got = wizard::suggest_deploy_markers(
        &gh_client(t.clone()),
        &ProjectRef::Path("acme/web".into()),
        "main",
        None,
    )
    .await
    .unwrap();
    assert_eq!(got.pipeline_id, None);
    assert!(
        t.seen()[0].starts_with("/repos/acme/web/actions/runs?"),
        "{:?}",
        t.seen()
    );
}

/// The answers a GitHub account cannot mean, each named on the step that has
/// the field.
#[test]
fn github_answers_that_cannot_work_are_refused_on_their_own_step() {
    let numeric = WizardAnswers {
        project: Some(ProjectRef::Id(82468124)),
        ..gh_answers()
    };
    let WizardError::Answers(issues) = wizard::build_config(&numeric, None).unwrap_err() else {
        panic!("a numeric project was accepted for a github account")
    };
    assert!(
        issues
            .iter()
            .any(|i| i.step == WizardStep::Project && i.message.contains("owner/repo")),
        "{issues:#?}"
    );

    // A GitLab source on a GitHub account is answered 200 with no rows, so the
    // watch would look empty and the API would agree with it.
    let gitlab_source = WizardAnswers {
        sources: vec!["merge_request_event".into()],
        ..gh_answers()
    };
    let WizardError::Answers(issues) = wizard::build_config(&gitlab_source, None).unwrap_err()
    else {
        panic!("a GitLab source was accepted for a github account")
    };
    assert!(issues.iter().any(|i| i.field == "sources"), "{issues:#?}");
    // ...and `push` is the one value both vocabularies share.
    assert!(wizard::build_config(&gh_answers(), None).is_ok());

    // A `pf/*` preflight watch is a GitLab idiom; a GitHub account is told so
    // on the watch step rather than handed a file without the watch it asked for.
    let preflight = WizardAnswers {
        preflight_ref: Some("pf/*".into()),
        ..gh_answers()
    };
    let WizardError::Answers(issues) = wizard::build_config(&preflight, None).unwrap_err() else {
        panic!("a preflight watch was accepted for a github account")
    };
    assert!(
        issues
            .iter()
            .any(|i| i.step == WizardStep::Watch && i.field == "preflight_ref"),
        "{issues:#?}"
    );

    // A workflow on a GitLab account is inert, and `config::validate` warns
    // about it, so the wizard never writes one.
    let gitlab_workflow = WizardAnswers {
        workflow: Some("ci.yml".into()),
        ..answers()
    };
    let WizardError::Answers(issues) = wizard::build_config(&gitlab_workflow, None).unwrap_err()
    else {
        panic!("a workflow was accepted for a gitlab account")
    };
    assert!(
        issues
            .iter()
            .any(|i| i.step == WizardStep::Watch && i.field == "workflow"),
        "{issues:#?}"
    );
}

/// The provider crosses IPC as a plain string, and its absence is `gitlab`, so
/// a payload written before the key existed means what it always did.
#[test]
fn github_answers_arrive_as_json_with_provider_and_workflow() {
    let answers: WizardAnswers = serde_json::from_value(serde_json::json!({
        "provider": "github",
        "account": "github",
        "base_url": "https://api.github.com",
        "token": {"keyring": {"service": "gh:github.com", "user": ""}},
        "project": "acme-corp/monorepo",
        "watch_id": "monorepo-main",
        "ref_name": "main",
        "workflow": "ci.yml",
        "sources": ["push"],
        "deploy_markers": ["publish"]
    }))
    .unwrap();
    assert_eq!(answers.provider, Provider::Github);
    assert_eq!(answers.workflow.as_deref(), Some("ci.yml"));
    wizard::build_config(&answers, None).unwrap();

    let partial: WizardAnswers =
        serde_json::from_value(serde_json::json!({"project": "g/p"})).unwrap();
    assert_eq!(partial.provider, Provider::Gitlab);
    assert_eq!(partial.workflow, None);
}

/// A GitHub token pasted where a variable NAME or a command belongs is refused
/// like a GitLab one, while a GitLab file is still judged only by GitLab's
/// prefixes, so a `ghs_` in a branch pattern changes nothing there.
#[test]
fn github_token_shapes_are_refused_for_github_and_ignored_for_gitlab() {
    for (pasted, secret) in [
        ("ghp_SECRETPART1", "SECRETPART1"),
        ("github_pat_SECRETPART2", "SECRETPART2"),
        ("gho_SECRETPART3", "SECRETPART3"),
        ("ghs_SECRETPART4", "SECRETPART4"),
    ] {
        let answers = WizardAnswers {
            token: TokenSource::Env(pasted.into()),
            ..gh_answers()
        };
        let WizardError::Answers(issues) = wizard::build_config(&answers, None).unwrap_err() else {
            panic!("{pasted} was accepted as a variable name")
        };
        assert!(
            issues
                .iter()
                .any(|i| i.field == "token" && i.message.contains("looks like a token")),
            "{issues:#?}"
        );
        let text = format!("{issues:?}");
        assert!(!text.contains(secret), "the value was echoed: {text}");
    }
    let gitlab = WizardAnswers {
        ref_name: "ghs_release".into(),
        ..answers()
    };
    assert!(wizard::build_config(&gitlab, None).is_ok());
    assert_eq!(
        wizard::find_token_prefix("ghp_abc"),
        None,
        "GitLab's list is unchanged"
    );
}
