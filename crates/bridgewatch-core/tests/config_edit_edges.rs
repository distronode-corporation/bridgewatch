// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! The config editor on paths `config.rs` does not take: a float, a removal
//! through `apply`, a path that walks into an array or a value that is not a
//! table, unsetting inside arrays and inline tables, and the escapes a quoted
//! path segment can carry.

use bridgewatch_core::config::edit::{
    ConfigEditor, Edit, EditError, EditValue, quote_path_segment, split_path,
};

const FILE: &str = r#"[ui]
theme = "auto"

[accounts.gl]
token = { env = "T" }
show = { max_rows = 3, inner = { deep = 1 } }

[[watches]]
id = "a"
account = "gl"
project = 1

[[watches]]
id = "b"
account = "gl"
project = 2
"#;

fn editor() -> ConfigEditor {
    ConfigEditor::new(FILE).expect("parses")
}

#[test]
fn a_float_is_written_as_a_float() {
    let mut e = editor();
    e.set("ui.scale", &EditValue::Float(1.5)).unwrap();
    assert!(e.to_toml().contains("scale = 1.5"), "{}", e.to_toml());
}

#[test]
fn apply_removes_a_watch_by_id() {
    let mut e = editor();
    e.apply(&[Edit::RemoveWatch { id: "a".into() }]).unwrap();
    assert_eq!(e.watch_ids(), ["b"]);
}

#[test]
fn a_set_into_an_array_of_tables_needs_an_index_in_range_and_a_key_below_it() {
    let mut e = editor();
    let one = EditValue::Integer(1);
    assert!(matches!(
        e.set("watches.first.id", &one),
        Err(EditError::NotATable { .. })
    ));
    assert!(
        matches!(e.set("watches.9.id", &one), Err(EditError::NoSuchKey { path }) if path == "watches.9")
    );
    assert!(matches!(
        e.set("watches.0", &one),
        Err(EditError::NotATable { .. })
    ));
    e.set("watches.1.ref", &EditValue::String("main".into()))
        .unwrap();
    assert!(e.to_toml().contains("ref = \"main\""));
}

#[test]
fn a_set_below_a_scalar_is_refused() {
    let mut e = editor();
    let one = EditValue::Integer(1);
    assert!(matches!(
        e.set("ui.theme.colour", &one),
        Err(EditError::NotATable { .. })
    ));
    // Below a scalar inside an inline table, too.
    assert!(matches!(
        e.set("accounts.gl.show.max_rows.x", &one),
        Err(EditError::NotATable { .. })
    ));
}

#[test]
fn unset_walks_arrays_and_inline_tables_and_says_whether_it_removed_anything() {
    let mut e = editor();
    assert!(!e.unset("watches.first.id").unwrap());
    assert!(!e.unset("watches.9.id").unwrap());
    assert!(!e.unset("ui.theme.colour").unwrap());
    assert!(!e.unset("accounts.gl.show.max_rows.x").unwrap());
    assert!(e.unset("accounts.gl.show.inner.deep").unwrap());
    assert!(e.unset("watches.0").unwrap());
    assert_eq!(e.watch_ids(), ["b"]);
}

#[test]
fn a_quoted_segment_carries_every_escape_both_ways() {
    let key = "a\nb\rc\td\u{8}e\u{c}f\"g\\h";
    let quoted = quote_path_segment(key);
    assert_eq!(quoted, r#""a\nb\rc\td\be\ff\"g\\h""#);
    assert_eq!(split_path(&format!("accounts.{quoted}")), ["accounts", key]);
    // A path that ends inside an escape keeps the backslash.
    assert_eq!(split_path(r#""abc\"#), [r"abc\"]);
}
