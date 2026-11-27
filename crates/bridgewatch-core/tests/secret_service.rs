// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! The quiet read against a real Secret Service (Linux).
//!
//! CI's `coverage` job runs the tests inside `dbus-run-session` with an
//! unlocked gnome-keyring and sets `BRIDGEWATCH_TEST_SECRET_SERVICE=1`.
//! Elsewhere (a desktop, the other Linux legs) there may be no Secret Service,
//! or a person's own keyring, so without that variable the test does nothing.
#![cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]

use std::collections::HashMap;

use bridgewatch_core::token::{SystemTokenProvider, TokenError, TokenProvider};
use secret_service::{EncryptionType, blocking::SecretService};

#[test]
fn a_quiet_read_finds_an_unlocked_item_and_says_when_there_is_none() {
    if std::env::var_os("BRIDGEWATCH_TEST_SECRET_SERVICE").is_none() {
        return;
    }
    let service = format!("bw-test:{}", std::process::id());
    let ss =
        SecretService::connect(EncryptionType::Dh).expect("a Secret Service on the session bus");
    let collection = ss.get_default_collection().expect("a default collection");
    let item = collection
        .create_item(
            "bridgewatch test item",
            HashMap::from([("service", service.as_str()), ("username", "token")]),
            b"glpat-from-the-secret-service",
            true,
            "text/plain",
        )
        .expect("the item is written");

    assert_eq!(
        SystemTokenProvider
            .keyring_get_quiet(&service, "token")
            .unwrap(),
        "glpat-from-the-secret-service"
    );
    assert!(matches!(
        SystemTokenProvider.keyring_get_quiet(&service, "someone-else"),
        Err(TokenError::NoEntry { .. })
    ));
    item.delete().expect("the item is removed");
}

/// With no session bus to reach, a quiet read is a store error in the
/// Secret Service's own words, never "no entry". Run in a child process,
/// because the bus address is read from the environment.
#[test]
fn a_quiet_read_with_no_session_bus_is_a_store_error() {
    if std::env::var_os("BW_NO_BUS_CHILD").is_some() {
        let read = SystemTokenProvider.keyring_get_quiet("bw-test:no-bus", "token");
        assert!(matches!(read, Err(TokenError::Store { .. })), "{read:?}");
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_quiet_read_with_no_session_bus_is_a_store_error",
        ])
        .env("BW_NO_BUS_CHILD", "1")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/bridgewatch-bus",
        )
        .status()
        .unwrap();
    assert!(status.success());
}
