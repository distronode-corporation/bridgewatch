// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! Loading a config file: a `~` path, a file that is not there, unknown keys in
//! every place a key can sit, and a file so far from a bridgewatch config that
//! it is refused outright.

use std::path::Path;

use bridgewatch_core::config::{ConfigError, expand_tilde, load, parse_lenient};

const BASE: &str = r#"
[accounts.gl]
token = { env = "T" }
[[watches]]
id = "x"
account = "gl"
project = 1
"#;

fn unknown_paths(raw: &str) -> Vec<String> {
    let (_, diagnostics) = parse_lenient(raw, Path::new("t.toml")).expect("lenient parse");
    diagnostics.into_iter().map(|d| d.path).collect()
}

#[test]
fn a_leading_tilde_is_the_home_directory() {
    let home = dirs::home_dir().expect("a home directory");
    assert_eq!(
        expand_tilde(Path::new("~/x/config.toml")),
        home.join("x/config.toml")
    );
    assert_eq!(
        expand_tilde(Path::new("/etc/x.toml")),
        Path::new("/etc/x.toml")
    );
}

#[test]
fn a_file_that_is_not_there_or_cannot_be_read_says_so_naming_it() {
    // A directory of this test's own, so no other file can stand in for the missing one.
    let dir = std::env::temp_dir().join(format!("bw-config-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("no-such-config.toml");
    assert!(matches!(load(&missing), Err(ConfigError::NotFound(named)) if named == missing));
    // There, but not a file that can be read: the directory itself.
    match load(&dir) {
        Err(ConfigError::Io { path: named, .. }) => assert_eq!(named, dir),
        other => panic!("expected an Io error, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unknown_key_is_named_wherever_it_sits() {
    // In the second of two watches.
    let two =
        format!("{BASE}\n[[watches]]\nid = \"y\"\naccount = \"gl\"\nproject = 2\nbogus = 1\n");
    assert_eq!(unknown_paths(&two), ["watches.1.bogus"]);
    // Inside an inline table, one level down and two.
    let inline = BASE.replace(
        "project = 1",
        "project = 1\nshow = { max_rows = 2, bogus = 1 }",
    );
    assert_eq!(unknown_paths(&inline), ["watches.0.show.bogus"]);
    let nested = BASE.replace(
        "[accounts.gl]\ntoken = { env = \"T\" }",
        r#"accounts = { gl = { token = { env = "T" }, rate_limit_backoff = { max_secs = 60, bogus = 1 } } }"#,
    );
    assert_eq!(
        unknown_paths(&nested),
        ["accounts.gl.rate_limit_backoff.bogus"]
    );
}

/// A hand-written deserializer's own "unknown field" (the keyring source's)
/// carries a span the key search cannot place, so it stays fatal rather than
/// guessing which key to drop.
#[test]
fn an_unknown_key_that_cannot_be_placed_is_fatal() {
    let raw = BASE.replace(
        r#"token = { env = "T" }"#,
        r#"token = { keyring = { service = "s", user = "", bogus = 1 } }"#,
    );
    let err = parse_lenient(&raw, Path::new("t.toml"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown field `bogus`"), "{err}");
}

#[test]
fn more_than_32_unknown_keys_is_not_a_bridgewatch_config() {
    let mut raw = BASE.to_string();
    for i in 0..40 {
        raw.push_str(&format!("bogus{i} = 1\n"));
    }
    let err = parse_lenient(&raw, Path::new("t.toml"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("more than 32 unrecognised keys"), "{err}");
}
