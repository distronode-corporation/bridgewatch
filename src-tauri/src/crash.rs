//! Opt-in crash reports, sent to Sentry.
//!
//! ⛔ OFF UNLESS TWO THINGS ARE BOTH TRUE, AND NOTHING IS SENT OTHERWISE:
//!   1. the build has an address: `BRIDGEWATCH_SENTRY_DSN`, read at COMPILE time
//!      with `option_env!`. Only the release workflow sets it, so a build from
//!      source, a fork's build and every CI test build have none, and for them
//!      this module never creates a client at all;
//!   2. the user said yes: `[ui].send_crash_reports = true`. Unset (never
//!      answered) and `false` are both no.
//!
//! The client is created the first time both hold, not at startup, so a user
//! who never says yes never has one. Saying no afterwards does not tear the
//! client down (`sentry`'s panic hook is installed once per process and cannot
//! be removed); it flips the flag `before_send` reads, so every event is dropped
//! before it reaches the transport. `sentry` only ever sends an event or a
//! client report riding along with one, so a dropped event is a request that
//! never happens. `src-tauri/tests/crash_reports.rs` proves both halves against
//! a local HTTP server standing in for Sentry.
//!
//! What a report holds: the panic message, the stack, the app's release, the
//! OS name and version, the CPU architecture and the list of loaded modules.
//! `before_send` rewrites every home-directory path in it (`/home/<u>`,
//! `/Users/<u>`, `C:\Users\<u>`) to `<user>`, and drops the user, the IP and
//! the host name (`server_name`, which the contexts integration fills from the
//! hostname). `send_default_pii` stays `false`. If the scrub cannot run, the
//! report is dropped, not sent unscrubbed.
//!
//! ⚠ Kept free of `crate::` paths on purpose: the proof test includes this file
//! with `#[path]`, because the app is a binary crate with no library to link
//! against. District Core for Rust carries a module of the same shape; keep the
//! two close so they can become one crate later.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use regex::Regex;
use sentry::protocol::Event;

/// The bundle identifier, as `tauri.conf.json` has it. Releases are named
/// after it, so the proof test holds the two together.
pub const BUNDLE_ID: &str = "com.distronode.bridgewatch";

/// The crash-report address this build was compiled with, if any.
pub fn baked_dsn() -> Option<&'static str> {
    non_empty(option_env!("BRIDGEWATCH_SENTRY_DSN"))
}

/// The build label this build was compiled with: the release workflow sets
/// `BRIDGEWATCH_BUILD` to the tagged commit's short SHA. `dev` otherwise.
pub fn baked_build() -> &'static str {
    non_empty(option_env!("BRIDGEWATCH_BUILD")).unwrap_or("dev")
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// `com.distronode.bridgewatch@<version>+<build>`, the name the release
/// workflow creates the Sentry release under. The two must agree, or the
/// uploaded debug files and commits attach to a release no event names.
pub fn release_name(version: &str, build: &str) -> String {
    format!("{BUNDLE_ID}@{version}+{build}")
}

/// The one reporter a process has.
pub fn global() -> &'static Reporter {
    static GLOBAL: OnceLock<Reporter> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        Reporter::new(
            baked_dsn(),
            release_name(env!("CARGO_PKG_VERSION"), baked_build()),
        )
    })
}

/// The consent the config file records: only an explicit `true` is a yes.
pub fn consent_of(answer: Option<bool>) -> bool {
    answer == Some(true)
}

/// A crash-report client that exists only once it is allowed to.
pub struct Reporter {
    dsn: Option<sentry::types::Dsn>,
    release: String,
    consent: Arc<AtomicBool>,
    guard: Mutex<Option<sentry::ClientInitGuard>>,
}

impl Reporter {
    /// A reporter for `dsn`. An absent, empty or unparseable address makes a
    /// reporter that can never send.
    ///
    /// ⛔ The address is parsed HERE, so `sentry::init` is never called without
    /// one: given none, `sentry` falls back to the `SENTRY_DSN` environment
    /// variable, which would let the environment turn reporting on in a build
    /// that has no address of its own.
    pub fn new(dsn: Option<&str>, release: String) -> Self {
        let dsn = dsn
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .and_then(|d| match d.parse::<sentry::types::Dsn>() {
                Ok(dsn) => Some(dsn),
                Err(e) => {
                    tracing::warn!(error = %e, "the built-in crash-report address does not parse; crash reports are off");
                    None
                }
            });
        Self {
            dsn,
            release,
            consent: Arc::new(AtomicBool::new(false)),
            guard: Mutex::new(None),
        }
    }

    /// Whether this build has somewhere to send reports at all.
    pub fn has_address(&self) -> bool {
        self.dsn.is_some()
    }

    /// Whether a client exists. False until the first yes in a build with an
    /// address, and false for ever in one without.
    pub fn has_client(&self) -> bool {
        self.lock().is_some()
    }

    /// Whether a report would be sent right now.
    pub fn is_sending(&self) -> bool {
        self.consent.load(Ordering::SeqCst) && self.has_client()
    }

    /// Record the user's answer. A yes in a build with an address creates the
    /// client if there is none yet; a no stops every later report.
    pub fn set_consent(&self, consent: bool) {
        self.consent.store(consent, Ordering::SeqCst);
        if !consent {
            return;
        }
        let Some(dsn) = &self.dsn else {
            return;
        };
        let mut guard = self.lock();
        if guard.is_none() {
            *guard = Some(sentry::init(self.options(dsn.clone())));
            tracing::info!(release = %self.release, "crash reports are on");
        }
    }

    /// Flush what is queued and close the client, at exit.
    pub fn shutdown(&self) {
        // Dropping the guard flushes, bounded by `shutdown_timeout`.
        drop(self.lock().take());
    }

    fn options(&self, dsn: sentry::types::Dsn) -> sentry::ClientOptions {
        let consent = self.consent.clone();
        // Field by field: `ClientOptions` is non-exhaustive.
        let mut options = sentry::ClientOptions::new();
        options.dsn = Some(dsn);
        options.release = Some(Cow::Owned(self.release.clone()));
        options.send_default_pii = false;
        // No session pings: the `release-health` feature is off, and this
        // says so again in case a dependency turns it on.
        options.auto_session_tracking = false;
        options.before_send = Some(Arc::new(move |event| {
            if consent.load(Ordering::SeqCst) {
                scrub_event(event)
            } else {
                None
            }
        }));
        options
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<sentry::ClientInitGuard>> {
        self.guard.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What a report may carry, applied to every event before it is sent.
///
/// Every string in the event, keys included, goes through [`scrub_paths`] by a
/// round trip through JSON, so a path in a field nobody thought of (a module's
/// `code_file`, a frame's `abs_path`, a panic message) is caught too. The user,
/// the host name and any request are dropped. `None` when the round trip
/// fails: a report that cannot be scrubbed is not sent.
pub fn scrub_event(event: Event<'static>) -> Option<Event<'static>> {
    let mut value = serde_json::to_value(&event).ok()?;
    scrub_value(&mut value);
    let mut event: Event<'static> = serde_json::from_value(value).ok()?;
    event.user = None;
    event.server_name = None;
    event.request = None;
    Some(event)
}

fn scrub_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            if let Cow::Owned(clean) = scrub_paths(s) {
                *s = clean;
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(scrub_value),
        serde_json::Value::Object(map) => {
            let entries = std::mem::take(map);
            for (key, mut item) in entries {
                scrub_value(&mut item);
                map.insert(scrub_paths(&key).into_owned(), item);
            }
        }
        _ => {}
    }
}

/// Replace the user name in every home-directory path with `<user>`:
/// `/home/<u>`, `/Users/<u>` and `C:\Users\<u>` (any drive, either slash).
pub fn scrub_paths(text: &str) -> Cow<'_, str> {
    static PATTERNS: OnceLock<[(Regex, &str); 2]> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            // `<` is outside the name class, so a scrubbed path is left alone.
            (
                Regex::new(r#"(/home/|/Users/)[^/\\\s"'<>]+"#).expect("a valid pattern"),
                "${1}<user>",
            ),
            (
                Regex::new(r#"(?i)\b([a-z]:[\\/]+users[\\/]+)[^/\\\s"'<>]+"#)
                    .expect("a valid pattern"),
                "${1}<user>",
            ),
        ]
    });
    let mut out = Cow::Borrowed(text);
    for (pattern, replacement) in patterns {
        if let Cow::Owned(replaced) = pattern.replace_all(&out, *replacement) {
            out = Cow::Owned(replaced);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_paths_lose_the_user_name_on_all_three_platforms() {
        assert_eq!(
            scrub_paths("at /home/alice/src/bw/main.rs:3"),
            "at /home/<user>/src/bw/main.rs:3"
        );
        assert_eq!(
            scrub_paths("/Users/bob/Library/Logs/bridgewatch"),
            "/Users/<user>/Library/Logs/bridgewatch"
        );
        assert_eq!(
            scrub_paths(r"C:\Users\carol\AppData\x.toml and d:/users/dave/y"),
            r"C:\Users\<user>\AppData\x.toml and d:/users/<user>/y"
        );
        // Several in one string, and a path ending at the name.
        assert_eq!(
            scrub_paths("'/home/eve' then \"/var/home/frank/x\""),
            "'/home/<user>' then \"/var/home/<user>/x\""
        );
    }

    #[test]
    fn a_scrubbed_or_harmless_string_is_left_alone() {
        for clean in ["/home/<user>/x", r"C:\Users\<user>\x", "/usr/lib/x.so", ""] {
            assert!(matches!(scrub_paths(clean), Cow::Borrowed(_)), "{clean}");
        }
        // A URL path segment called `home` is not a home directory, but the
        // rule cannot tell, and erring towards removing a word is the safe side.
        assert_eq!(
            scrub_paths("https://gitlab.com/home/x"),
            "https://gitlab.com/home/<user>"
        );
    }

    #[test]
    fn only_an_explicit_yes_is_consent() {
        assert!(consent_of(Some(true)));
        assert!(!consent_of(Some(false)));
        assert!(!consent_of(None));
    }

    #[test]
    fn the_release_name_is_the_bundle_id_at_version_plus_build() {
        assert_eq!(
            release_name("1.2.3", "abc123def456"),
            "com.distronode.bridgewatch@1.2.3+abc123def456"
        );
    }

    #[test]
    fn a_reporter_without_an_address_never_creates_a_client() {
        for dsn in [None, Some(""), Some("   "), Some("not a dsn")] {
            let reporter = Reporter::new(dsn, release_name("0.0.0", "test"));
            reporter.set_consent(true);
            assert!(!reporter.has_address(), "{dsn:?}");
            assert!(!reporter.has_client(), "{dsn:?}");
            assert!(!reporter.is_sending(), "{dsn:?}");
        }
    }
}
