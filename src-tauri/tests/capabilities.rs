//! The webview's capability stays events-only.
//!
//! Every plugin is driven from Rust. A plugin permission granted to the
//! webview is a power handed to any script running there for no feature that
//! needs it (Lo10), and an opener scope in particular is the wrong place for
//! the link check, which needs the configured accounts (M18, `links.rs`).

#[test]
fn the_webview_is_granted_events_and_nothing_else() {
    let raw = include_str!("../capabilities/default.json");
    let json: serde_json::Value = serde_json::from_str(raw).expect("capability parses");
    let permissions = json["permissions"].as_array().expect("a permissions list");
    let names: Vec<String> = permissions
        .iter()
        .map(|p| match p {
            serde_json::Value::String(s) => s.clone(),
            other => other["identifier"].as_str().unwrap_or("?").to_string(),
        })
        .collect();
    assert_eq!(names, ["core:event:default"], "{names:?}");
}

#[test]
fn the_frontend_does_not_import_a_plugin_it_has_no_permission_for() {
    let ipc = include_str!("../../src/lib/ipc.ts");
    assert!(
        !ipc.contains("@tauri-apps/plugin-"),
        "ipc.ts imports a plugin; route it through a Rust command instead"
    );
}
