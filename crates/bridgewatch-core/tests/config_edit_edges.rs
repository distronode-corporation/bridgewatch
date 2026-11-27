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

/// TOML 1.1 lets an inline table carry a comment; a key appended after it
/// goes in without moving the comment's text onto the new value.
#[test]
fn a_key_appended_after_a_commented_inline_value_keeps_the_comment_where_it_was() {
    let raw = "[accounts.gl]\nshow = { max_rows = 3 # rows\n}\n";
    let mut e = ConfigEditor::new(raw).expect("TOML 1.1 parses");
    e.set("accounts.gl.show.settled", &EditValue::Integer(2))
        .unwrap();
    let out = e.to_toml();
    assert!(out.contains("# rows"), "{out}");
    assert!(out.contains("settled = 2"), "{out}");
    ConfigEditor::new(&out).expect("the result still parses");
}

/// A watch edit names a watch by id; one that is not there is said, and a
/// file whose `watches` is not a list of tables has none to find.
#[test]
fn a_watch_edit_on_an_unknown_id_is_no_such_watch() {
    let mut e = editor();
    let edit = Edit::SetWatch {
        id: "zzz".into(),
        path: "ref".into(),
        value: EditValue::String("main".into()),
    };
    assert!(matches!(e.apply(&[edit]), Err(EditError::NoSuchWatch(id)) if id == "zzz"));
    let odd = ConfigEditor::new("watches = 1\n").unwrap();
    assert_eq!(odd.watch_index("a"), None);
}

#[test]
fn a_file_with_no_watches_has_none_to_find() {
    assert_eq!(ConfigEditor::new("").unwrap().watch_index("a"), None);
}

/// A watch from the settings window is JSON, which can carry an interval no
/// TOML integer can hold: adding it is an error, never a panic.
#[test]
fn a_watch_with_an_interval_toml_cannot_hold_is_refused() {
    let raw = "[accounts.gl]\ntoken = { env = \"T\" }\n[[watches]]\nid = \"a\"\naccount = \"gl\"\nproject = 1\n";
    let mut watch = bridgewatch_core::config::parse_lenient(raw, std::path::Path::new("t.toml"))
        .unwrap()
        .0
        .watches
        .remove(0);
    watch.poll.live_secs = u64::MAX;
    let error = editor().add_watch(&watch).unwrap_err();
    assert!(matches!(error, EditError::Serialize(_)), "{error:?}");
}
