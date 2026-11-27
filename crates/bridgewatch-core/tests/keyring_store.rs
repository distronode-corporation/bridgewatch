// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! The real token provider's credential-store calls, against keyring-core's
//! mock store.
//!
//! keyring-core's default store is process-wide, so this is a test binary of
//! its own: no other test here can find the mock in place of the OS store. The
//! platform store is installed first, as the provider does on first use, and
//! the mock replaces it; every test then holds `STORE` so none runs over another.

use std::sync::{Arc, Mutex, OnceLock};

use bridgewatch_core::token::{SystemTokenProvider, TokenError, TokenProvider};
use keyring_core::mock;

static STORE: Mutex<()> = Mutex::new(());

/// The mock store, installed once in place of the platform's.
fn mock_store() -> &'static Arc<mock::Store> {
    static MOCK: OnceLock<Arc<mock::Store>> = OnceLock::new();
    MOCK.get_or_init(|| {
        let _ = keyring::Entry::store_status();
        let store = mock::Store::new().expect("a mock store");
        keyring_core::set_default_store(store.clone());
        store
    })
}

/// Hold the store for one test.
fn store() -> std::sync::MutexGuard<'static, ()> {
    let guard = STORE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    mock_store();
    guard
}

/// The next call on this entry fails with `err`, once.
fn fail_next(service: &str, user: &str, err: keyring_core::Error) {
    let entry = keyring_core::Entry::new(service, user).expect("a mock entry");
    let cred: &mock::Cred = entry.as_any().downcast_ref().expect("a mock credential");
    cred.set_error(err);
}

#[test]
fn a_token_written_is_read_back_and_an_envelope_is_opened() {
    let _store = store();
    let p = SystemTokenProvider;
    p.keyring_set("bw-test:write", "token", "glpat-written")
        .unwrap();
    assert_eq!(
        p.keyring_get("bw-test:write", "token").unwrap(),
        "glpat-written"
    );
    // The quiet read goes through the same store on macOS and Windows. On Linux
    // it asks the Secret Service over D-Bus itself, which `token/quiet.rs`
    // tests through a scripted search.
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )))]
    assert_eq!(
        p.keyring_get_quiet("bw-test:write", "token").unwrap(),
        "glpat-written"
    );
    // What another tool wrote, in go-keyring's base64 envelope.
    p.keyring_set(
        "bw-test:envelope",
        "token",
        "go-keyring-base64:Z2xwYXQtZW52ZWxvcGVk",
    )
    .unwrap();
    assert_eq!(
        p.keyring_get("bw-test:envelope", "token").unwrap(),
        "glpat-enveloped"
    );
}

#[test]
fn a_read_of_nothing_is_no_entry_and_a_store_failure_says_what_failed() {
    let _store = store();
    let p = SystemTokenProvider;
    assert!(matches!(
        p.keyring_get("bw-test:absent", "token"),
        Err(TokenError::NoEntry { service, user }) if service == "bw-test:absent" && user == "token"
    ));
    fail_next(
        "bw-test:broken",
        "token",
        keyring_core::Error::Invalid("collection".into(), "is gone".into()),
    );
    match p.keyring_get("bw-test:broken", "token") {
        Err(TokenError::Store { service, message }) => {
            assert_eq!(service, "bw-test:broken");
            assert!(message.contains("is gone"), "{message}");
        }
        other => panic!("expected a store error, got {other:?}"),
    }
}

#[test]
fn a_delete_of_nothing_is_fine_and_a_failed_delete_is_not() {
    let _store = store();
    let p = SystemTokenProvider;
    p.keyring_delete("bw-test:never-written", "token").unwrap();
    p.keyring_set("bw-test:delete", "token", "glpat-x").unwrap();
    p.keyring_delete("bw-test:delete", "token").unwrap();
    assert!(matches!(
        p.keyring_get("bw-test:delete", "token"),
        Err(TokenError::NoEntry { .. })
    ));
    fail_next(
        "bw-test:locked",
        "token",
        keyring_core::Error::Invalid("store".into(), "is locked".into()),
    );
    assert!(matches!(
        p.keyring_delete("bw-test:locked", "token"),
        Err(TokenError::Store { .. })
    ));
}

#[test]
fn an_empty_user_is_never_written_and_deleting_it_does_nothing() {
    let _store = store();
    let p = SystemTokenProvider;
    match p.keyring_set("bw-test:empty", "", "glpat-x") {
        Err(TokenError::Store { message, .. }) => {
            assert!(message.contains("empty user"), "{message}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    p.keyring_delete("bw-test:empty", "").unwrap();
}

/// The empty account is read by the platform's own tool (`security` or
/// `secret-tool`), which has no entry by this name, or is not installed.
#[test]
fn an_empty_user_read_with_no_such_item_is_no_entry() {
    let _store = store();
    assert!(matches!(
        SystemTokenProvider.keyring_get("bw-test:no-such-item-anywhere", ""),
        Err(TokenError::NoEntry { user, .. }) if user.is_empty()
    ));
}

#[test]
fn a_write_the_store_refuses_says_so() {
    let _store = store();
    fail_next(
        "bw-test:readonly",
        "token",
        keyring_core::Error::Invalid("store".into(), "is read-only".into()),
    );
    assert!(matches!(
        SystemTokenProvider.keyring_set("bw-test:readonly", "token", "glpat-x"),
        Err(TokenError::Store { message, .. }) if message.contains("is read-only")
    ));
}

/// A prompt the person cancelled is "needs approval, refused", whatever store
/// reported it: the Secret Service's dismissed prompt on Linux, the Keychain's
/// user-cancelled status on macOS.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_cancelled_store_prompt_is_needs_approval_refused() {
    let _store = store();
    #[cfg(target_os = "linux")]
    let cancelled = keyring_core::Error::NoStorageAccess(Box::new(secret_service::Error::Prompt));
    #[cfg(target_os = "macos")]
    let cancelled = keyring_core::Error::NoStorageAccess(Box::new(
        security_framework::base::Error::from_code(-128),
    ));
    fail_next("bw-test:cancelled", "token", cancelled);
    assert!(matches!(
        SystemTokenProvider.keyring_get("bw-test:cancelled", "token"),
        Err(TokenError::NeedsApproval { refused: true, .. })
    ));
}
