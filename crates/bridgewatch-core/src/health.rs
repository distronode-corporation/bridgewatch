//! Token health: whether an account's credential is about to stop working, or
//! already has, and why (2.0 F3).
//!
//! # Four states
//!
//! | state | what it means | how it is known |
//! | --- | --- | --- |
//! | `expiring` | it works, and stops within 72 h | an expiry the provider states |
//! | `expired` | it stopped at its expiry | GitLab's 401 says so; else a 401 at or past a known expiry |
//! | `revoked` | somebody withdrew it | GitLab's 401 says so; a sign-in whose refresh was refused before it ran out |
//! | `invalid` | the server does not accept it, and says no more | any other 401 |
//!
//! # Where an expiry comes from, and what it costs
//!
//! - **GitLab personal, project and group access tokens:** `expires_at` (a
//!   date) from `GET /personal_access_tokens/self`, asked ONCE per poller and
//!   remembered. GitLab ends a token at 00:00 UTC on that date, which is the
//!   earliest reading of it and the one used here.
//! - **GitHub personal access tokens** (fine-grained, and classic ones with an
//!   expiry): the `github-authentication-token-expiration` response header,
//!   which rides on every API response, so it costs no request at all. It is
//!   ignored on a sign-in, whose access token lasts eight hours and renews
//!   itself.
//! - **A sign-in (OAuth):** the refresh token's expiry, stored with the
//!   sign-in (GitHub App user tokens: about six months; GitLab states none).
//!
//! # Said once
//!
//! A warning is a notification at 72 h and another at 24 h, each ONCE per
//! credential (keyed on the account and the expiry, in the persisted ledger),
//! and a line in the popover's strip for as long as it is true.

use serde::{Deserialize, Serialize};

/// When to warn, in hours before an expiry, widest first.
pub const WARN_HOURS: [u32; 2] = [72, 24];

/// How close to the local clock a GitHub expiry header may be and still be
/// believed, in seconds.
///
/// ⚠️ go-github #3708 reports fine-grained tokens answering with the CURRENT
/// time in `github-authentication-token-expiration` rather than the token's
/// expiry. Believed, that is "expires in 0s" on every response, a warning on
/// the first and "expired" on the next. A token that really has two minutes
/// left is refused soon enough to be said then.
pub const HEADER_NOW_SLACK_SECS: i64 = 120;

/// Which credential an expiry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpiryOf {
    /// A token the user made and saved: replaced in Settings.
    Token,
    /// A sign-in's refresh token: renewed by signing in again.
    SignIn,
}

/// When a credential stops working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expiry {
    /// Unix seconds.
    pub at: i64,
    /// Which credential.
    pub of: ExpiryOf,
}

/// Why the server refused the credential the last time it was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A 401 that says no more.
    Invalid,
    /// A 401 that says the token was revoked.
    Revoked,
    /// A 401 that says the token has expired.
    Expired,
    /// A sign-in whose refresh was refused: it ran out or was withdrawn,
    /// which its refresh expiry tells apart.
    SignInEnded,
}

/// What a client has seen of its credential. Never the credential.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Observed {
    /// When it stops working, when the provider said.
    pub expiry: Option<Expiry>,
    /// The server's last refusal, or `None` once a request is accepted again.
    pub refused: Option<Refusal>,
}

/// One client's [`Observed`], shared by its clones. Written by every request
/// the client sends, read by the poller after each tick.
#[derive(Debug, Clone, Default)]
pub struct CredentialProbe(std::sync::Arc<std::sync::Mutex<Observed>>);

impl CredentialProbe {
    fn with<R>(&self, f: impl FnOnce(&mut Observed) -> R) -> R {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut held)
    }

    /// What has been seen so far.
    pub fn get(&self) -> Observed {
        self.with(|o| *o)
    }

    /// The credential was accepted: any earlier refusal is over.
    pub fn accepted(&self) {
        self.with(|o| o.refused = None);
    }

    /// The credential was refused, for this reason.
    pub fn refused(&self, why: Refusal) {
        self.with(|o| o.refused = Some(why));
    }

    /// The provider stated an expiry.
    pub fn expires(&self, expiry: Expiry) {
        self.with(|o| o.expiry = Some(expiry));
    }

    /// The provider said, or a sign-in holds, that it does not expire.
    pub fn never_expires(&self) {
        self.with(|o| o.expiry = None);
    }
}

/// What a sign-in's transport holds of the sign-in: see
/// [`crate::client::Transport::sign_in`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInNote {
    /// When the sign-in stops working unless the user signs in again, Unix
    /// seconds: the refresh token's expiry, or for a set that cannot renew
    /// itself the access token's. `None` when neither is stated, or nothing
    /// has been read yet.
    pub ends_at: Option<i64>,
    /// Who signed in, when the sign-in says.
    pub login: Option<String>,
}

/// An account's credential health, when there is something to say. A
/// healthy credential has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// It works, and stops within [`WARN_HOURS`].
    Expiring,
    /// It stopped at its expiry.
    Expired,
    /// It was withdrawn.
    Revoked,
    /// The server does not accept it.
    Invalid,
}

impl HealthState {
    /// The word for the dedupe key and the UI.
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthState::Expiring => "expiring",
            HealthState::Expired => "expired",
            HealthState::Revoked => "revoked",
            HealthState::Invalid => "invalid",
        }
    }
}

/// One account's token health, as the popover's strip and a notification
/// show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenHealth {
    /// The `[accounts.*]` key.
    pub account: String,
    /// What is wrong, or about to be.
    pub state: HealthState,
    /// Which credential: a saved token, or a sign-in.
    pub of: ExpiryOf,
    /// When it stops (stopped) working, Unix seconds, when known.
    pub expires_at: Option<i64>,
    /// For `expiring`, the warning it is inside: 72 or 24.
    pub warn_hours: Option<u32>,
    /// The sentence to show, with what to do.
    pub message: String,
}

/// What the observations of one account's client mean at `now`, or `None`
/// when there is nothing to say.
pub fn assess(account: &str, observed: &Observed, now: i64) -> Option<TokenHealth> {
    let expiry = observed.expiry;
    let past = expiry.is_some_and(|e| e.at <= now);
    let of = expiry.map(|e| e.of).unwrap_or(match observed.refused {
        Some(Refusal::SignInEnded) => ExpiryOf::SignIn,
        _ => ExpiryOf::Token,
    });
    let mut warn_hours = None;
    let state = match observed.refused {
        Some(Refusal::Revoked) => HealthState::Revoked,
        Some(Refusal::Expired) => HealthState::Expired,
        Some(Refusal::SignInEnded) | Some(Refusal::Invalid) if past => HealthState::Expired,
        Some(Refusal::SignInEnded) => HealthState::Revoked,
        Some(Refusal::Invalid) => HealthState::Invalid,
        None => {
            // ⚠ Accepted at or past the stated expiry is still "expiring", in
            // its last warning: GitLab's date can be read as the end of that
            // day too, and "expired" for a token that works is a false alarm.
            warn_hours = Some(warning_for(expiry?.at - now)?);
            HealthState::Expiring
        }
    };
    let expires_at = expiry.map(|e| e.at);
    Some(TokenHealth {
        account: account.to_string(),
        message: sentence(account, state, of, expires_at, now),
        state,
        of,
        expires_at,
        warn_hours,
    })
}

/// The narrowest warning `left` seconds is inside, or `None` past them all.
fn warning_for(left: i64) -> Option<u32> {
    WARN_HOURS
        .iter()
        .rev()
        .find(|h| left <= i64::from(**h) * 3600)
        .copied()
}

/// The strip's and the notification's sentence.
fn sentence(account: &str, state: HealthState, of: ExpiryOf, at: Option<i64>, now: i64) -> String {
    let (what, fix) = match of {
        ExpiryOf::Token => (
            "token",
            "save a new token in Settings (Accounts)".to_string(),
        ),
        // The app's words: the strip and the notification are in the app,
        // and `auth status` gives the CLI's own command.
        ExpiryOf::SignIn => (
            "sign-in",
            "sign in again in Settings (Accounts)".to_string(),
        ),
    };
    let when = |at: i64| stamp(at);
    match (state, at) {
        (HealthState::Expiring, Some(at)) if at > now => format!(
            "Account \"{account}\": the {what} expires in {} ({}); before then, {fix}.",
            span((at - now).unsigned_abs()),
            when(at)
        ),
        (HealthState::Expiring, Some(at)) => format!(
            "Account \"{account}\": the {what} expires now ({}); {fix}.",
            when(at)
        ),
        (HealthState::Expired, Some(at)) => format!(
            "Account \"{account}\": the {what} expired {}; nothing is polled until you {fix}.",
            when(at)
        ),
        (HealthState::Expiring | HealthState::Expired, None) => format!(
            "Account \"{account}\": the {what} has expired; nothing is polled until you {fix}."
        ),
        (HealthState::Revoked, _) => format!(
            "Account \"{account}\": the {what} was revoked; nothing is polled until you {fix}."
        ),
        (HealthState::Invalid, _) => format!(
            "Account \"{account}\": the server does not accept the {what} (401); nothing is \
             polled until you {fix}."
        ),
    }
}

/// A duration in seconds, as a person says it: `2d 3h`, `5h 12m`, `40m`.
pub fn span(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// Unix seconds as `2026-11-10 00:00 UTC`.
pub fn stamp(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("{secs}"))
}

/// GitHub's `github-authentication-token-expiration`, as Unix seconds.
///
/// Two spellings are seen: `2026-11-07 08:23:13 UTC` and
/// `2025-09-05 17:55:53 +0500`. Anything else is `None`: an unreadable
/// header is "not said", never an error.
pub fn parse_github_expiration(value: &str) -> Option<i64> {
    let value = value.trim();
    let value = match value.strip_suffix(" UTC") {
        Some(rest) => format!("{rest} +0000"),
        None => value.to_string(),
    };
    chrono::DateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S %z")
        .ok()
        .map(|t| t.timestamp())
}

/// GitLab's `expires_at` (`YYYY-MM-DD`), as Unix seconds: 00:00 UTC on that
/// date, the earliest reading. `None` for anything else.
pub fn parse_gitlab_date(value: &str) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc().timestamp())
}

/// What a GitLab 401 says about the token.
///
/// GitLab's API guard answers an expired or revoked token with an OAuth error
/// body, `{"error":"invalid_token","error_description":"Token is expired. You
/// can either do re-authorization or token refresh."}` or `"Token was revoked.
/// You have to re-authorize from the user."`, and every other refusal with
/// `{"message":"401 Unauthorized"}`.
pub fn gitlab_refusal(body: &str) -> Refusal {
    #[derive(Deserialize)]
    struct OAuthBody {
        error: Option<String>,
        error_description: Option<String>,
    }
    let Ok(parsed) = serde_json::from_str::<OAuthBody>(body) else {
        return Refusal::Invalid;
    };
    if parsed.error.as_deref() != Some("invalid_token") {
        return Refusal::Invalid;
    }
    let description = parsed
        .error_description
        .unwrap_or_default()
        .to_ascii_lowercase();
    if description.contains("revoked") {
        Refusal::Revoked
    } else if description.contains("expired") {
        Refusal::Expired
    } else {
        Refusal::Invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;

    fn expiring_in(left: i64) -> Option<TokenHealth> {
        let observed = Observed {
            expiry: Some(Expiry {
                at: 1_000_000 + left,
                of: ExpiryOf::Token,
            }),
            refused: None,
        };
        assess("lab", &observed, 1_000_000)
    }

    #[test]
    fn the_warnings_are_72h_and_24h_and_nothing_before() {
        assert_eq!(expiring_in(72 * H + 1), None);
        let at = |left| expiring_in(left).map(|h| (h.state, h.warn_hours));
        assert_eq!(at(72 * H), Some((HealthState::Expiring, Some(72))));
        assert_eq!(at(24 * H + 1), Some((HealthState::Expiring, Some(72))));
        assert_eq!(at(24 * H), Some((HealthState::Expiring, Some(24))));
        // Accepted past its date: still the last warning, never "expired".
        assert_eq!(at(-H), Some((HealthState::Expiring, Some(24))));
        assert!(
            expiring_in(30 * H)
                .unwrap()
                .message
                .contains("expires in 1d 6h"),
        );
    }

    #[test]
    fn no_expiry_and_no_refusal_is_nothing_to_say() {
        assert_eq!(assess("lab", &Observed::default(), 0), None);
    }

    #[test]
    fn a_refusal_reads_against_the_expiry() {
        let with = |refused, at: Option<i64>| {
            let observed = Observed {
                expiry: at.map(|at| Expiry {
                    at,
                    of: ExpiryOf::Token,
                }),
                refused: Some(refused),
            };
            assess("a", &observed, 100).map(|h| h.state)
        };
        assert_eq!(with(Refusal::Invalid, None), Some(HealthState::Invalid));
        assert_eq!(
            with(Refusal::Invalid, Some(500)),
            Some(HealthState::Invalid)
        );
        assert_eq!(
            with(Refusal::Invalid, Some(100)),
            Some(HealthState::Expired)
        );
        assert_eq!(with(Refusal::Revoked, Some(50)), Some(HealthState::Revoked));
        assert_eq!(with(Refusal::Expired, None), Some(HealthState::Expired));
        assert_eq!(with(Refusal::SignInEnded, None), Some(HealthState::Revoked));
        assert_eq!(
            with(Refusal::SignInEnded, Some(99)),
            Some(HealthState::Expired)
        );
    }

    #[test]
    fn github_expiry_headers_parse_in_both_spellings() {
        assert_eq!(
            parse_github_expiration("2026-11-07 08:23:13 UTC"),
            Some(1_794_039_793)
        );
        assert_eq!(
            parse_github_expiration("2025-09-05 17:55:53 +0500"),
            Some(1_757_076_953)
        );
        assert_eq!(parse_github_expiration("tomorrow"), None);
        assert_eq!(parse_github_expiration(""), None);
    }

    #[test]
    fn a_gitlab_date_is_midnight_utc() {
        assert_eq!(parse_gitlab_date("2026-11-20"), Some(1_795_132_800));
        assert_eq!(parse_gitlab_date("2026-11-20T00:00:00Z"), None);
    }

    #[test]
    fn a_gitlab_401_body_names_revoked_and_expired() {
        assert_eq!(
            gitlab_refusal(
                r#"{"error":"invalid_token","error_description":"Token was revoked. You have to re-authorize from the user."}"#
            ),
            Refusal::Revoked
        );
        assert_eq!(
            gitlab_refusal(
                r#"{"error":"invalid_token","error_description":"Token is expired. You can either do re-authorization or token refresh."}"#
            ),
            Refusal::Expired
        );
        assert_eq!(
            gitlab_refusal(r#"{"message":"401 Unauthorized"}"#),
            Refusal::Invalid
        );
        assert_eq!(gitlab_refusal("<html>"), Refusal::Invalid);
        assert_eq!(
            gitlab_refusal(r#"{"error":"insufficient_scope","error_description":"expired"}"#),
            Refusal::Invalid
        );
    }
}
