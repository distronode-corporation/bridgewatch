// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! `validate_answers` on the answers `wizard.rs` does not send: one test per
//! message, on the shipped answers with one field changed, and each checked to
//! land on the step and field the wizard sends the user back to.

use bridgewatch_core::config::{ProjectRef, Provider, TokenSource};
use bridgewatch_core::wizard::{
    ListedBy, NotifyAnswers, QuickWatch, WizardAnswers, WizardStep, validate_answers,
};

fn answers() -> WizardAnswers {
    WizardAnswers {
        provider: Provider::Gitlab,
        account: "gitlab".into(),
        base_url: "https://gitlab.com".into(),
        token: TokenSource::Own(true),
        project: Some(ProjectRef::Id(42)),
        watch_id: "app-main".into(),
        ref_name: "main".into(),
        workflow: None,
        group: Default::default(),
        expect: Vec::new(),
        sources: vec!["push".into()],
        deploy_markers: vec!["deploy:production".into()],
        schedule_watch: true,
        preflight_ref: Some("pf/*".into()),
        primary: false,
        notify: Some(NotifyAnswers {
            deployed: true,
            blocking_failure: true,
            finished: false,
        }),
        launch_at_login: Some(true),
        live_secs: Some(5),
        watches: Vec::new(),
    }
}

/// The issues for these answers, as `(step, field, message)`.
fn issues(a: &WizardAnswers) -> Vec<(WizardStep, String, String)> {
    validate_answers(a)
        .into_iter()
        .map(|i| (i.step, i.field, i.message))
        .collect()
}

fn has(a: &WizardAnswers, step: WizardStep, field: &str, message: &str) -> bool {
    let found = issues(a);
    let hit = found
        .iter()
        .any(|(s, f, m)| *s == step && f == field && m.contains(message));
    if !hit {
        eprintln!("{found:?}");
    }
    hit
}

#[test]
fn the_shipped_answers_have_no_issue() {
    assert_eq!(issues(&answers()), []);
}

#[test]
fn a_token_source_that_names_nothing_is_an_account_issue() {
    let mut a = answers();
    a.token = TokenSource::Keyring {
        service: " ".into(),
        user: String::new(),
    };
    assert!(has(
        &a,
        WizardStep::Account,
        "token",
        "the keyring item needs a service name"
    ));
    a.token = TokenSource::Own(false);
    assert!(has(
        &a,
        WizardStep::Account,
        "token",
        "choose where the token comes from"
    ));
}

#[test]
fn an_empty_project_path_and_a_bad_branch_pattern_are_issues_on_their_steps() {
    let mut a = answers();
    a.project = Some(ProjectRef::Path(" ".into()));
    assert!(has(
        &a,
        WizardStep::Project,
        "project",
        "the project path is empty"
    ));
    let mut a = answers();
    a.ref_name = "re:(".into();
    assert!(
        issues(&a)
            .iter()
            .any(|(s, f, _)| *s == WizardStep::Watch && f == "ref_name")
    );
}

#[test]
fn a_preflight_pattern_that_is_empty_or_does_not_parse_is_a_watch_issue() {
    let mut a = answers();
    a.preflight_ref = Some(" ".into());
    assert!(has(
        &a,
        WizardStep::Watch,
        "preflight_ref",
        "the preflight pattern is empty"
    ));
    a.preflight_ref = Some("re:(".into());
    assert!(
        issues(&a)
            .iter()
            .any(|(s, f, _)| *s == WizardStep::Watch && f == "preflight_ref")
    );
}

#[test]
fn an_empty_deploy_marker_is_a_deploy_issue() {
    let mut a = answers();
    a.deploy_markers = vec![" ".into()];
    assert!(has(
        &a,
        WizardStep::Deploy,
        "deploy_markers",
        "a deploy marker is empty"
    ));
}

fn quick(path: &str, ref_name: &str, markers: &[&str]) -> QuickWatch {
    QuickWatch {
        project: ProjectRef::Path(path.into()),
        path: path.into(),
        ref_name: ref_name.into(),
        deploy_markers: markers.iter().map(|m| m.to_string()).collect(),
        listed_by: ListedBy {
            provider: Provider::Gitlab,
            origin: "https://gitlab.com".into(),
        },
    }
}

#[test]
fn a_one_click_watch_is_checked_like_a_step_by_step_one() {
    let mut a = answers();
    a.project = None;
    a.watches = vec![quick(" ", "main", &[])];
    assert!(has(
        &a,
        WizardStep::Account,
        "watches",
        "a listed project has no path"
    ));
    a.watches = vec![quick("acme/web", "re:(", &[])];
    assert!(has(&a, WizardStep::Account, "watches", "acme/web: "));
    a.watches = vec![quick("acme/web", "main", &[" "])];
    assert!(has(
        &a,
        WizardStep::Account,
        "watches",
        "acme/web: a deploy marker is empty"
    ));
    a.watches = vec![quick("acme/web", "main", &["re:("])];
    assert!(
        issues(&a)
            .iter()
            .any(|(_, f, m)| f == "watches" && m.starts_with("acme/web: ") && !m.contains("empty"))
    );
}

/// A re-run over the wizard's own file changes the watch it wrote in place:
/// its account, project (by path), branch, sources and live interval.
#[test]
fn a_rerun_changes_the_watch_it_wrote_in_place() {
    use bridgewatch_core::wizard::build_config;
    let first = build_config(&answers(), None).unwrap().toml;
    let changed = WizardAnswers {
        account: "work".into(),
        project: Some(ProjectRef::Path("acme/web".into())),
        ref_name: "develop".into(),
        sources: vec!["web".into()],
        live_secs: Some(9),
        ..answers()
    };
    let built = build_config(&changed, Some(&first)).unwrap();
    let config = bridgewatch_core::config::parse_str(&built.toml, std::path::Path::new("c.toml"))
        .unwrap()
        .config;
    let w = config
        .watches
        .iter()
        .find(|w| w.id == "app-main")
        .expect("the same watch");
    let before = bridgewatch_core::config::parse_str(&first, std::path::Path::new("c.toml"))
        .unwrap()
        .config;
    assert_eq!(
        config.watches.len(),
        before.watches.len(),
        "changed in place, not added: {}",
        built.toml
    );
    assert_eq!(w.account, "work");
    assert_eq!(w.project, ProjectRef::Path("acme/web".into()));
    assert_eq!(w.ref_pattern, "develop");
    assert_eq!(w.sources, ["web"]);
    assert_eq!(w.poll.live_secs, 9);
}

/// A file that does not parse cannot be edited, and the wizard says so on
/// the existing-config path rather than writing over it.
#[test]
fn an_existing_file_that_is_not_toml_is_not_written_over() {
    use bridgewatch_core::wizard::build_config;
    let err = build_config(&answers(), Some("[accounts\nbroken")).unwrap_err();
    assert_eq!(
        err.to_failure().kind,
        bridgewatch_core::wizard::FailureKind::ExistingConfig,
        "{err}"
    );
}

/// What each error is called over IPC, which decides what the wizard shows.
#[test]
fn every_error_has_its_failure_kind_and_only_an_invalid_file_carries_diagnostics() {
    use bridgewatch_core::client::ClientError;
    use bridgewatch_core::config::{Diagnostic, Severity};
    use bridgewatch_core::wizard::{FailureKind, WizardError};
    let not_found = WizardError::NotFound {
        what: "project acme/web".into(),
    }
    .to_failure();
    assert_eq!(not_found.kind, FailureKind::NotFound);
    let network = WizardError::Client(ClientError::Server { status: 502 }).to_failure();
    assert_eq!(network.kind, FailureKind::Network);
    let diagnostic = Diagnostic {
        severity: Severity::Error,
        path: "watches".into(),
        message: "bad".into(),
        span: None,
    };
    let invalid = WizardError::Invalid(vec![diagnostic.clone()]).to_failure();
    assert_eq!(invalid.kind, FailureKind::Invalid);
    assert_eq!(invalid.diagnostics.len(), 1);
    assert_eq!(invalid.diagnostics[0].message, diagnostic.message);
    assert!(network.diagnostics.is_empty());
}
