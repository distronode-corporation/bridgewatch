//! The one door to the user's browser.
//!
//! Every URL the app opens goes through [`open`]: the popover's pipeline and
//! job links, the tray's "Open pipelines page", and anything added later. It
//! opens a URL only when its scheme, host and port are those of a configured
//! account's `base_url`.
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

use bridgewatch_core::config::Config;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use url::Url;

/// Check `candidate` against the configured accounts. Returns the parsed URL
/// on success and a sentence for the log on refusal.
pub fn check(candidate: &str, config: Option<&Config>) -> Result<Url, String> {
    let url = Url::parse(candidate.trim()).map_err(|e| format!("not a URL ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("the {:?} scheme is never opened", url.scheme()));
    }
    let Some(config) = config else {
        return Err("no configuration is loaded, so no host is trusted".into());
    };
    let allowed = config.accounts.values().any(|account| {
        Url::parse(account.base_url.trim()).is_ok_and(|base| same_origin(&base, &url))
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
    let url = match check(candidate, config) {
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

    #[test]
    fn nothing_is_opened_without_a_configuration() {
        assert!(check("https://gitlab.com/", None).is_err());
    }
}
