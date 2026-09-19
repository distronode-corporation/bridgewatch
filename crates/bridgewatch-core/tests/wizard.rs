//! The setup wizard's steps, against a scripted transport and a fake keyring.
//!
//! Nothing here touches a network, a real credential store or a file: the
//! transport answers by path, the keyring probe is a fake that can only say
//! "exists" or "does not", and `build_config` returns text.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bridgewatch_core::client::http::{HttpRequest, HttpResponse, Transport};
use bridgewatch_core::client::{ClientError, FixtureTransport, GitLabClient, RequestRing};
use bridgewatch_core::config::{self, Account, ProjectRef, Role, TokenSource};
use bridgewatch_core::model::{Job, User};
use bridgewatch_core::token::Secret;
use bridgewatch_core::wizard::{
    self, GlabDetection, KeyringProbe, MarkerScope, NotifyAnswers, ProjectListing, TokenKind,
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
            oauth_scopes: None,
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

const PERSONAL_USER: &str =
    r#"{"id":1,"username":"sean","name":"Sean","bot":false,"email":"x@example.invalid"}"#;
const READ_API_TOKEN: &str = r#"{"id":9,"name":"bridgewatch","scopes":["read_api"],"expires_at":"2027-01-01","active":true}"#;

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
    let id = wizard::test_connection(&client(t.clone())).await.unwrap();

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
    let id = wizard::test_connection(&client(t)).await.unwrap();

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
    let id = wizard::test_connection(&client(t)).await.unwrap();
    assert_eq!(id.warnings.len(), 1);
    assert!(id.warnings[0].contains("read_api"), "{}", id.warnings[0]);
}

/// `/personal_access_tokens/self` is best effort: an instance without it still
/// answers who the user is.
#[tokio::test]
async fn an_instance_without_the_token_endpoint_still_connects() {
    let t = Routed::new().route("/user", 200, PERSONAL_USER);
    let id = wizard::test_connection(&client(t)).await.unwrap();
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
        let err = wizard::test_connection(&client(t)).await.unwrap_err();
        assert!(
            matches!(err, WizardError::Unauthorized { status: s } if s == status),
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
        matches!(err, WizardError::Unauthorized { status: 401 }),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// resolve_project
// ---------------------------------------------------------------------------

#[test]
fn what_people_type_for_a_project_is_understood() {
    use wizard::parse_project_input as p;
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
    let resolved = wizard::resolve_project(&client(t.clone()), "https://gitlab.com/g/sub/app.git")
        .await
        .unwrap();
    assert_eq!(resolved.id, 42);
    assert_eq!(resolved.path, "g/sub/app");
    assert_eq!(resolved.default_branch.as_deref(), Some("trunk"));
    assert_eq!(t.seen(), ["/projects/g%2Fsub%2Fapp"]);
}

#[tokio::test]
async fn an_unknown_project_is_not_found_by_name() {
    let err = wizard::resolve_project(&client(Routed::new()), "123")
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
    let got = wizard::suggest_deploy_markers(&client(t.clone()), &ProjectRef::Id(1), "main")
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
    let got = wizard::suggest_deploy_markers(&client(t.clone()), &ProjectRef::Id(1), "main")
        .await
        .unwrap();
    assert_eq!(got.pipeline_id, None);
    assert!(got.suggestions.is_empty());
    assert_eq!(t.seen().len(), 1);
}

#[tokio::test]
async fn a_project_the_token_cannot_see_is_an_error_for_the_deploy_step() {
    let t = Routed::new().route("/projects/1/pipelines?", 404, "{}");
    let err = wizard::suggest_deploy_markers(&client(t), &ProjectRef::Id(1), "main")
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
        wizard::detect_glab_token(&probe, "https://gitlab.com"),
        GlabDetection::Found {
            source: TokenSource::Keyring {
                service: "glab:gitlab.com:token".into(),
                user: String::new(),
            }
        }
    );
    assert_eq!(
        wizard::detect_glab_token(&probe, "https://GitLab.Example.com:8443/"),
        GlabDetection::NotFound {
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
        wizard::detect_glab_token(&broken, "https://gitlab.com"),
        GlabDetection::Unavailable { .. }
    ));
    assert!(matches!(
        wizard::detect_glab_token(&probe, "gitlab.com"),
        GlabDetection::Unavailable { .. }
    ));
}

// ---------------------------------------------------------------------------
// build_config
// ---------------------------------------------------------------------------

fn answers() -> WizardAnswers {
    WizardAnswers {
        account: "gitlab".into(),
        base_url: "https://gitlab.com".into(),
        token: TokenSource::Keyring {
            service: "glab:gitlab.com:token".into(),
            user: String::new(),
        },
        project: Some(ProjectRef::Id(42)),
        watch_id: "app-main".into(),
        ref_name: "main".into(),
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
    assert_eq!(wizard::suggest_account_name("https://gitlab.com"), "gitlab");
    assert_eq!(
        wizard::suggest_account_name("https://code.acme.io/"),
        "code"
    );
    assert_eq!(wizard::suggest_account_name("nonsense"), "gitlab");
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
    let err = wizard::test_connection(&client(t)).await.unwrap_err();
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

    let err = wizard::resolve_project(&client(Routed::new()), "nope")
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
