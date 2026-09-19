//! `bridgewatch auth`: sign in, sign out and see who is signed in, for an
//! account whose token source is `token = { oauth = .. }`.
//!
//! The same device flow the app's wizard runs, in a terminal: print the code
//! and the page, poll, store. Every function here takes its transport, its
//! credential store, its sleep and its output as arguments, which is what lets
//! the tests at the bottom run a whole sign-in against a script with no network
//! and no keychain.
//!
//! ⛔ Nothing printed here is a token. `status` prints who, until when and with
//! what scopes; the user code is printed because the user has to type it.

use std::future::Future;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use bridgewatch_core::client::Transport;
use bridgewatch_core::config::{Account, Config, OAuthSource, TokenSource};
use bridgewatch_core::oauth::{self, BuiltinClients, OAuthError, Progress, store};
use bridgewatch_core::token::TokenProvider;
use clap::Subcommand;

use crate::{Failure, Outcome, exit};

/// `bridgewatch auth ...`.
#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Sign in with GitHub or GitLab (the OAuth device flow) and store the
    /// sign-in in the OS credential store. The account's token source must be
    /// token = { oauth = true } or { oauth = { client_id = "..." } }.
    Login {
        /// The [accounts.<name>] key to sign in for.
        #[arg(long)]
        account: String,
    },
    /// Forget an account's sign-in: delete it from the credential store.
    Logout {
        /// The [accounts.<name>] key.
        #[arg(long)]
        account: String,
    },
    /// Show who each signing-in account is signed in as, until when and with
    /// what scopes. Never a token. Exits 70 when one is not signed in.
    Status {
        /// Only this account.
        #[arg(long)]
        account: Option<String>,
    },
}

/// The configured account named `name`, or a usage error listing the ones
/// that exist.
fn account<'a>(config: &'a Config, name: &str) -> Result<&'a Account, Failure> {
    config.accounts.get(name).ok_or_else(|| {
        Failure::new(
            exit::USAGE,
            anyhow::anyhow!(
                "no account {name:?}; configured accounts are [{}]",
                config
                    .accounts
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })
}

/// The account's `oauth` source, or a configuration error saying what to
/// write.
fn oauth_source<'a>(name: &str, account: &'a Account) -> Result<&'a OAuthSource, Failure> {
    match &account.token {
        TokenSource::Oauth(source) => Ok(source),
        _ => Err(Failure::new(
            exit::CONFIG,
            anyhow::anyhow!(
                "account {name:?} does not sign in: its token source is not oauth. Write \
                 token = {{ oauth = true }} in [accounts.{name}] (or \
                 {{ oauth = {{ client_id = \"...\" }} }} for your own application) first"
            ),
        )),
    }
}

/// Map a sign-in error onto an exit code: a missing client id is the
/// configuration's fault, everything else happened before any verdict.
fn failed(e: OAuthError) -> Failure {
    let code = match e {
        OAuthError::NoClientId(_) => exit::CONFIG,
        _ => exit::ERROR,
    };
    Failure::new(code, anyhow::anyhow!("{e}"))
}

/// `auth login`: the device flow, in the terminal.
#[allow(clippy::too_many_arguments)]
pub async fn login<S, F, C>(
    config: &Config,
    name: &str,
    transport: Arc<dyn Transport>,
    store_provider: &dyn TokenProvider,
    builtin: &BuiltinClients,
    sleep: S,
    cancel: C,
    now: u64,
    out: &mut (dyn Write + Send),
) -> Outcome
where
    S: Fn(Duration) -> F,
    F: Future<Output = ()>,
    C: Future<Output = ()>,
{
    let account = account(config, name)?;
    let source = oauth_source(name, account)?;
    let client_id = oauth::client_id_for(account, source, builtin).map_err(failed)?;
    let endpoints = oauth::Endpoints::for_account(account);
    let device = oauth::start(transport.as_ref(), &endpoints, &client_id)
        .await
        .map_err(failed)?;

    writeln!(
        out,
        "To sign in to {} as account \"{name}\":",
        endpoints.host
    )?;
    writeln!(out, "  1. Open {}", device.verification_uri)?;
    writeln!(out, "  2. Enter the code {}", device.user_code)?;
    writeln!(
        out,
        "Waiting for you to approve it (the code lasts {}; Ctrl-C stops)...",
        span(device.expires_in)
    )?;
    out.flush()?;

    let mut progress = |p: Progress| {
        if let Progress::SlowedDown { interval } = p {
            tracing::debug!(interval, "the server asked for slower polling");
        }
    };
    let reply = oauth::wait_for_token(
        transport.as_ref(),
        &endpoints,
        &client_id,
        &device,
        sleep,
        cancel,
        &mut progress,
    )
    .await
    .map_err(failed)?;
    let (_, signed_in) = oauth::complete_sign_in(
        name,
        account,
        &client_id,
        reply,
        transport,
        store_provider,
        now,
    )
    .await
    .map_err(failed)?;
    match signed_in.login {
        Some(login) => writeln!(out, "Signed in to {} as @{login}.", endpoints.host)?,
        None => writeln!(
            out,
            "Signed in to {}. (Who you are could not be asked; the first poll will say \
             whether the sign-in works.)",
            endpoints.host
        )?,
    }
    Ok(exit::OK)
}

/// `auth logout`.
pub fn logout(
    config: &Config,
    name: &str,
    store_provider: &dyn TokenProvider,
    out: &mut dyn Write,
) -> Outcome {
    account(config, name)?;
    store::delete(name, store_provider).map_err(failed)?;
    writeln!(
        out,
        "Signed out: the sign-in of account \"{name}\" is removed from the credential store."
    )?;
    Ok(exit::OK)
}

/// `auth status`.
pub fn status(
    config: &Config,
    only: Option<&str>,
    store_provider: &dyn TokenProvider,
    now: u64,
    out: &mut dyn Write,
) -> Outcome {
    let names: Vec<String> = match only {
        Some(name) => {
            oauth_source(name, account(config, name)?)?;
            vec![name.to_string()]
        }
        None => config
            .accounts
            .iter()
            .filter(|(_, a)| matches!(a.token, TokenSource::Oauth(_)))
            .map(|(k, _)| k.clone())
            .collect(),
    };
    if names.is_empty() {
        writeln!(
            out,
            "No account signs in. An account does when its token source is token = {{ oauth = true }}."
        )?;
        return Ok(exit::OK);
    }
    let mut all_signed_in = true;
    for name in &names {
        let host = oauth::Endpoints::for_account(&config.accounts[name]).host;
        match store::load(name, store_provider) {
            Ok(Some(set)) => {
                let s = set.status();
                let who = s
                    .login
                    .as_deref()
                    .map(|l| format!("@{l}"))
                    .unwrap_or_else(|| "an unknown user".to_string());
                let expiry = match s.expires_at {
                    Some(at) if at > now => format!(
                        "the access token expires in {} ({})",
                        span(at - now),
                        stamp(at)
                    ),
                    Some(at) => format!("the access token expired at {}", stamp(at)),
                    None => "the access token does not expire".to_string(),
                };
                let renews = match (s.can_refresh, s.refresh_expires_at) {
                    (true, Some(at)) => format!(", renewed automatically until {}", stamp(at)),
                    (true, None) => ", renewed automatically".to_string(),
                    (false, _) => String::new(),
                };
                let scopes = if s.scopes.is_empty() {
                    "none reported".to_string()
                } else {
                    s.scopes.join(", ")
                };
                writeln!(
                    out,
                    "{name} ({host}): signed in as {who}; {expiry}{renews}; scopes: {scopes}"
                )?;
            }
            Ok(None) => {
                all_signed_in = false;
                writeln!(
                    out,
                    "{name} ({host}): not signed in; run bridgewatch auth login --account {name}"
                )?;
            }
            Err(e) => {
                all_signed_in = false;
                writeln!(out, "{name} ({host}): {e}")?;
            }
        }
    }
    Ok(if all_signed_in { exit::OK } else { exit::ERROR })
}

/// A duration in words: `7h 12m`, `14m`, `45s`.
fn span(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// A Unix timestamp as RFC 3339, UTC.
fn stamp(secs: u64) -> String {
    let secs = i64::try_from(secs).unwrap_or(i64::MAX);
    chrono_like(secs)
}

/// RFC 3339 without a date crate in this binary: civil-from-days (Howard
/// Hinnant's algorithm), which is exact for every date a token can expire on.
fn chrono_like(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use bridgewatch_core::client::ScriptTransport;
    use bridgewatch_core::token::TokenError;

    use super::*;

    /// A credential store in memory. Nothing here reaches the real keychain.
    #[derive(Default)]
    struct Memory(Mutex<BTreeMap<(String, String), String>>);

    impl TokenProvider for Memory {
        fn keyring_get(&self, service: &str, user: &str) -> Result<String, TokenError> {
            self.0
                .lock()
                .unwrap()
                .get(&(service.into(), user.into()))
                .cloned()
                .ok_or_else(|| TokenError::NoEntry {
                    service: service.into(),
                    user: user.into(),
                })
        }
        fn keyring_set(&self, service: &str, user: &str, secret: &str) -> Result<(), TokenError> {
            self.0
                .lock()
                .unwrap()
                .insert((service.into(), user.into()), secret.into());
            Ok(())
        }
        fn keyring_delete(&self, service: &str, user: &str) -> Result<(), TokenError> {
            self.0
                .lock()
                .unwrap()
                .remove(&(service.into(), user.into()));
            Ok(())
        }
        fn env(&self, _: &str) -> Option<String> {
            None
        }
        fn run(&self, _: &[String]) -> Result<String, TokenError> {
            Err(TokenError::Empty)
        }
    }

    const BUILTIN: BuiltinClients = BuiltinClients {
        github_com: Some("Iv23liEXAMPLE"),
        github_app_slug: Some("bridgewatch"),
        gitlab_com: Some("gitlab-app-id"),
    };

    fn config(raw: &str) -> Config {
        toml::from_str(raw).expect("the test config deserialises")
    }

    fn github() -> Config {
        config(
            "[accounts.gh]\nprovider = \"github\"\ntoken = { oauth = true }\n\n\
             [accounts.pat]\nprovider = \"github\"\ntoken = { env = \"GH\" }\n",
        )
    }

    fn scripted_github_sign_in() -> Arc<ScriptTransport> {
        let t = Arc::new(ScriptTransport::new());
        t.reply(
            "/login/device/code",
            200,
            r#"{"device_code":"DEVICE-SECRET","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
        );
        t.reply(
            "/login/oauth/access_token",
            200,
            r#"{"error":"authorization_pending"}"#,
        );
        t.reply(
            "/login/oauth/access_token",
            200,
            r#"{"access_token":"ghu_ACCESS","expires_in":28800,"refresh_token":"ghr_REFRESH","refresh_token_expires_in":15897600,"token_type":"bearer","scope":""}"#,
        );
        t.reply("api.github.com/user", 200, r#"{"id":1,"login":"octocat"}"#);
        t
    }

    #[tokio::test]
    async fn login_prints_the_code_and_page_then_stores_the_sign_in() {
        let transport = scripted_github_sign_in();
        let store = Memory::default();
        let mut out = Vec::new();
        let code = login(
            &github(),
            "gh",
            transport.clone(),
            &store,
            &BUILTIN,
            |_| std::future::ready(()),
            std::future::pending(),
            1_000,
            &mut out,
        )
        .await
        .expect("signs in");
        assert_eq!(code, exit::OK);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("Open https://github.com/login/device"),
            "{text}"
        );
        assert!(text.contains("Enter the code WDJB-MJHT"), "{text}");
        assert!(
            text.contains("Signed in to github.com as @octocat."),
            "{text}"
        );
        for secret in ["DEVICE-SECRET", "ghu_ACCESS", "ghr_REFRESH"] {
            assert!(!text.contains(secret), "{secret} was printed: {text}");
        }
        let set = store::load("gh", &store).unwrap().expect("stored");
        assert_eq!(set.access_token.expose(), "ghu_ACCESS");
        assert_eq!(set.login.as_deref(), Some("octocat"));
        assert_eq!(transport.remaining(), 0);
    }

    #[tokio::test]
    async fn login_on_an_account_that_does_not_sign_in_is_a_config_error() {
        let failure = login(
            &github(),
            "pat",
            Arc::new(ScriptTransport::new()),
            &Memory::default(),
            &BUILTIN,
            |_| std::future::ready(()),
            std::future::pending(),
            0,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.code, exit::CONFIG);
        assert!(
            failure.error.to_string().contains("oauth = true"),
            "{}",
            failure.error
        );

        let unknown = login(
            &github(),
            "nope",
            Arc::new(ScriptTransport::new()),
            &Memory::default(),
            &BUILTIN,
            |_| std::future::ready(()),
            std::future::pending(),
            0,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(unknown.code, exit::USAGE);
    }

    /// While this build has no built-in application, github.com cannot be
    /// signed in to without a client id of the user's own, and it is the
    /// configuration's fault, said before any request.
    #[tokio::test]
    async fn login_without_a_client_id_names_what_to_write() {
        let transport = Arc::new(ScriptTransport::new());
        let failure = login(
            &github(),
            "gh",
            transport.clone(),
            &Memory::default(),
            &BuiltinClients::default(),
            |_| std::future::ready(()),
            std::future::pending(),
            0,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.code, exit::CONFIG);
        assert!(
            failure.error.to_string().contains("client_id"),
            "{}",
            failure.error
        );
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn a_refused_sign_in_exits_70_in_plain_words() {
        let t = Arc::new(ScriptTransport::new());
        t.reply(
            "/login/device/code",
            200,
            r#"{"device_code":"D","user_code":"U","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
        );
        t.reply(
            "/login/oauth/access_token",
            200,
            r#"{"error":"access_denied"}"#,
        );
        let failure = login(
            &github(),
            "gh",
            t,
            &Memory::default(),
            &BUILTIN,
            |_| std::future::ready(()),
            std::future::pending(),
            0,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.code, exit::ERROR);
        assert!(
            failure
                .error
                .to_string()
                .contains("cancelled on github.com"),
            "{}",
            failure.error
        );
    }

    #[tokio::test]
    async fn status_shows_who_and_until_when_and_never_a_token() {
        let store = Memory::default();
        login(
            &github(),
            "gh",
            scripted_github_sign_in(),
            &store,
            &BUILTIN,
            |_| std::future::ready(()),
            std::future::pending(),
            1_000,
            &mut Vec::new(),
        )
        .await
        .unwrap();

        let mut out = Vec::new();
        let code = status(&github(), None, &store, 1_000 + 3600, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(code, exit::OK, "{text}");
        assert!(
            text.contains("gh (github.com): signed in as @octocat"),
            "{text}"
        );
        assert!(text.contains("expires in 7h 0m"), "{text}");
        assert!(text.contains("renewed automatically until"), "{text}");
        assert!(
            !text.contains("pat"),
            "only signing-in accounts are listed: {text}"
        );
        for secret in ["ghu_ACCESS", "ghr_REFRESH"] {
            assert!(!text.contains(secret), "{text}");
        }

        let mut out = Vec::new();
        logout(&github(), "gh", &store, &mut out).unwrap();
        let mut out = Vec::new();
        let code = status(&github(), Some("gh"), &store, 0, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(code, exit::ERROR, "{text}");
        assert!(
            text.contains("not signed in; run bridgewatch auth login --account gh"),
            "{text}"
        );
    }

    #[test]
    fn status_of_an_account_that_does_not_sign_in_is_a_config_error() {
        let failure = status(
            &github(),
            Some("pat"),
            &Memory::default(),
            0,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(failure.code, exit::CONFIG);
    }

    #[test]
    fn a_timestamp_reads_as_utc() {
        assert_eq!(stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(stamp(1_789_669_380), "2026-09-17T18:23:00Z");
        assert_eq!(span(28_800), "8h 0m");
        assert_eq!(span(59), "59s");
        assert_eq!(span(15_897_600), "184d 0h");
    }
}
