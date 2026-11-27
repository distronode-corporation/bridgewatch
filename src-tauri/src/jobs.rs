//! The popover's two job commands: the end of a failed job's log, and a job
//! action (retry, play).
//!
//! Both name the job the way the popover knows it, by its watch and its page
//! URL, and resolve it through the core ([`actions::locate`]) against the
//! watch's OWN account, so a URL the popover was handed cannot pick some other
//! account's token. Both are sent through the running poller's client for that
//! account: the same in-flight bound, the same request ring (the Debug section
//! shows them), the same sign-in.
//!
//! ⛔ No decision about whether a write is allowed is made here. The client
//! refuses unless the account has `actions = true`, and that refusal comes back
//! as the error string like any other; the popover only hides the button.

use std::sync::Arc;

use bridgewatch_core::actions::{self, JobAction, JobTarget};
use bridgewatch_core::client::{CiClient, JobActionOutcome, LogTail};
use bridgewatch_core::poll::PollNow;
use tauri::State;

use crate::state::AppState;

type Shared<'a> = State<'a, Arc<AppState>>;

/// The job the popover means, and the client to reach it through.
fn resolve(
    state: &AppState,
    watch: &str,
    url: &str,
) -> Result<(JobTarget, Arc<dyn CiClient>), String> {
    let config = state
        .config()
        .ok_or_else(|| "no configuration is loaded".to_string())?;
    let account = config
        .watches
        .iter()
        .find(|w| w.id == watch)
        .map(|w| w.account.clone())
        .ok_or_else(|| format!("no watch with id {watch:?}"))?;
    let target = actions::locate(&config, url, Some(&account)).map_err(|e| e.to_string())?;
    let client = state.client(&target.account).ok_or_else(|| {
        format!(
            "account {:?} is not being polled yet; try again in a moment",
            target.account
        )
    })?;
    Ok((target, client))
}

/// The end of one job's log, cleaned for display as text.
#[tauri::command]
pub async fn job_log_tail(
    watch: String,
    url: String,
    state: Shared<'_>,
) -> Result<LogTail, String> {
    let (target, client) = resolve(&state, &watch, &url)?;
    actions::log_tail(client.as_ref(), &target)
        .await
        .map_err(|e| e.to_string())
}

/// Retry or play one job, then poll at once so the popover shows it moving.
#[tauri::command]
pub async fn job_action(
    watch: String,
    url: String,
    action: JobAction,
    state: Shared<'_>,
) -> Result<JobActionOutcome, String> {
    let (target, client) = resolve(&state, &watch, &url)?;
    let outcome = actions::perform(client.as_ref(), &target, action)
        .await
        .map_err(|e| e.to_string())?;
    state.request_poll(PollNow::JobAction);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridgewatch_core::client::RequestRing;
    use bridgewatch_core::token::{Secret, SystemTokenProvider};
    use tauri::Manager;

    /// Every way the popover's job can fail to resolve, each in its own
    /// words, and a job the instance refuses to show or act on: the refusal
    /// comes back as the command's error, and nothing is polled for it.
    #[test]
    fn a_job_resolves_through_its_watch_and_a_refusal_is_the_answer() {
        let base = crate::wizard::tests::refusing_server(4);
        let raw = format!(
            "[accounts.gl]\nbase_url = \"{base}\"\ntoken = {{ env = \"BW_TEST_UNSET\" }}\n\
             [[watches]]\nid = \"w\"\naccount = \"gl\"\nproject = \"g/p\"\nref = \"main\"\n"
        );
        let config = bridgewatch_core::config::parse_str(&raw, std::path::Path::new("x.toml"))
            .unwrap()
            .config;
        let app = tauri::test::mock_app();
        app.manage(Arc::new(AppState::new(crate::config::Resolved {
            path: std::path::PathBuf::from("/nonexistent/config.toml"),
            seeded: false,
            first_run: false,
            loaded: None,
            validation: crate::config::Validation::default(),
            text: None,
        })));
        let state = || app.state::<Arc<AppState>>();
        fn run<F: std::future::Future>(f: F) -> F::Output {
            tauri::async_runtime::block_on(f)
        }
        let job = format!("{base}/g/p/-/jobs/7");
        let tail = |watch: &str, url: &str| run(job_log_tail(watch.into(), url.into(), state()));

        assert_eq!(tail("w", &job).unwrap_err(), "no configuration is loaded");
        state().lock().config = Some(config.clone());
        assert_eq!(tail("nope", &job).unwrap_err(), "no watch with id \"nope\"");
        assert!(tail("w", "https://elsewhere.example/g/p/-/jobs/7").is_err());
        assert_eq!(
            tail("w", &job).unwrap_err(),
            "account \"gl\" is not being polled yet; try again in a moment"
        );

        let account = config.accounts.get("gl").unwrap();
        let client = bridgewatch_core::poll::client_for_account(
            "gl",
            account,
            Some(Secret::new("not-a-real-token")),
            Arc::new(SystemTokenProvider),
            RequestRing::new(4),
        )
        .unwrap();
        state().set_clients([("gl".to_string(), client)].into_iter().collect());
        assert!(tail("w", &job).is_err(), "the instance refused the log");
        let acted = run(job_action(
            "w".into(),
            job.clone(),
            JobAction::Retry,
            state(),
        ));
        assert!(acted.is_err(), "the instance refused the retry");
    }
}
