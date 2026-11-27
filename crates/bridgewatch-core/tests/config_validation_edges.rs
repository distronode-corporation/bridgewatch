// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! The validator's messages for the mistakes `config.rs` does not make: one
//! test per message, each on the smallest file that produces it, so a reworded
//! message or a check that stops firing shows up as the line that changed.

use std::path::Path;

use bridgewatch_core::config::{Severity, parse_lenient, validate};

/// Every diagnostic the validator gives this file, as `(severity, path, message)`.
fn problems(raw: &str) -> Vec<(Severity, String, String)> {
    let (config, _) = parse_lenient(raw, Path::new("t.toml")).expect("parses");
    validate(&config)
        .into_iter()
        .map(|d| (d.severity, d.path, d.message))
        .collect()
}

fn one_account(account: &str) -> String {
    format!("[accounts.gl]\n{account}\n[[watches]]\nid = \"x\"\naccount = \"gl\"\nproject = 1\n")
}

fn has(
    found: &[(Severity, String, String)],
    severity: Severity,
    path: &str,
    message: &str,
) -> bool {
    found
        .iter()
        .any(|(s, p, m)| *s == severity && p == path && m.contains(message))
}

#[test]
fn a_file_with_no_accounts_says_how_to_add_one() {
    let found = problems("[[watches]]\nid = \"x\"\naccount = \"gl\"\nproject = 1\n");
    assert!(
        has(&found, Severity::Error, "accounts", "no accounts defined"),
        "{found:?}"
    );
}

#[test]
fn an_accounts_base_url_api_path_and_timeout_are_checked() {
    let found = problems(&one_account(
        "base_url = \"\"\napi_path = \"api/v4\"\ntimeout_secs = 0\ntoken = { env = \"T\" }",
    ));
    assert!(
        has(
            &found,
            Severity::Error,
            "accounts.gl.base_url",
            "base_url is empty"
        ),
        "{found:?}"
    );
    assert!(
        has(
            &found,
            Severity::Error,
            "accounts.gl.api_path",
            "must start with a slash"
        ),
        "{found:?}"
    );
    assert!(
        has(
            &found,
            Severity::Error,
            "accounts.gl.timeout_secs",
            "must be > 0"
        ),
        "{found:?}"
    );
}

#[test]
fn a_token_source_that_names_nothing_is_an_error() {
    for (token, message) in [
        (
            "token = { command = [] }",
            "command token source is an empty list",
        ),
        (
            "token = { env = \" \" }",
            "env token source names an empty variable",
        ),
        (
            "token = { keyring = { service = \"\", user = \"\" } }",
            "keyring token source has an empty service",
        ),
        ("token = { own = false }", "selects no source"),
    ] {
        let found = problems(&one_account(token));
        assert!(
            has(&found, Severity::Error, "accounts.gl.token", message),
            "{token}: {found:?}"
        );
    }
}

#[test]
fn a_watch_with_an_empty_id_is_an_error() {
    let found = problems(
        "[accounts.gl]\ntoken = { env = \"T\" }\n[[watches]]\nid = \" \"\naccount = \"gl\"\nproject = 1\n",
    );
    assert!(
        has(&found, Severity::Error, "watches.0.id", "id is empty"),
        "{found:?}"
    );
}

#[test]
fn a_github_project_that_is_not_owner_slash_repo_is_a_warning() {
    let found = problems(
        "[accounts.gh]\nprovider = \"github\"\ntoken = { env = \"T\" }\n[[watches]]\nid = \"x\"\naccount = \"gh\"\nproject = \"acme/web/extra\"\n",
    );
    assert!(
        has(
            &found,
            Severity::Warning,
            "watches.0.project",
            "is not a GitHub owner/repo pair"
        ),
        "{found:?}"
    );
}

#[test]
fn a_job_override_pattern_that_does_not_compile_is_an_error() {
    let found = problems(
        &one_account("token = { env = \"T\" }")
            .replace("project = 1", "project = 1\njobs = { \"re:(\" = \"gate\" }"),
    );
    assert!(
        found
            .iter()
            .any(|(s, p, _)| *s == Severity::Error && p == "watches.0.jobs.re:("),
        "{found:?}"
    );
}
