//! `tauri.linux.conf.json` differs from `tauri.conf.json` in one key.
//!
//! Tauri merges the platform file over the base with a JSON merge patch, and a
//! merge patch replaces an array whole, so the Linux file carries every window
//! again. It exists for one reason: on GTK a window that is not resizable
//! keeps the size it was created with, so the popover could neither fit its
//! content nor take `[ui].popover.width` (`commands::fit_popover`). Any other
//! difference is a window edited in one file and not the other.

use serde_json::Value;

fn parse(raw: &str, name: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("{name} parses: {e}"))
}

#[test]
fn the_linux_windows_are_the_base_windows_with_a_resizable_popover() {
    let base = parse(include_str!("../tauri.conf.json"), "tauri.conf.json");
    let linux = parse(
        include_str!("../tauri.linux.conf.json"),
        "tauri.linux.conf.json",
    );

    let mut expected = base["app"]["windows"].clone();
    let popover = expected
        .as_array_mut()
        .expect("a windows list")
        .iter_mut()
        .find(|w| w["label"] == "popover")
        .expect("a popover window");
    assert_eq!(
        popover["resizable"], false,
        "the base popover is not resizable"
    );
    popover["resizable"] = Value::Bool(true);

    assert_eq!(linux["app"]["windows"], expected);
}

#[test]
fn the_linux_file_overrides_the_windows_and_nothing_else() {
    let linux = parse(
        include_str!("../tauri.linux.conf.json"),
        "tauri.linux.conf.json",
    );
    let mut top: Vec<&str> = linux
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    top.sort_unstable();
    assert_eq!(top, ["$schema", "app"]);
    let app: Vec<&str> = linux["app"]
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(app, ["windows"]);
}
