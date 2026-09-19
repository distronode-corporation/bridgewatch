//! Configuration: round-tripping, validation, ref matchers and token sources.

mod support;

use std::path::Path;

use bridgewatch_core::config::edit::{
    ConfigEditor, Edit, EditValue, quote_path_segment, split_path,
};
use bridgewatch_core::config::{
    self, Config, FailurePolicy, JobOverride, Pattern, ProjectRef, RefMatcher, Role, Severity,
    TokenSource, WatchRules,
};

fn parse(raw: &str) -> Result<config::Loaded, config::ConfigError> {
    config::parse_str(raw, Path::new("test.toml"))
}

/// The shipped example must load, and must mean what its comments say.
#[test]
fn the_example_config_loads_and_says_what_it_looks_like() {
    let loaded = parse(&support::example_config_raw()).expect("the example is valid");
    let c = &loaded.config;

    assert_eq!(c.accounts.len(), 1);
    let account = &c.accounts["gitlab"];
    assert_eq!(account.base_url, "https://gitlab.com");
    assert_eq!(account.api_path, "/api/v4");
    assert_eq!(account.timeout_secs, 15);
    assert_eq!(account.rate_limit_backoff.max_secs, 300);
    assert_eq!(
        account.token,
        TokenSource::Keyring {
            service: "glab:gitlab.com:token".into(),
            user: String::new(),
        },
        "an empty keyring user is legal, and is how glab stores its token"
    );

    assert_eq!(c.watches.len(), 3);
    let main = &c.watches[0];
    assert_eq!(main.id, "main-push");
    assert_eq!(main.project, ProjectRef::Id(82468124));
    assert_eq!(main.sources, ["push"]);
    assert_eq!(main.role, Role::Primary);
    assert_eq!(main.deploy_markers, ["deploy:origins", "deploy:marketing"]);
    assert_eq!(main.sibling_failure, FailurePolicy::Downgrade);
    assert_eq!(main.post_deploy_failure, FailurePolicy::Downgrade);
    assert_eq!(
        main.jobs.entries(),
        [
            (
                "re:^verify:web_coverage_full".to_string(),
                JobOverride::Gate
            ),
            ("kics-iac-sast".to_string(), JobOverride::Ignore),
        ],
        "job overrides keep their file order, because first match wins"
    );

    assert_eq!(c.watches[1].role, Role::Secondary);
    assert_eq!(c.watches[1].dive.only_when.as_deref(), Some("failed"));
    assert_eq!(
        c.watches[2].dive.bridges, "",
        "an empty dive glob means dive into nothing"
    );
    assert_eq!(c.log.keep_requests, 50);
    assert_eq!(c.ui.popover.width, 440);
}

/// A file with nothing but one account and one watch must load, because every
/// other key has a default.
#[test]
fn a_minimal_config_loads() {
    let loaded = parse(
        r#"
        [accounts.gl]
        token = { env = "TOK" }

        [[watches]]
        id = "x"
        account = "gl"
        project = 1
        "#,
    )
    .expect("minimal config loads");

    let w = &loaded.config.watches[0];
    assert_eq!(w.ref_pattern, "main");
    assert_eq!(w.poll.live_secs, 5, "fast while live: the 0.1.0 default");
    assert_eq!(w.poll.idle_secs, 60);
    assert_eq!(w.show.max_rows, 5);
    assert_eq!(w.dive.bridges, "*");
    assert_eq!(w.dive.depth, 1);
    assert!(w.notify.deployed && w.notify.blocking_failure && w.notify.finished);
    assert!(!w.notify.started && !w.notify.gate_opened);
    assert_eq!(loaded.config.accounts["gl"].base_url, "https://gitlab.com");
}

/// A token written literally into the file is refused, with a message that says
/// what to write instead.
#[test]
fn a_literal_token_is_refused_with_advice() {
    let err = parse(
        r#"
        [accounts.gl]
        token = "glpat-not-a-real-token"

        [[watches]]
        id = "x"
        account = "gl"
        project = 1
        "#,
    )
    .expect_err("a literal token must not load");

    let message = err.to_string();
    assert!(
        message.contains("may not be written literally"),
        "message should explain the refusal: {message}"
    );
    for suggestion in ["keyring", "env", "command", "own"] {
        assert!(
            message.contains(suggestion),
            "message should name {suggestion}: {message}"
        );
    }
}

/// Validation reports every problem at once, with a span for each.
#[test]
fn validation_reports_all_problems_with_spans() {
    let raw = r#"
[accounts.gl]
base_url = "gitlab.example.com"
token = { env = "TOK" }

[[watches]]
id = "one"
account = "nope"
project = 1
ref = "re:[unclosed"
deploy_markers = ["re:(also bad"]

[[watches]]
id = "one"
account = "gl"
project = 1
"#;
    let err = parse(raw).expect_err("this file is not usable");
    let config::ConfigError::Invalid { diagnostics, .. } = err else {
        panic!("expected Invalid, got {err}");
    };

    let errors: Vec<&config::Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    let paths: Vec<&str> = errors.iter().map(|d| d.path.as_str()).collect();

    assert!(paths.contains(&"accounts.gl.base_url"), "{paths:?}");
    assert!(paths.contains(&"watches.0.account"), "{paths:?}");
    assert!(paths.contains(&"watches.0.ref"), "{paths:?}");
    assert!(paths.contains(&"watches.0.deploy_markers.0"), "{paths:?}");
    assert!(paths.contains(&"watches.1.id"), "duplicate id: {paths:?}");
    assert!(
        errors.len() >= 5,
        "all problems at once, not the first: {paths:?}"
    );

    let account_error = errors
        .iter()
        .find(|d| d.path == "watches.0.account")
        .unwrap();
    assert!(
        account_error.message.contains("unknown account") && account_error.message.contains("gl"),
        "the message names the accounts that do exist: {}",
        account_error.message
    );
    let (line, _col) = account_error.line_col(raw).expect("a span was located");
    assert_eq!(
        raw.lines().nth(line - 1).unwrap().trim(),
        r#"account = "nope""#,
        "the span points at the offending line"
    );
}

/// Warnings are returned rather than fatal, and the two the brief calls for are
/// present.
#[test]
fn warnings_do_not_block_loading() {
    let loaded = parse(
        r#"
        [accounts.gl]
        token = { command = ["pass", "gitlab/pat"] }

        [[watches]]
        id = "x"
        account = "gl"
        project = 1
        role = "primary"
        deploy_markers = []
        "#,
    )
    .expect("warnings are not errors");

    let messages: Vec<&str> = loaded.warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("runs a program")),
        "a command token source is flagged: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("no deploy markers")),
        "a primary watch with no markers is allowed but warned: {messages:?}"
    );
    assert!(
        loaded
            .warnings
            .iter()
            .all(|w| w.severity == Severity::Warning)
    );
}

/// Exact, glob and regex ref patterns, and which of them the API can be asked
/// for directly.
#[test]
fn ref_matchers_cover_exact_glob_and_regex() {
    let exact = RefMatcher::parse("main").unwrap();
    assert!(exact.matches("main"));
    assert!(!exact.matches("main2"));
    assert!(!exact.matches("feature/main"));
    assert_eq!(
        exact.exact(),
        Some("main"),
        "an exact ref can be pushed to the API"
    );

    let glob = RefMatcher::parse("pf/*").unwrap();
    assert!(glob.matches("pf/drift"));
    assert!(!glob.matches("main"));
    assert!(
        glob.exact().is_none(),
        "a glob must be filtered client-side"
    );

    let regex = RefMatcher::parse("re:^release/.*$").unwrap();
    assert!(regex.matches("release/1.2"));
    assert!(!regex.matches("hotfix/release/1.2"));
    assert!(regex.exact().is_none());

    assert!(RefMatcher::parse("re:[unclosed").is_err());
}

/// A project path is URL-encoded; a numeric id is not.
#[test]
fn project_refs_are_url_safe() {
    assert_eq!(ProjectRef::Id(82468124).url_segment(), "82468124");
    assert_eq!(
        ProjectRef::Path("acme-corp/monorepo".into()).url_segment(),
        "acme-corp%2Fmonorepo"
    );
}

/// Job override patterns match literally unless prefixed `re:`, and the first
/// matching entry wins.
#[test]
fn job_overrides_match_in_file_order() {
    let loaded = parse(
        r#"
        [accounts.gl]
        token = { env = "T" }
        [[watches]]
        id = "x"
        account = "gl"
        project = 1
        [watches.jobs]
        "re:^verify:" = "warning"
        "verify:web_types" = "blocking"
        "#,
    )
    .unwrap();
    let rules = WatchRules::compile(&loaded.config.watches[0]).unwrap();

    assert_eq!(
        rules.job_override("verify:web_types"),
        Some(JobOverride::Warning),
        "the earlier regex wins over the later literal"
    );
    assert_eq!(rules.job_override("build"), None);

    assert!(
        Pattern::parse("re:^deploy:")
            .unwrap()
            .matches("deploy:origins")
    );
    assert!(
        !Pattern::parse("deploy:origins")
            .unwrap()
            .matches("deploy:origins-eu")
    );
}

/// Editing must not reflow the file. This is the contract the GUI depends on.
#[test]
fn an_edit_preserves_comments_and_key_order() {
    let original = support::example_config_raw();
    let mut editor = ConfigEditor::new(&original).expect("parses");

    editor
        .apply(&[
            Edit::Set {
                path: "watches.1.poll.live_secs".into(),
                value: EditValue::Integer(45),
            },
            Edit::AddWatch {
                watch: Box::new(
                    toml::from_str(
                        r#"
                        id = "releases"
                        account = "gitlab"
                        project = 82468124
                        ref = "re:^release/.*$"
                        role = "secondary"
                        "#,
                    )
                    .expect("watch parses"),
                ),
            },
        ])
        .expect("edits apply");

    let edited = editor.to_toml();

    // Every comment survives, verbatim.
    for line in original.lines().filter(|l| l.trim_start().starts_with('#')) {
        assert!(edited.contains(line), "comment lost: {line}");
    }
    for inline in [
        "# any number of accounts; self-managed instances welcome",
        "# names or \"re:\" regex; the first listed with a success = deployed",
        "# gate | warning | blocking | ignore",
        "# minijinja over the verdict view model",
    ] {
        assert!(edited.contains(inline), "trailing comment lost: {inline}");
    }

    // The first watch is byte-identical.
    let first_watch = |s: &str| {
        let start = s.find("id       = \"main-push\"").expect("first watch");
        let end = s
            .find("[[watches]]\nid = \"hourly\"")
            .expect("second watch");
        s[start..end].to_string()
    };
    assert_eq!(
        first_watch(&original),
        first_watch(&edited),
        "the untouched first watch must be byte-identical"
    );

    // The edit landed, and nothing else in that watch moved.
    let reloaded = parse(&edited).expect("the edited file still parses");
    assert_eq!(reloaded.config.watches[1].poll.live_secs, 45);
    assert_eq!(
        reloaded.config.watches[1].poll.idle_secs, 60,
        "its sibling key is untouched"
    );
    assert_eq!(reloaded.config.watches[0].poll.live_secs, 5);

    // The fourth watch exists, in order, after the three that were there.
    let ids: Vec<&str> = reloaded
        .config
        .watches
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(ids, ["main-push", "hourly", "preflights", "releases"]);
}

/// `unset` really removes a key, and `set` really replaces one, on a table key
/// that itself contains a colon.
#[test]
fn set_and_unset_reach_a_regex_keyed_override() {
    let mut editor = ConfigEditor::new(&support::example_config_raw()).unwrap();
    let before = parse(&editor.to_toml()).unwrap();
    assert_eq!(before.config.watches[0].jobs.entries().len(), 2);

    editor
        .unset("watches.0.jobs.re:^verify:web_coverage_full")
        .expect("unset runs");
    let after = parse(&editor.to_toml()).unwrap();
    assert_eq!(
        after.config.watches[0].jobs.entries(),
        [("kics-iac-sast".to_string(), JobOverride::Ignore)],
        "the override really went away"
    );

    editor
        .set(
            "watches.0.jobs.kics-iac-sast",
            &EditValue::String("warning".into()),
        )
        .expect("set runs");
    let after = parse(&editor.to_toml()).unwrap();
    assert_eq!(
        after.config.watches[0].jobs.entries()[0].1,
        JobOverride::Warning
    );
}

/// A key containing a dot is addressable by quoting it.
#[test]
fn a_quoted_path_segment_addresses_a_key_with_a_dot() {
    let raw = r#"
[accounts.gl]
token = { env = "T" }
[[watches]]
id = "x"
account = "gl"
project = 1
[watches.jobs]
"build.docs" = "ignore"
"#;
    let mut editor = ConfigEditor::new(raw).unwrap();
    editor
        .set(
            r#"watches.0.jobs."build.docs""#,
            &EditValue::String("warning".into()),
        )
        .expect("quoted segment resolves");
    let loaded = parse(&editor.to_toml()).unwrap();
    assert_eq!(
        loaded.config.watches[0].jobs.entries()[0].1,
        JobOverride::Warning
    );
}

/// Removing a watch by id, without having to know its index.
#[test]
fn a_watch_can_be_removed_by_id() {
    let mut editor = ConfigEditor::new(&support::example_config_raw()).unwrap();
    assert_eq!(editor.watch_ids(), ["main-push", "hourly", "preflights"]);
    assert!(editor.remove_watch("hourly").unwrap());
    assert!(
        !editor.remove_watch("hourly").unwrap(),
        "removing it twice is not an error"
    );
    assert_eq!(editor.watch_ids(), ["main-push", "preflights"]);
    assert_eq!(parse(&editor.to_toml()).unwrap().config.watches.len(), 2);
}

/// The JSON Schema covers the keys the GUI's settings form is built from.
#[test]
fn the_json_schema_describes_the_file() {
    let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
    let properties = schema["properties"].as_object().expect("object schema");
    for key in ["accounts", "watches", "icon", "verdict", "ui", "log"] {
        assert!(properties.contains_key(key), "schema is missing {key}");
    }
    let text = serde_json::to_string(&schema).unwrap();
    for key in [
        "deploy_markers",
        "sibling_failure",
        "post_deploy_failure",
        "only_when",
    ] {
        assert!(text.contains(key), "schema is missing {key}");
    }
}

/// ⛔ B5. The settings pane quotes a table key with `JSON.stringify`, so the
/// path it sends carries JSON escapes: `\\` for a backslash and `\"` for a
/// quote. Undoing the quotes without undoing those escapes rewrote the key
/// itself on every save.
///
/// The encoded column is the literal output of `JSON.stringify` for the key
/// beside it, taken from node, not from this crate's idea of what it emits.
#[test]
fn a_json_stringify_quoted_segment_decodes_to_the_key_it_encoded() {
    let cases = [
        // A regex override with an escaped dot: the case that grew a backslash
        // per save until it matched no job at all.
        (r#""re:^build\\.docs""#, r"re:^build\.docs"),
        // An account name with a dot, which is how the accounts pane addresses
        // `[accounts."gitlab.com"]` since it began quoting names too.
        (r#""gitlab.com""#, "gitlab.com"),
        // A quote inside a key used to unbalance the quoting, so every segment
        // after it stopped splitting.
        (r#""a\"b""#, "a\"b"),
        (r#""re:^deploy\\\\d""#, r"re:^deploy\\d"),
        // Control characters, which no sane key holds and which JSON.stringify
        // still escapes: the two sides must agree on these too, or the first
        // one pasted into a key field addresses a different key.
        (r#""a\tb\u001fc\bd""#, "a\tb\u{1f}c\u{8}d"),
    ];
    for (encoded, key) in cases {
        assert_eq!(
            split_path(&format!("watches.0.jobs.{encoded}")),
            ["watches", "0", "jobs", key],
            "{encoded} must decode to {key:?}"
        );
        assert_eq!(
            quote_path_segment(key),
            encoded,
            "the Rust encoder must emit exactly what JSON.stringify does"
        );
    }
    // An unquoted backslash is not an escape: a key with no dot is sent raw.
    assert_eq!(
        split_path(r"watches.0.jobs.re:^build\d+"),
        ["watches", "0", "jobs", r"re:^build\d+"]
    );
}

/// The same thing through the editor: the key written to the file is the key
/// the GUI meant, and it does not drift when the row is edited again.
#[test]
fn editing_a_key_with_a_backslash_twice_leaves_it_unchanged() {
    let raw = r#"
[accounts."gitlab.com"]
token = { env = "T" }
[[watches]]
id = "x"
account = "gitlab.com"
project = 1
[watches.jobs]
"re:^build\\.docs" = "ignore"
"#;
    let key = r"re:^build\.docs";
    let path = format!("watches.0.jobs.{}", quote_path_segment(key));

    let mut editor = ConfigEditor::new(raw).unwrap();
    for value in ["warning", "gate"] {
        editor
            .set(&path, &EditValue::String(value.into()))
            .expect("the quoted segment resolves");
        let loaded = parse(&editor.to_toml()).expect("the edited file still loads");
        assert_eq!(
            loaded.config.watches[0]
                .jobs
                .entries()
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>(),
            [key],
            "the override must still be keyed on the pattern the user wrote"
        );
    }
    assert_eq!(
        parse(&editor.to_toml()).unwrap().config.watches[0]
            .jobs
            .entries()[0]
            .1,
        JobOverride::Gate
    );
}

/// An account name with a dot is one segment, not two tables. Getting this
/// wrong writes `[accounts.gitlab]` with a `com` sub-table, and the watch that
/// names `gitlab.com` then has no account at all.
#[test]
fn a_dotted_account_name_is_one_segment() {
    let raw = r#"
[accounts."gitlab.com"]
token = { env = "T" }
[[watches]]
id = "x"
account = "gitlab.com"
project = 1
"#;
    let mut editor = ConfigEditor::new(raw).unwrap();
    editor
        .set(
            &format!("accounts.{}.base_url", quote_path_segment("gitlab.com")),
            &EditValue::String("https://gitlab.example".into()),
        )
        .expect("the quoted account name resolves");
    let loaded = parse(&editor.to_toml()).expect("the edited file still loads");
    assert_eq!(
        loaded.config.accounts.len(),
        1,
        "no second account appeared"
    );
    assert_eq!(
        loaded.config.accounts["gitlab.com"].base_url,
        "https://gitlab.example"
    );
}

/// ⛔ M15. A key bridgewatch does not know is a warning, not a fatal error: a
/// file written by a newer build has to still run here, and `deny_unknown_fields`
/// on every struct meant one unrecognised line took the whole app down to
/// "invalid configuration".
#[test]
fn an_unknown_key_is_a_warning_and_the_rest_of_the_file_still_loads() {
    let raw = r#"
[accounts.gl]
token = { env = "T" }
retry_forever = true

[[watches]]
id = "x"
account = "gl"
project = 1
future_key = "whatever"

[watches.poll]
live_secs = 7
nope = 2

[watches.jobs]
"kics-iac-sast" = "ignore"
"#;
    let loaded = parse(raw).expect("an unknown key must not stop the file loading");

    // The parts it DID understand are all there.
    assert_eq!(loaded.config.watches[0].poll.live_secs, 7);
    assert_eq!(loaded.config.watches[0].jobs.entries().len(), 1);
    assert_eq!(loaded.config.accounts.len(), 1);

    let paths: Vec<&str> = loaded.warnings.iter().map(|w| w.path.as_str()).collect();
    for expected in [
        "accounts.gl.retry_forever",
        "watches.0.future_key",
        "watches.0.poll.nope",
    ] {
        assert!(
            paths.contains(&expected),
            "{expected} missing from {paths:?}"
        );
    }
    let one = loaded
        .warnings
        .iter()
        .find(|w| w.path == "watches.0.future_key")
        .unwrap();
    assert_eq!(one.severity, Severity::Warning);
    assert!(
        one.message.contains("unknown field") && one.message.contains("ignored"),
        "the message keeps serde's own 'expected one of' list: {}",
        one.message
    );
    // And it points at the line the key is on, in the file as the user wrote it.
    let (line, _col) = one.line_col(raw).expect("a span was located");
    assert_eq!(
        raw.lines().nth(line - 1).unwrap().trim(),
        r#"future_key = "whatever""#
    );
}

/// A whole unknown TABLE goes the same way, header and contents together.
#[test]
fn an_unknown_table_is_dropped_whole() {
    let raw = r#"
[accounts.gl]
token = { env = "T" }

[[watches]]
id = "x"
account = "gl"
project = 1

[watches.retry]
max = 3
backoff = "linear"
"#;
    let loaded = parse(raw).expect("loads");
    assert_eq!(
        loaded
            .warnings
            .iter()
            .filter(|w| w.path == "watches.0.retry")
            .count(),
        1,
        "one warning for the table, not one per key inside it: {:?}",
        loaded.warnings
    );
}

/// ⛔ Leniency stops at unknown KEYS. A value bridgewatch cannot interpret is
/// still fatal, because continuing would mean guessing which of four policies
/// the user meant — and a syntax error still reports where it is (M16).
#[test]
fn a_bad_value_and_a_syntax_error_are_still_fatal() {
    let bad_variant = parse(
        r#"
[accounts.gl]
token = { env = "T" }
[[watches]]
id = "x"
account = "gl"
project = 1
sibling_failure = "explode"
"#,
    )
    .expect_err("an unknown enum variant cannot be guessed");
    assert!(
        matches!(bad_variant, config::ConfigError::Parse { .. }),
        "got {bad_variant:?}"
    );

    let raw = "[accounts.gl]\ntoken = { env = \"T\" }\nthis is not toml\n";
    let err = parse(raw).expect_err("a syntax error is fatal");
    let config::ConfigError::Parse { span, message, .. } = err else {
        panic!("expected Parse");
    };
    assert!(span.is_some(), "and it carries a span: {message}");
}

/// ⚠ M15's other half: values whose TYPE is right and whose CONTENT names
/// something that does not exist. Each of these used to load in silence and
/// then behave as though the key had never been written.
#[test]
fn values_that_name_nothing_are_reported() {
    let loaded = parse(
        r#"
[accounts.gl]
base_url = "http://gitlab.example.com"
token = { env = "T" }

[icon]
mode = "sparkly"
states = { failed = "octagon", exploded = "check", deployed = "chek" }

[log]
level = "shout"

[ui.popover]
width = 0

[[watches]]
id = "a"
account = "gl"
project = 1
ref = "re:main"
sources = ["push", "telepathy"]
deploy_markers = ["deploy:origins"]
show = { max_rows = 0 }

[[watches]]
id = "b"
account = "gl"
project = 1
deploy_markers = ["deploy:origins"]
"#,
    )
    .expect("every one of these is a warning, not an error");

    let by_path: Vec<(&str, &str)> = loaded
        .warnings
        .iter()
        .map(|w| (w.path.as_str(), w.message.as_str()))
        .collect();
    let says = |path: &str, needle: &str| {
        by_path
            .iter()
            .any(|(p, m)| *p == path && m.contains(needle))
    };

    assert!(says("accounts.gl.base_url", "plain http"), "{by_path:?}");
    assert!(says("icon.mode", "not an icon mode"), "{by_path:?}");
    assert!(
        says("icon.states.exploded", "not an icon state"),
        "{by_path:?}"
    );
    assert!(
        !by_path.iter().any(|(p, _)| *p == "icon.states.failed"),
        "a real state with a real glyph is not flagged: {by_path:?}"
    );
    assert!(
        says("icon.states.deployed", "not a builtin glyph"),
        "a misspelt glyph draws the question mark, so it is said: {by_path:?}"
    );
    assert!(
        !by_path
            .iter()
            .any(|(p, m)| *p == "icon.states.exploded" && m.contains("builtin glyph")),
        "`check` is a real glyph; only the state name is wrong: {by_path:?}"
    );
    assert!(says("log.level", "not a log level"), "{by_path:?}");
    assert!(says("ui.popover", "nothing to draw in"), "{by_path:?}");
    assert!(
        says("watches.0.sources.1", "not a pipeline source"),
        "{by_path:?}"
    );
    assert!(
        !by_path.iter().any(|(p, _)| *p == "watches.0.sources.0"),
        "`push` is a real source: {by_path:?}"
    );
    assert!(
        says("watches.0.show.max_rows", "contributes no rows"),
        "{by_path:?}"
    );
    assert!(says("watches.0.ref", "unanchored"), "{by_path:?}");
    assert!(says("watches", "2 watches are primary"), "{by_path:?}");
}

/// The http:// warning is about the token crossing a network, so loopback is
/// exempt: a local instance is a normal way to develop and the credential never
/// leaves the machine.
#[test]
fn a_loopback_base_url_is_not_warned_about() {
    let warned = |base: &str| -> bool {
        let loaded = parse(&format!(
            r#"
[accounts.gl]
base_url = "{base}"
token = {{ env = "T" }}
[[watches]]
id = "x"
account = "gl"
project = 1
deploy_markers = ["d"]
"#
        ))
        .expect("loads");
        loaded
            .warnings
            .iter()
            .any(|w| w.message.contains("plain http"))
    };
    // ⚠ `[::1]:8080` used to be split on `:` and read as the host `[`, so the
    // one IPv6 loopback spelling with a port was warned about.
    for base in [
        "http://localhost:8080",
        "http://127.0.0.1",
        "http://127.0.0.2:3000/gitlab",
        "http://[::1]:8080",
        "http://[::1]",
    ] {
        assert!(
            !warned(base),
            "{base} is loopback and should not be warned about"
        );
    }
    // Prefix look-alikes are somebody else's machine.
    for base in [
        "http://127.0.0.1.example.com",
        "http://localhost.example.com",
        "http://gitlab.example.com",
    ] {
        assert!(
            warned(base),
            "{base} is not loopback and must be warned about"
        );
    }
}

/// An anchored ref regex says what it means, so it is left alone.
#[test]
fn an_anchored_ref_regex_is_not_warned_about() {
    let loaded = parse(
        r#"
[accounts.gl]
token = { env = "T" }
[[watches]]
id = "x"
account = "gl"
project = 1
ref = "re:^release/.*$"
deploy_markers = ["d"]
"#,
    )
    .expect("loads");
    assert!(
        !loaded.warnings.iter().any(|w| w.path == "watches.0.ref"),
        "{:?}",
        loaded.warnings
    );
}

/// ⛔ Lo12. Two token sources in one table used to load as whichever serde saw
/// first, so a user who had moved their token into bridgewatch's own keychain
/// entry was still sending the environment variable.
#[test]
fn a_token_table_naming_two_sources_is_refused() {
    let err = parse(
        r#"
[accounts.gl]
token = { env = "T", own = true }
[[watches]]
id = "x"
account = "gl"
project = 1
"#,
    )
    .expect_err("bridgewatch must not pick one");
    let message = err.to_string();
    assert!(
        message.contains("exactly one of") && message.contains("env") && message.contains("own"),
        "the error names both: {message}"
    );

    // And a single source still loads, including the empty-user keyring form.
    let loaded = parse(
        r#"
[accounts.gl]
token = { keyring = { service = "glab:gitlab.com:token", user = "" } }
[[watches]]
id = "x"
account = "gl"
project = 1
deploy_markers = ["d"]
"#,
    )
    .expect("one source is fine");
    assert_eq!(
        loaded.config.accounts["gl"].token,
        TokenSource::Keyring {
            service: "glab:gitlab.com:token".into(),
            user: String::new()
        }
    );
}

/// ⛔ M14. "Add account" in the settings pane sets
/// `accounts."<name>".base_url` on a file whose `accounts` table exists only
/// through `[accounts.gitlab]`. The new key used to go in as an inline table on
/// that IMPLICIT parent, which forced toml_edit to print an `[accounts]` header
/// for it — at the parent's position, above the file's opening comment block, so
/// the file's own introduction ended up under a header it does not describe.
#[test]
fn adding_an_account_leaves_the_leading_comment_block_on_top() {
    let raw = support::example_config_raw();
    let mut editor = ConfigEditor::new(&raw).unwrap();
    editor
        .apply(&[
            Edit::Set {
                path: format!("accounts.{}.base_url", quote_path_segment("self.hosted")),
                value: EditValue::String("https://gitlab.example".into()),
            },
            Edit::Set {
                path: format!("accounts.{}.token.own", quote_path_segment("self.hosted")),
                value: EditValue::Boolean(true),
            },
        ])
        .expect("the edits apply");
    let out = editor.to_toml();

    let first_line = raw.lines().next().unwrap();
    assert_eq!(
        out.lines().next().unwrap(),
        first_line,
        "the file still opens with its own comment:\n{out}"
    );
    assert!(
        !out.lines().any(|l| l.trim() == "[accounts]"),
        "no bare [accounts] header was invented:\n{out}"
    );
    assert!(
        out.starts_with(raw.split("[[watches]]").next().unwrap()),
        "every byte before the first watch is untouched:\n{out}"
    );

    let loaded = parse(&out).expect("the edited file loads");
    assert_eq!(loaded.config.accounts.len(), 2);
    let added = &loaded.config.accounts["self.hosted"];
    assert_eq!(added.base_url, "https://gitlab.example");
    assert_eq!(added.token, TokenSource::Own(true));

    // The root has no header either, so a table the file never wrote goes at
    // the end as a table of its own, not as a line above the first comment.
    let raw = "# my config\n\n[accounts.gl]\ntoken = { env = \"T\" }\n";
    let mut editor = ConfigEditor::new(raw).unwrap();
    editor
        .set("ui.popover.width", &EditValue::Integer(500))
        .expect("a missing table is created");
    let out = editor.to_toml();
    assert!(out.starts_with(raw), "appended, not prepended:\n{out}");
    assert_eq!(parse(&out).unwrap().config.ui.popover.width, 500);
}

/// A theme directory may name glyphs the builtin set does not have, so the
/// glyph check stands down there rather than warning about the user's own art.
#[test]
fn a_custom_theme_may_name_its_own_glyphs() {
    let loaded = parse(
        r#"
[accounts.gl]
token = { env = "T" }
[icon]
theme = "~/my-icons"
states = { failed = "flame" }
[[watches]]
id = "x"
account = "gl"
project = 1
deploy_markers = ["d"]
"#,
    )
    .expect("loads");
    assert!(
        !loaded
            .warnings
            .iter()
            .any(|w| w.path == "icon.states.failed"),
        "{:?}",
        loaded.warnings
    );
    assert!(config::BUILTIN_GLYPHS.contains(&"check-outline"));
    assert_eq!(config::BUILTIN_GLYPHS.len(), 16);
}

/// ⚠ Lo2. Two matching rules that surprise people are documented where the
/// user meets them, in the validation output, rather than changed: `re:` is
/// unanchored (asserted above), and a glob's `*` crosses `/`. Both behaviours
/// are pinned here so the diagnostic cannot drift from what actually matches.
#[test]
fn a_glob_star_that_crosses_a_slash_is_said_and_pf_star_is_not() {
    let star = RefMatcher::parse("*/main").unwrap();
    assert!(star.matches("a/main"));
    assert!(star.matches("a/b/main"), "`*` crosses `/`");
    let unanchored = RefMatcher::parse("re:main").unwrap();
    assert!(unanchored.matches("not-main"), "`re:` is unanchored");

    let warned = |pattern: &str| {
        parse(&format!(
            r#"
[accounts.gl]
token = {{ env = "T" }}
[[watches]]
id = "x"
account = "gl"
project = 1
ref = "{pattern}"
deploy_markers = ["d"]
"#
        ))
        .expect("loads")
        .warnings
        .into_iter()
        .any(|w| w.path == "watches.0.ref" && w.message.contains("also matches `/`"))
    };
    assert!(warned("*/main"));
    assert!(warned("release/*/hotfix"));
    assert!(
        !warned("pf/*"),
        "a trailing star reaching nested refs is intended"
    );
    assert!(!warned("main"));
}

// ---------------------------------------------------------------------------
// ui.jobs and watches.show.jobs
// ---------------------------------------------------------------------------

const TWO_WATCHES: &str = r#"
[accounts.gl]
token = { env = "T" }

[[watches]]
id = "a"
account = "gl"
project = 1

[[watches]]
id = "b"
account = "gl"
project = 2
show = { max_rows = 3 }   # keep this comment
"#;

/// `ui.jobs` defaults to `all`; `show.jobs` defaults to "not set", and the
/// effective mode is the watch's own when it has one, the global one otherwise.
#[test]
fn the_effective_jobs_mode_is_the_watch_s_then_the_global() {
    use bridgewatch_core::config::JobsMode;

    let loaded = parse(TWO_WATCHES).unwrap();
    assert_eq!(loaded.config.ui.jobs, JobsMode::All, "the 0.1.0 default");
    assert_eq!(loaded.config.watches[0].show.jobs, None);

    let mut config = loaded.config;
    let ui = config.ui.clone();
    assert_eq!(config.watches[0].effective_jobs(&ui), JobsMode::All);

    config.ui.jobs = JobsMode::Failures;
    config.watches[1].show.jobs = Some(JobsMode::All);
    let ui = config.ui.clone();
    assert_eq!(config.watches[0].effective_jobs(&ui), JobsMode::Failures);
    assert_eq!(config.watches[1].effective_jobs(&ui), JobsMode::All);

    config.ui.jobs = JobsMode::All;
    config.watches[1].show.jobs = Some(JobsMode::Failures);
    let ui = config.ui.clone();
    assert_eq!(config.watches[0].effective_jobs(&ui), JobsMode::All);
    assert_eq!(config.watches[1].effective_jobs(&ui), JobsMode::Failures);
}

/// A mode bridgewatch does not know is refused, and the message names both
/// choices, so the fix is in the error.
#[test]
fn an_unknown_jobs_mode_is_refused_naming_the_choices() {
    for raw in [
        format!("{TWO_WATCHES}\n[ui]\njobs = \"some\"\n"),
        TWO_WATCHES.replace(
            "show = { max_rows = 3 }",
            "show = { max_rows = 3, jobs = \"everything\" }",
        ),
    ] {
        let err = parse(&raw).expect_err("an unknown mode cannot be guessed");
        let config::ConfigError::Parse { message, .. } = &err else {
            panic!("expected Parse, got {err:?}");
        };
        assert!(
            message.contains("failures") && message.contains("all"),
            "{message}"
        );
    }
}

/// Both keys are in the schema the settings form is built from, as the two
/// words and with `all` as the default.
#[test]
fn the_jobs_modes_are_in_the_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
    let defs = &schema["$defs"];
    let mode = serde_json::to_string(&defs["JobsMode"]).unwrap();
    assert!(
        mode.contains("\"failures\"") && mode.contains("\"all\""),
        "{mode}"
    );
    assert!(
        defs["UiConfig"]["properties"]["jobs"].is_object(),
        "{}",
        defs["UiConfig"]
    );
    assert_eq!(schema["properties"]["ui"]["default"]["jobs"], "all");
    assert!(defs["ShowConfig"]["properties"]["jobs"].is_object());
}

/// The settings pane writes both keys through the editor. They land where a
/// person would have typed them, the comment on the `show` line survives, and
/// the file reloads meaning what was asked. Unsetting the override hands the
/// watch back to the global mode.
#[test]
fn jobs_mode_edits_round_trip() {
    use bridgewatch_core::config::JobsMode;

    let mut editor = ConfigEditor::new(TWO_WATCHES).unwrap();
    editor
        .apply(&[
            Edit::Set {
                path: "ui.jobs".into(),
                value: EditValue::String("failures".into()),
            },
            Edit::SetWatch {
                id: "b".into(),
                path: "show.jobs".into(),
                value: EditValue::String("all".into()),
            },
        ])
        .unwrap();
    let edited = editor.to_toml();
    assert!(
        edited.contains("show = { max_rows = 3, jobs = \"all\" }   # keep this comment"),
        "{edited}"
    );

    let loaded = parse(&edited).expect("the edited file loads");
    let ui = loaded.config.ui.clone();
    assert_eq!(ui.jobs, JobsMode::Failures);
    assert_eq!(
        loaded.config.watches[0].effective_jobs(&ui),
        JobsMode::Failures
    );
    assert_eq!(loaded.config.watches[1].effective_jobs(&ui), JobsMode::All);

    // A second pass over the edited text is a no-op in meaning.
    let again = ConfigEditor::new(&edited).unwrap().to_toml();
    assert_eq!(again, edited, "toml_edit round-trips it byte for byte");

    let mut editor = ConfigEditor::new(&edited).unwrap();
    editor
        .apply(&[Edit::Unset {
            path: "watches.1.show.jobs".into(),
        }])
        .unwrap();
    let loaded = parse(&editor.to_toml()).unwrap();
    assert_eq!(loaded.config.watches[1].show.jobs, None);
    assert_eq!(
        loaded.config.watches[1].effective_jobs(&loaded.config.ui),
        JobsMode::Failures,
        "back on the global mode"
    );
}

/// A watch added from the settings pane with no override of its own must not
/// write one: an explicit `jobs = "all"` would pin it against a later change
/// to `ui.jobs`.
#[test]
fn a_new_watch_without_a_mode_does_not_pin_one() {
    let mut editor = ConfigEditor::new(TWO_WATCHES).unwrap();
    let watch: bridgewatch_core::config::Watch =
        toml::from_str("id = \"c\"\naccount = \"gl\"\nproject = 3\n").unwrap();
    editor
        .apply(&[Edit::AddWatch {
            watch: Box::new(watch),
        }])
        .unwrap();
    let edited = editor.to_toml();
    let show_line = edited
        .lines()
        .rfind(|l| l.starts_with("show = "))
        .expect("the new watch's show table");
    assert!(!show_line.contains("jobs"), "{show_line}");
    assert_eq!(parse(&edited).unwrap().config.watches[2].show.jobs, None);
}

/// The log-filter rule both shells share: a non-empty `RUST_LOG` wins whole, a
/// blank one counts as unset, and `[log].level` reaches only the crates it is
/// given. The CLI and the app pass different crate names and must otherwise
/// behave identically, which is why the rule lives here rather than twice.
#[test]
fn the_log_directive_scopes_the_file_level_and_ignores_a_blank_rust_log() {
    let app = &["bridgewatch_app", "bridgewatch_core"];
    let cli = &["bridgewatch", "bridgewatch_core"];

    assert_eq!(
        config::log_directive(Some("trace"), Some("off"), app),
        "trace"
    );
    assert_eq!(
        config::log_directive(Some(" \t "), Some("info"), app),
        config::log_directive(None, Some("info"), app)
    );
    assert_eq!(
        config::log_directive(None, Some("debug"), app),
        "warn,bridgewatch_app=debug,bridgewatch_core=debug"
    );
    assert_eq!(
        config::log_directive(None, Some("DEBUG"), cli),
        "warn,bridgewatch=debug,bridgewatch_core=debug"
    );
    // Quieter than `warn` is quieter everywhere; nothing, or a level the file
    // should not have carried, is `warn`.
    for level in ["error", "off"] {
        assert_eq!(config::log_directive(None, Some(level), app), level);
    }
    assert_eq!(config::log_directive(None, None, app), "warn");
    assert_eq!(config::log_directive(None, Some("loud"), app), "warn");
    // Every level the file may name is either scoped or a bare level word.
    for level in config::LOG_LEVELS {
        let directive = config::log_directive(None, Some(level), app);
        assert!(
            config::LOG_LEVELS.contains(&directive.as_str()) || directive.starts_with("warn,"),
            "{level}: {directive}"
        );
    }
}

/// `describe_load` is the line both front ends print when they have read a
/// configuration, and the log level is part of it: `[log].level` is read from
/// the file being reported, but `RUST_LOG` beats it, and nothing else would say
/// so.
///
/// Lives here, beside `log_directive`, because it logs nothing itself: it
/// builds a string. `tests/logging.rs` is reserved for tests that capture
/// output, and one that does not would poison their subscriber.
#[test]
fn the_config_line_names_the_file_the_counts_and_the_level() {
    let config = support::config_with(&[] as &[Edit]);
    let line = config::describe_load(
        "using /tmp/bw/config.toml",
        Some(&config),
        "warn,bridgewatch_core=info",
    );
    assert!(line.contains("/tmp/bw/config.toml"), "{line}");
    assert!(line.contains("3 watch(es)"), "{line}");
    assert!(line.contains("1 account(s)"), "{line}");
    assert!(line.contains("warn,bridgewatch_core=info"), "{line}");

    let broken = config::describe_load("reloaded /tmp/bw/config.toml", None, "");
    assert!(broken.contains("does not load"), "{broken}");
    assert!(broken.contains("nothing is watched"), "{broken}");
}

/// Colour is for a person reading a terminal, and both shells share the rule so
/// that one of them cannot quietly keep escaping a pipe.
///
/// ⚠ `NO_COLOR` is "present and not empty", per the convention, which is NOT
/// how `RUST_LOG` is read two functions up: a blank `RUST_LOG` counts as unset
/// because it is a leftover in a shell profile, while a blank `NO_COLOR` is
/// explicitly defined by the convention to mean nothing at all.
#[test]
fn ansi_is_for_a_terminal_and_no_color_overrules_it() {
    assert!(config::use_ansi(true, None));
    assert!(!config::use_ansi(false, None));

    // Set and non-empty wins over a terminal, whatever the value says.
    assert!(!config::use_ansi(true, Some("1")));
    assert!(!config::use_ansi(true, Some("0")));
    assert!(!config::use_ansi(true, Some("false")));
    assert!(!config::use_ansi(false, Some("1")));

    // Set but empty is not set.
    assert!(config::use_ansi(true, Some("")));
    assert!(!config::use_ansi(false, Some("")));
}

/// The shipped example states the global mode, so the key is discoverable.
#[test]
fn the_example_states_the_jobs_mode() {
    let loaded = parse(&support::example_config_raw()).unwrap();
    assert_eq!(
        loaded.config.ui.jobs,
        bridgewatch_core::config::JobsMode::All
    );
    assert!(support::example_config_raw().contains("jobs = \"all\""));
}
