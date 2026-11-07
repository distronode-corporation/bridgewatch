//! Reads that may not ask, and telling a refused prompt from any other
//! failure of the credential store.
//!
//! # macOS
//!
//! `keyring` 4.2.0 reads through `apple-native-keyring-store` 1.0.2's
//! `keychain` store, which is the file-based keychain API
//! (`SecKeychainFindGenericPassword`, via `security-framework` 3.7.0). Its
//! access-list dialog ("bridgewatch wants to use your confidential
//! information stored in ...") is controlled by one switch,
//! `SecKeychainSetUserInteractionAllowed`, which `security-framework` wraps as
//! `SecKeychain::disable_user_interaction()` (a guard that turns it back on
//! when dropped). With it off, a read that would show the dialog returns
//! `errSecInteractionNotAllowed` (-25308) instead. The other option,
//! `kSecUseAuthenticationUI` (`ItemSearchOptions::skip_authenticated_items`),
//! is a `SecItemCopyMatching` attribute for the data-protection keychain's
//! access control, and the store does not search that way.
//!
//! ⛔ The switch is process-wide. A quiet read therefore takes [`TURNS`]
//! exclusively, and every other keychain call shares it, so no read that may
//! ask can start while the switch is off. A quiet read that cannot get its
//! turn within [`QUIET_WAIT`] (another read is up on a dialog) is
//! [`TokenError::NeedsApproval`] without reading: that is what it would have
//! found.
//!
//! # Linux and the BSDs
//!
//! The `keyring` store is `zbus-secret-service-keyring-store`, which UNLOCKS a
//! locked collection to read from it: a prompt. A quiet read searches the
//! Secret Service itself (`secret-service` 5.1, the crate that store is built
//! on, with the same `service` and `username` attributes) and reads only an
//! item that is already unlocked. A dismissed unlock prompt comes back from
//! the store as `secret_service::Error::Prompt`.
//!
//! # Windows
//!
//! The Credential Manager never asks. A quiet read is an ordinary one.

use super::TokenError;

/// `errSecUserCanceled`: the user pressed Cancel.
pub(super) const ERR_SEC_USER_CANCELED: i32 = -128;
/// `errSecAuthFailed`: what a Deny on the access dialog returns.
pub(super) const ERR_SEC_AUTH_FAILED: i32 = -25293;
/// `errSecInteractionNotAllowed`: the read would have asked, and may not.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

/// What a macOS `OSStatus` says about approval: `Some(true)` when a prompt
/// was shown and refused, `Some(false)` when one would have been shown and
/// was not allowed to be, `None` for anything else.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn approval_of_status(code: i32) -> Option<bool> {
    match code {
        ERR_SEC_USER_CANCELED | ERR_SEC_AUTH_FAILED => Some(true),
        ERR_SEC_INTERACTION_NOT_ALLOWED => Some(false),
        _ => None,
    }
}

/// What a `keyring` error says about approval, as [`approval_of_status`].
#[cfg(target_os = "macos")]
pub(super) fn approval_of(err: &keyring::Error) -> Option<bool> {
    match err {
        keyring::Error::PlatformFailure(inner) | keyring::Error::NoStorageAccess(inner) => inner
            .downcast_ref::<security_framework::base::Error>()
            .and_then(|e| approval_of_status(e.code())),
        _ => None,
    }
}

/// What a `keyring` error says about approval: a dismissed unlock prompt is a
/// refusal.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
pub(super) fn approval_of(err: &keyring::Error) -> Option<bool> {
    match err {
        keyring::Error::NoStorageAccess(inner) => inner
            .downcast_ref::<secret_service::Error>()
            .and_then(|e| matches!(e, secret_service::Error::Prompt).then_some(true)),
        _ => None,
    }
}

/// Windows: nothing asks, so nothing is refused.
#[cfg(not(any(
    target_os = "macos",
    all(unix, not(any(target_os = "ios", target_os = "android")))
)))]
pub(super) fn approval_of(_err: &keyring::Error) -> Option<bool> {
    None
}

/// Keychain calls that may ask share this; a quiet read holds it alone.
#[cfg(target_os = "macos")]
static TURNS: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// How long a quiet read waits for the other keychain calls to finish.
#[cfg(target_os = "macos")]
const QUIET_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Held for the length of a keychain call that may ask. Never blocks for
/// longer than a quiet read takes.
#[cfg(target_os = "macos")]
pub(super) fn interactive_turn() -> std::sync::RwLockReadGuard<'static, ()> {
    TURNS.read().unwrap_or_else(|e| e.into_inner())
}

/// Nothing to share where the switch does not exist.
#[cfg(not(target_os = "macos"))]
pub(super) fn interactive_turn() -> NoTurn {
    NoTurn
}

/// What [`interactive_turn`] holds where there is no switch: nothing.
#[cfg(not(target_os = "macos"))]
pub(super) struct NoTurn;

/// Read `service`/`user` with the Keychain's user interaction off.
#[cfg(target_os = "macos")]
pub(super) fn get(service: &str, user: &str) -> Result<String, TokenError> {
    use security_framework::os::macos::keychain::SecKeychain;

    let needs_approval = || TokenError::NeedsApproval {
        service: service.to_string(),
        refused: false,
    };
    // The empty account is read by the `security` tool (see the module
    // documentation of `token`), another process, which this switch does not
    // reach and which has no quiet mode of its own.
    if user.is_empty() {
        return Err(needs_approval());
    }
    let deadline = std::time::Instant::now() + QUIET_WAIT;
    let _turn = loop {
        match TURNS.try_write() {
            Ok(turn) => break turn,
            Err(std::sync::TryLockError::Poisoned(e)) => break e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(std::sync::TryLockError::WouldBlock) => return Err(needs_approval()),
        }
    };
    // Dropped before `_turn` (reverse order of declaration): the switch is
    // back on before any other keychain call can start.
    let _no_ui = SecKeychain::disable_user_interaction().map_err(|e| TokenError::Store {
        service: service.to_string(),
        message: format!("could not turn the Keychain's prompts off for a status read: {e}"),
    })?;
    keyring::Entry::new(service, user)
        .and_then(|e| e.get_password())
        .map_err(|e| super::classify_keyring_error(e, service, user))
}

/// Read `service`/`user` from the Secret Service only if it is unlocked.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
pub(super) fn get(service: &str, user: &str) -> Result<String, TokenError> {
    use secret_service::{EncryptionType, blocking::SecretService};

    let store = |e: secret_service::Error| TokenError::Store {
        service: service.to_string(),
        message: e.to_string(),
    };
    let ss = SecretService::connect(EncryptionType::Dh).map_err(store)?;
    let found = ss
        .search_items(std::collections::HashMap::from([
            ("service", service),
            ("username", user),
        ]))
        .map_err(store)?;
    if let Some(item) = found.unlocked.first() {
        let bytes = item.get_secret().map_err(store)?;
        return String::from_utf8(bytes).map_err(|_| TokenError::Store {
            service: service.to_string(),
            message: "the stored value is not UTF-8".to_string(),
        });
    }
    if !found.locked.is_empty() {
        return Err(TokenError::NeedsApproval {
            service: service.to_string(),
            refused: false,
        });
    }
    Err(TokenError::NoEntry {
        service: service.to_string(),
        user: user.to_string(),
    })
}

/// Windows: an ordinary read, which never asks.
#[cfg(not(any(
    target_os = "macos",
    all(unix, not(any(target_os = "ios", target_os = "android")))
)))]
pub(super) fn get(service: &str, user: &str) -> Result<String, TokenError> {
    if user.is_empty() {
        return Err(TokenError::NoEntry {
            service: service.to_string(),
            user: String::new(),
        });
    }
    keyring::Entry::new(service, user)
        .and_then(|e| e.get_password())
        .map_err(|e| super::classify_keyring_error(e, service, user))
}

#[cfg(test)]
mod tests {
    use super::{
        ERR_SEC_AUTH_FAILED, ERR_SEC_INTERACTION_NOT_ALLOWED, ERR_SEC_USER_CANCELED,
        approval_of_status,
    };

    /// Cancel and Deny are refusals; "may not ask" is not; nothing else
    /// (not found, a locked keychain file that is missing) is either.
    #[test]
    fn only_a_cancel_a_deny_and_a_forbidden_prompt_are_about_approval() {
        assert_eq!(approval_of_status(ERR_SEC_USER_CANCELED), Some(true));
        assert_eq!(approval_of_status(ERR_SEC_AUTH_FAILED), Some(true));
        assert_eq!(
            approval_of_status(ERR_SEC_INTERACTION_NOT_ALLOWED),
            Some(false)
        );
        for other in [0, -25300, -25294, -61, -34018] {
            assert_eq!(approval_of_status(other), None, "{other}");
        }
    }

    /// A dismissed Secret Service unlock is a refusal; a missing service is a
    /// store failure that keeps its backoff.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_dismissed_unlock_prompt_is_a_refusal_and_nothing_else_is() {
        let dismissed = keyring::Error::NoStorageAccess(Box::new(secret_service::Error::Prompt));
        assert_eq!(super::approval_of(&dismissed), Some(true));
        let locked = keyring::Error::NoStorageAccess(Box::new(secret_service::Error::Locked));
        assert_eq!(super::approval_of(&locked), None);
        let gone = keyring::Error::PlatformFailure(Box::new(secret_service::Error::Unavailable));
        assert_eq!(super::approval_of(&gone), None);
        assert_eq!(super::approval_of(&keyring::Error::NoEntry), None);
    }
}
