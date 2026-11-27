// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! What the config loader says about a value of the wrong shape, for the keys
//! with a hand-written deserializer: the token source, `oauth`, and the job
//! override table. Each message names what was expected, which is the whole
//! help a person editing the file gets.

use bridgewatch_core::config::{Provider, parse_str};

fn load(account: &str, watch: &str) -> Result<bridgewatch_core::config::Loaded, String> {
    parse_str(
        &format!(
            r#"
            [accounts.gl]
            {account}
            [[watches]]
            id = "x"
            account = "gl"
            project = 1
            {watch}
            "#
        ),
        std::path::Path::new("t.toml"),
    )
    .map_err(|e| e.to_string())
}

#[test]
fn a_token_that_is_not_a_table_says_what_a_source_looks_like() {
    let err = load("token = 5", "").unwrap_err();
    assert!(err.contains("a token source table"), "{err}");
}

#[test]
fn an_empty_or_unknown_token_source_is_refused_with_the_names_that_work() {
    let empty = load("token = { }", "").unwrap_err();
    assert!(empty.contains("names no source"), "{empty}");
    let unknown = load(r#"token = { vault = "x" }"#, "").unwrap_err();
    assert!(
        unknown.contains(r#"unknown token source "vault""#),
        "{unknown}"
    );
}

#[test]
fn an_oauth_value_of_the_wrong_shape_says_both_spellings() {
    let err = load(r#"token = { oauth = "yes" }"#, "").unwrap_err();
    assert!(
        err.contains("oauth = true, or oauth = { client_id"),
        "{err}"
    );
}

#[test]
fn a_job_table_that_is_not_a_table_says_what_it_maps() {
    let err = load(r#"token = { env = "T" }"#, "jobs = 5").unwrap_err();
    assert!(err.contains("a table of job-name patterns"), "{err}");
}

#[test]
fn job_overrides_are_written_back_in_file_order() {
    let loaded = load(
        r#"token = { env = "T" }"#,
        r#"jobs = { "zeta" = "gate", "alpha" = "ignore" }"#,
    )
    .unwrap();
    let json = serde_json::to_string(&loaded.config.watches[0].jobs).unwrap();
    assert_eq!(json, r#"{"zeta":"gate","alpha":"ignore"}"#);
}

#[test]
fn a_provider_is_displayed_as_its_config_word() {
    assert_eq!(Provider::Gitlab.to_string(), "gitlab");
    assert_eq!(Provider::Github.to_string(), Provider::Github.as_str());
}
