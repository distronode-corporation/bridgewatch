//! The committed `src/lib/config.schema.json` must be current.
//!
//! The other half of the parity gate lives in `src/lib/settings/registry.test.ts`,
//! which checks that every leaf key in that file has a settings control and that
//! every control has a key. That half runs under `npm test` and needs no Rust.
//! This half needs the types, so it lives here.
//!
//! ⚠ This does not shell out to `bridgewatch config schema`. It calls the same
//! `schemars::schema_for!(Config)` through the same `serde_json::to_string_pretty`
//! that `ConfigAction::Schema` does — the CLI subcommand is four lines and this
//! reproduces all four — which keeps the test to milliseconds and means it runs
//! without a built CLI binary. ⚠ The shortcut is only sound while those four
//! lines match: if `config schema` grows a flag or a different serialiser, the
//! CLI and this test can drift and neither would fail.

use bridgewatch_core::config::Config;

/// The committed artifact the frontend imports.
const COMMITTED: &str = include_str!("../../src/lib/config.schema.json");

/// Exactly what `bridgewatch config schema` writes to stdout, including the
/// trailing newline `println!` adds.
fn current() -> String {
    let schema = schemars::schema_for!(Config);
    format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("schema is serialisable")
    )
}

#[test]
fn committed_schema_is_current() {
    let current = current();
    if COMMITTED != current {
        // Not `assert_eq!`: a 20 KB diff of JSON in a test failure is unreadable
        // and the fix is one command.
        let (line, expected, found) = first_difference(&current, COMMITTED);
        panic!(
            "src/lib/config.schema.json is stale.\n\
             Regenerate it:\n\
             \x20   cargo run -p bridgewatch-cli -- config schema > src/lib/config.schema.json\n\n\
             First difference at line {line}:\n\
             \x20 expected: {expected}\n\
             \x20 found:    {found}\n\
             (committed {} bytes, current {} bytes)",
            COMMITTED.len(),
            current.len(),
        );
    }
}

#[test]
fn the_schema_still_describes_the_keys_the_settings_pane_binds_to() {
    // A handful of load-bearing paths, spelled the way the TypeScript walker
    // sees them. If schemars ever changes how it renders a map or an array of
    // tables, this fails HERE with a name, rather than in a vitest set
    // difference that just says a path is missing.
    let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
    let defs = &schema["$defs"];

    assert!(
        schema["properties"]["accounts"]["additionalProperties"]["$ref"].is_string(),
        "accounts is no longer a dictionary of Account"
    );
    assert!(
        schema["properties"]["watches"]["items"]["$ref"].is_string(),
        "watches is no longer an array of Watch"
    );
    assert!(
        defs["Watch"]["properties"]["jobs"].is_object(),
        "Watch no longer has a jobs table"
    );
    assert!(
        defs["Account"]["properties"]["token"].is_object(),
        "Account no longer has a token"
    );
    // The token is a union, and the walker stops at one. If it ever gains
    // `properties` the walker would descend into variant-specific keys.
    let token_def = &defs["TokenSource"];
    assert!(
        token_def["oneOf"].is_array() || token_def["anyOf"].is_array(),
        "TokenSource is no longer a union; the schema walker would descend into it"
    );
}

/// Line number and the two texts at the first line that differs.
fn first_difference(a: &str, b: &str) -> (usize, String, String) {
    let mut left = a.lines();
    let mut right = b.lines();
    let mut n = 0;
    loop {
        n += 1;
        match (left.next(), right.next()) {
            (None, None) => return (n, "<end of file>".into(), "<end of file>".into()),
            (x, y) if x == y => continue,
            (x, y) => {
                return (
                    n,
                    x.unwrap_or("<end of file>").to_string(),
                    y.unwrap_or("<end of file>").to_string(),
                );
            }
        }
    }
}
