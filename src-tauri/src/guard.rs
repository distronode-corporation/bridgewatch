//! The changes a settings window may not make in one step.
//!
//! Every write from either window arrives over IPC, and IPC is reachable from
//! any script running in the webview. Two kinds of change turn such a script
//! into something much worse than a broken config:
//!
//! - a `command` token source, which runs a program on the next reload;
//! - a new host for an account that already has a credential, which sends that
//!   credential there on the next tick;
//! - an account (new or changed) whose token source is a credential that lives
//!   outside this file (a keyring entry, an environment variable) on a host no
//!   account used before, which is the same exfiltration by addition.
//!
//! So none of them is written on the first request. The write is answered with a
//! [`ConfirmRequest`] that describes the change in words chosen HERE, and it
//! only lands when the same text comes back with that request's id. The
//! frontend's own question used to be the only check, and it guarded one of
//! the three routes to the file (the token form); "Edit as text" and a raw
//! `apply_config_edits` walked straight past it.
//!
//! ⚠ What this does NOT do is defend against a webview that is already fully
//! compromised: such a script can read the id out of the first answer and send
//! it back. Closing that needs a confirmation the webview cannot click, i.e. a
//! native dialog (`tauri-plugin-dialog`), which this crate does not depend on.
//! What it does guarantee is that no single call, and no path that skips the
//! confirmation UI, can make either change, and that the words the user
//! confirms are the shell's description of the exact text being written.
//!
//! Edits made in the user's own editor are not subject to any of this: the
//! file watcher is not IPC.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bridgewatch_core::config::{Config, TokenSource};
use serde::Serialize;

/// How long a confirmation stays answerable.
pub const CONFIRM_TTL: Duration = Duration::from_secs(300);

/// What the frontend is asked to confirm.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfirmRequest {
    /// Send this back with the same text to go ahead.
    pub id: String,
    /// One sentence per sensitive change, for the question.
    pub changes: Vec<String>,
}

/// Every sensitive difference between the file as it is and as it would be.
///
/// `old` is `None` when nothing loadable is on disk or running, in which case
/// every `command` source in `new` counts as introduced.
pub fn sensitive_changes(old: Option<&Config>, new: &Config) -> Vec<String> {
    let mut out = Vec::new();
    for (name, account) in &new.accounts {
        let before = old.and_then(|o| o.accounts.get(name));

        if let TokenSource::Command(argv) = &account.token {
            let unchanged = matches!(
                before.map(|b| &b.token),
                Some(TokenSource::Command(prev)) if prev == argv
            );
            if !unchanged {
                out.push(format!(
                    "Account \"{name}\" will get its token by running `{}`. That program runs \
                     every time a token is needed, with your permissions.",
                    argv.join(" ")
                ));
            }
        }

        let now = origin(&account.base_url);
        if let Some(before) = before
            && origin(&before.base_url) != now
        {
            out.push(format!(
                "Account \"{name}\" moves from {} to {}. Its token will be sent to the new \
                 host.",
                before.base_url.trim_end_matches('/'),
                account.base_url.trim_end_matches('/')
            ));
            continue;
        }

        // A credential that exists independently of this file (another tool's
        // keyring entry, an environment variable) sent to a host no account
        // used before. Without this, ADDING an account was the way round the
        // host-move rule above: `[accounts.x]` on a new host with glab's
        // keyring entry as its source ships that token there. An `own` entry
        // is the account's own, written by the user for it, so it is exempt.
        let shared = match &account.token {
            TokenSource::Keyring { service, user } if user.is_empty() => {
                Some(format!("the credential-store entry \"{service}\""))
            }
            TokenSource::Keyring { service, user } => Some(format!(
                "the credential-store entry \"{service}\" (user \"{user}\")"
            )),
            TokenSource::Env(var) => Some(format!("the environment variable {var}")),
            TokenSource::Command(_) | TokenSource::Own(_) => None,
        };
        let known_host =
            old.is_some_and(|o| o.accounts.values().any(|a| origin(&a.base_url) == now));
        if let Some(shared) = shared
            && !known_host
        {
            out.push(format!(
                "Account \"{name}\" will send {shared} to {}, which no configured account \
                 used before.",
                account.base_url.trim_end_matches('/')
            ));
        }
    }
    out
}

/// Scheme, host and port, lowercased, so `https://GitLab.com/` and
/// `https://gitlab.com` are one origin. Unparsable input is its own origin.
fn origin(base_url: &str) -> String {
    match url::Url::parse(base_url.trim()) {
        Ok(u) => format!(
            "{}://{}:{}",
            u.scheme(),
            u.host_str().unwrap_or("").to_ascii_lowercase(),
            u.port_or_known_default().unwrap_or(0)
        ),
        Err(_) => base_url.trim().to_string(),
    }
}

/// The one outstanding confirmation, if any.
///
/// One at a time on purpose: a second sensitive write replaces the first
/// question rather than leaving two ids alive.
#[derive(Default)]
pub struct Confirmations {
    pending: Mutex<Option<Pending>>,
}

struct Pending {
    id: String,
    text: String,
    issued: Instant,
}

impl Confirmations {
    /// Park `text` and return the request that will release it.
    pub fn issue(&self, text: &str, changes: Vec<String>) -> ConfirmRequest {
        let id = fresh_id();
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            id: id.clone(),
            text: text.to_string(),
            issued: Instant::now(),
        });
        ConfirmRequest { id, changes }
    }

    /// True when `id` is the outstanding request for exactly this text and it
    /// has not expired. Spends it either way: an id is good for one attempt.
    pub fn redeem(&self, id: Option<&str>, text: &str) -> bool {
        let Some(id) = id else { return false };
        let taken = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        taken.is_some_and(|p| p.id == id && p.text == text && p.issued.elapsed() < CONFIRM_TTL)
    }
}

/// An id nobody can guess from outside the process. Not a secret against a
/// script inside the webview (see the module comment); unpredictability is
/// only there so a stale id from an earlier question can never match.
fn fresh_id() -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(raw: &str) -> Config {
        bridgewatch_core::config::parse_str(raw, Path::new("x.toml"))
            .expect("fixture parses")
            .config
    }

    const OWN: &str = "[accounts.gl]\nbase_url = \"https://gitlab.com\"\ntoken = { own = true }\n";
    const CMD: &str = "[accounts.gl]\nbase_url = \"https://gitlab.com\"\ntoken = { command = [\"pass\", \"gl\"] }\n";

    #[test]
    fn introducing_a_command_source_is_sensitive() {
        let changes = sensitive_changes(Some(&parse(OWN)), &parse(CMD));
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert!(changes[0].contains("`pass gl`"), "{changes:?}");
    }

    #[test]
    fn changing_the_program_is_sensitive_and_keeping_it_is_not() {
        let other = CMD.replace("\"gl\"", "\"other\"");
        assert_eq!(
            sensitive_changes(Some(&parse(CMD)), &parse(&other)).len(),
            1
        );
        assert!(sensitive_changes(Some(&parse(CMD)), &parse(CMD)).is_empty());
    }

    #[test]
    fn with_nothing_on_disk_every_command_source_is_new() {
        assert_eq!(sensitive_changes(None, &parse(CMD)).len(), 1);
        assert!(sensitive_changes(None, &parse(OWN)).is_empty());
    }

    #[test]
    fn moving_an_account_to_another_host_is_sensitive() {
        let moved = OWN.replace("https://gitlab.com", "https://gitlab.example.net");
        let changes = sensitive_changes(Some(&parse(OWN)), &parse(&moved));
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert!(changes[0].contains("gitlab.example.net"));
    }

    #[test]
    fn a_new_account_that_sends_a_stored_credential_to_a_new_host_is_sensitive() {
        // The host-move rule only looked at accounts that already existed, so
        // ADDING `[accounts.x]` on another host with glab's keyring entry (or
        // `$GITLAB_TOKEN`) as its source sent that credential there with no
        // question asked.
        for source in [
            "{ keyring = { service = \"glab:gitlab.com\" } }",
            "{ env = \"GITLAB_TOKEN\" }",
        ] {
            let added = format!(
                "{OWN}[accounts.x]\nbase_url = \"https://evil.example\"\ntoken = {source}\n"
            );
            let changes = sensitive_changes(Some(&parse(OWN)), &parse(&added));
            assert_eq!(changes.len(), 1, "{source}: {changes:?}");
            assert!(changes[0].contains("evil.example"), "{changes:?}");
        }
        // Also from nothing (a first run, or a file that does not load).
        let fresh = "[accounts.x]\nbase_url = \"https://evil.example\"\ntoken = { env = \"T\" }\n";
        assert_eq!(sensitive_changes(None, &parse(fresh)).len(), 1);
    }

    #[test]
    fn a_stored_credential_on_a_host_already_configured_is_not_asked_about() {
        // A second account on the same instance, or an own-entry account on a
        // new one (its entry is its own, written by the user), is ordinary.
        let second = format!(
            "{OWN}[accounts.y]\nbase_url = \"https://gitlab.com\"\ntoken = {{ env = \"T\" }}\n"
        );
        assert!(sensitive_changes(Some(&parse(OWN)), &parse(&second)).is_empty());
        let own_new_host =
            format!("{OWN}[accounts.z]\nbase_url = \"https://gitlab.example.net\"\n");
        assert!(sensitive_changes(Some(&parse(OWN)), &parse(&own_new_host)).is_empty());
        // And a moved account is described once, not twice.
        let moved_env =
            "[accounts.gl]\nbase_url = \"https://evil.example\"\ntoken = { env = \"T\" }\n";
        let before = "[accounts.gl]\nbase_url = \"https://gitlab.com\"\ntoken = { env = \"T\" }\n";
        assert_eq!(
            sensitive_changes(Some(&parse(before)), &parse(moved_env)).len(),
            1
        );
    }

    #[test]
    fn spelling_the_same_origin_differently_is_not() {
        let same = OWN.replace("https://gitlab.com", "https://GitLab.com:443/");
        assert!(sensitive_changes(Some(&parse(OWN)), &parse(&same)).is_empty());
        // And an ordinary edit is not either.
        let edited = format!("{OWN}timeout_secs = 30\n");
        assert!(sensitive_changes(Some(&parse(OWN)), &parse(&edited)).is_empty());
    }

    #[test]
    fn a_confirmation_releases_only_its_own_text_once() {
        let c = Confirmations::default();
        let req = c.issue("text A", vec!["x".into()]);
        assert!(
            !c.redeem(Some(&req.id), "text B"),
            "a different text was released"
        );
        // The failed attempt spent it.
        assert!(!c.redeem(Some(&req.id), "text A"));

        let req = c.issue("text A", vec![]);
        assert!(!c.redeem(None, "text A"));
        let req2 = c.issue("text A", vec![]);
        assert!(
            !c.redeem(Some(&req.id), "text A"),
            "a superseded id still worked"
        );
        let req3 = c.issue("text A", vec![]);
        assert_ne!(req2.id, req3.id);
        assert!(c.redeem(Some(&req3.id), "text A"));
        assert!(!c.redeem(Some(&req3.id), "text A"), "an id worked twice");
    }
}
