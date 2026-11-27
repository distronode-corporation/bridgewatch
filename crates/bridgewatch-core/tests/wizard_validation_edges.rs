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
