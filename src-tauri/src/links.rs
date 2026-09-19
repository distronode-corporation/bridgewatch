//! The one door to the user's browser.
//!
//! Every URL the app opens goes through [`open`]: the popover's pipeline and
//! job links, the tray's "Open pipelines page", and anything added later. It
//! opens a URL only when its scheme, host and port are those of a configured
//! account's `base_url`, or, for a github account, of the web origin that
//! `base_url` implies.
//!
//! ⛔ A github account's `base_url` is its API host (`https://api.github.com`),
//! while every link GitHub hands out (a run's `html_url`, a commit's checks
//! page, the Actions index) is on the WEB host (`https://github.com`). Trusting
//! only `base_url` refused every one of them. The web origin comes from
//! [`github::web_origin`], the same rule that built those URLs, so the two
//! cannot drift: one leading `api.` comes off (github.com and a GHE.com
//! data-residency host), and GitHub Enterprise Server keeps its host. Either
//! way it is derived from the account's own configured host, never widened.
//!
//! Why here and not in the opener plugin's capability scope:
//!
//! - The scope was `https://*`, which refused every link on a self-managed
//!   `http://` instance (a `base_url` the config accepts), and allowed every
//!   https URL on the internet.
//! - The URLs come from GitLab's API (`web_url`), so a hostile or compromised
//!   instance chooses them; `file://` and `javascript:` included. The tray's
//!   menu item handed those to the Rust opener, which has no scope at all.
//! - The allowed hosts are the ACCOUNTS, which only the configuration knows,
//!   and a static capability file cannot say "whatever the config says".
//!
//! The webview is granted no opener permission at all now; it asks the shell.

use bridgewatch_core::client::github;
use bridgewatch_core::config::{Account, Config, Provider};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use url::Url;

/// Check `candidate` against the configured accounts. Returns the parsed URL
/// on success and a sentence for the log on refusal.
///
/// The shell itself opens through [`open_trusting`]; this, the same check
/// with nothing extra trusted, is what the tests hold to the rules above.
#[cfg(test)]
pub fn check(candidate: &str, config: Option<&Config>) -> Result<Url, String> {
    check_with(candidate, config, &[])
}

/// [`check`], also trusting the hosts of `extra`, for ONE call.
///
/// A sign-in opens its verification page before the account it signs in for
/// is in the file (the wizard signs in first and writes the file last), so the
/// account being signed in is trusted for that one open, by exactly the same
/// origin rule, and nowhere else. The page's URL has already been held to that
/// host by the core (`oauth::device::start`), so this is the second check and
/// not the only one.
pub fn check_with(
    candidate: &str,
    config: Option<&Config>,
    extra: &[Account],
) -> Result<Url, String> {
    let url = Url::parse(candidate.trim()).map_err(|e| format!("not a URL ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("the {:?} scheme is never opened", url.scheme()));
    }
    if config.is_none() && extra.is_empty() {
        return Err("no configuration is loaded, so no host is trusted".into());
    }
    let configured = config.into_iter().flat_map(|c| c.accounts.values());
    let allowed = configured.chain(extra.iter()).any(|account| {
        trusted_origins(account)
            .iter()
            .any(|origin| Url::parse(origin).is_ok_and(|base| same_origin(&base, &url)))
    });
    if allowed {
        Ok(url)
    } else {
        Err(format!(
            "{} is not the host of any configured account",
            url.host_str().unwrap_or("(no host)")
        ))
    }
}

/// The origins an account's links may be on: its `base_url`, and for a github
/// account the web origin as well. A GitLab account's list is exactly what it
/// always was.
fn trusted_origins(account: &Account) -> Vec<String> {
    let base = account.base_url.trim().to_string();
    match account.provider {
        Provider::Gitlab => vec![base],
        Provider::Github => {
            let web = github::web_origin(&base);
            vec![base, web]
        }
    }
}

/// Scheme, host (case-insensitively, as `Url` already lowercases it) and
/// effective port. A path prefix on `base_url` (GitLab under `/gitlab`) is not
/// required of the link: the host is the trust boundary, not the path.
fn same_origin(base: &Url, url: &Url) -> bool {
    base.scheme() == url.scheme()
        && base.host_str().is_some()
        && base.host_str() == url.host_str()
        && base.port_or_known_default() == url.port_or_known_default()
}

/// Open `candidate` in the default browser if [`check`] allows it.
pub fn open(app: &AppHandle, candidate: &str, config: Option<&Config>) -> Result<(), String> {
    open_trusting(app, candidate, config, &[])
}

/// [`open`], also trusting the hosts of `extra` for this call only. See
/// [`check_with`].
pub fn open_trusting(
    app: &AppHandle,
    candidate: &str,
    config: Option<&Config>,
    extra: &[Account],
) -> Result<(), String> {
    let url = match check_with(candidate, config, extra) {
        Ok(url) => url,
        Err(why) => {
            tracing::warn!(reason = %why, "refused to open a link");
            return Err(format!("not opened: {why}"));
        }
    };
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn config(bases: &[&str]) -> Config {
        let raw: String = bases
            .iter()
            .enumerate()
            .map(|(i, b)| format!("[accounts.a{i}]\nbase_url = \"{b}\"\n"))
            .collect();
        bridgewatch_core::config::parse_str(&raw, Path::new("x.toml"))
            .expect("fixture parses")
            .config
    }

    #[test]
    fn a_link_on_a_configured_host_is_opened() {
        let c = config(&["https://gitlab.com"]);
        assert!(check("https://gitlab.com/g/p/-/pipelines/12", Some(&c)).is_ok());
        assert!(check("https://GITLAB.com:443/g/p", Some(&c)).is_ok());
    }

    #[test]
    fn a_plain_http_self_managed_instance_works() {
        let c = config(&["http://gitlab.lan:8080/gitlab"]);
        assert!(check("http://gitlab.lan:8080/gitlab/g/p/-/jobs/3", Some(&c)).is_ok());
        // But not on another port, or upgraded, or downgraded.
        assert!(check("http://gitlab.lan/g/p", Some(&c)).is_err());
        assert!(check("https://gitlab.lan:8080/g/p", Some(&c)).is_err());
    }

    #[test]
    fn other_hosts_and_schemes_are_refused() {
        let c = config(&["https://gitlab.com"]);
        for bad in [
            "https://evil.example/g/p",
            "https://gitlab.com.evil.example/",
            "https://evil.example/@gitlab.com",
            "http://gitlab.com/g/p",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "",
        ] {
            assert!(check(bad, Some(&c)).is_err(), "{bad:?} was allowed");
        }
    }

    #[test]
    fn userinfo_does_not_change_the_host() {
        let c = config(&["https://gitlab.com"]);
        assert!(check("https://gitlab.com@evil.example/", Some(&c)).is_err());
    }

    fn github(base: Option<&str>) -> Config {
        let base = base
            .map(|b| format!("base_url = \"{b}\"\n"))
            .unwrap_or_default();
        bridgewatch_core::config::parse_str(
            &format!("[accounts.gh]\nprovider = \"github\"\n{base}"),
            Path::new("x.toml"),
        )
        .expect("fixture parses")
        .config
    }

    /// The three links the app opens for a github.com account: a run's
    /// `html_url`, a commit group's checks page and the tray's Actions index.
    /// All three are on the WEB host, not on the API host `base_url` names.
    #[test]
    fn a_github_com_account_opens_its_links_on_the_web_host() {
        let c = github(None);
        for good in [
            "https://github.com/acme-corp/monorepo/actions/runs/4103",
            "https://github.com/acme-corp/monorepo/commit/0f1e2d3c/checks",
            "https://github.com/acme-corp/monorepo/actions",
            "https://api.github.com/repos/acme-corp/monorepo",
        ] {
            assert!(check(good, Some(&c)).is_ok(), "{good:?} was refused");
        }
    }

    #[test]
    fn a_github_enterprise_server_account_trusts_its_one_host() {
        let c = github(Some("https://ghe.acme.com"));
        assert!(check("https://ghe.acme.com/o/r/actions/runs/9", Some(&c)).is_ok());
        assert!(check("https://acme.com/o/r", Some(&c)).is_err());
        assert!(check("https://github.com/o/r", Some(&c)).is_err());
    }

    #[test]
    fn a_data_residency_account_trusts_its_web_and_api_hosts() {
        let c = github(Some("https://api.acme.ghe.com"));
        assert!(check("https://acme.ghe.com/o/r/actions/runs/9", Some(&c)).is_ok());
        assert!(check("https://api.acme.ghe.com/repos/o/r", Some(&c)).is_ok());
        assert!(check("https://ghe.com/o/r", Some(&c)).is_err());
        assert!(check("https://other.ghe.com/o/r", Some(&c)).is_err());
        assert!(check("https://github.com/o/r", Some(&c)).is_err());
    }

    #[test]
    fn look_alike_hosts_are_refused_for_a_github_account() {
        let c = github(None);
        for bad in [
            "https://github.com.evil.example/o/r",
            "https://evilgithub.com/o/r",
            "https://api.github.com.evil.example/",
            "https://github.com@evil.example/o/r",
            "http://github.com/o/r",
            "https://github.com:8443/o/r",
            "https://gist.github.com/o",
        ] {
            assert!(check(bad, Some(&c)).is_err(), "{bad:?} was allowed");
        }
    }

    /// The web origin is a github rule only: a GitLab account whose host
    /// happens to start with `api.` does not gain the bare domain.
    #[test]
    fn a_gitlab_account_gains_no_web_origin() {
        let c = config(&["https://api.gitlab.example"]);
        assert!(check("https://api.gitlab.example/g/p", Some(&c)).is_ok());
        assert!(check("https://gitlab.example/g/p", Some(&c)).is_err());
        let c = config(&["https://gitlab.com"]);
        assert!(check("https://github.com/o/r", Some(&c)).is_err());
    }

    #[test]
    fn nothing_is_opened_without_a_configuration() {
        assert!(check("https://gitlab.com/", None).is_err());
    }

    /// The wizard signs in before any account is written, so the account
    /// being signed in is trusted for the one open, by the same origin rule:
    /// github.com's device page, and not a look-alike or another host.
    #[test]
    fn a_sign_in_trusts_its_own_account_for_one_call() {
        let github = Account::for_provider(Provider::Github);
        assert!(
            check_with(
                "https://github.com/login/device",
                None,
                std::slice::from_ref(&github)
            )
            .is_ok()
        );
        for bad in [
            "https://github.com.evil.example/login/device",
            "https://evil.example/login/device",
            "http://github.com/login/device",
        ] {
            assert!(
                check_with(bad, None, std::slice::from_ref(&github)).is_err(),
                "{bad:?} was allowed"
            );
        }
        // And only for that call: the plain check still trusts nothing.
        assert!(check("https://github.com/login/device", None).is_err());

        let gitlab = Account {
            base_url: "https://gitlab.example.com".into(),
            ..Account::for_provider(Provider::Gitlab)
        };
        assert!(
            check_with(
                "https://gitlab.example.com/oauth/device",
                None,
                std::slice::from_ref(&gitlab)
            )
            .is_ok()
        );
        assert!(check_with("https://gitlab.com/oauth/device", None, &[gitlab]).is_err());
    }
}
