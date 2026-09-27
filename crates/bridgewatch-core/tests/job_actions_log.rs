//! The audit line a write leaves, and the promise that neither it nor a log
//! tail's requests carry a credential or a signed URL's signature.
//!
//! ⛔ A file, and so a process, of its own, for the reason
//! `tests/client_log.rs` gives: every test here captures through
//! `support::with_log` from its first line.

mod support;

use std::sync::Arc;

use bridgewatch_core::actions::{self, JobAction, JobTarget};
use bridgewatch_core::client::{HttpResponse, RequestRing, ScriptTransport, client_for};
use bridgewatch_core::config::{Account, ProjectRef, Provider};
use bridgewatch_core::token::Secret;

fn target(provider: Provider) -> JobTarget {
    JobTarget {
        account: "work".into(),
        provider,
        project: ProjectRef::Path("acme-corp/monorepo".into()),
        job_id: 7,
    }
}

fn client(
    provider: Provider,
    actions: bool,
    t: Arc<ScriptTransport>,
) -> Arc<dyn bridgewatch_core::client::CiClient> {
    client_for(
        &Account {
            actions,
            ..Account::for_provider(provider)
        },
        &Secret::new("TOKEN-SECRET-0001"),
        t,
        RequestRing::new(8),
    )
    .unwrap()
}

#[test]
fn every_write_is_logged_at_info_with_what_was_done_and_never_the_token() {
    let runtime = support::runtime();
    let t = Arc::new(ScriptTransport::new());
    t.reply("/jobs/7/retry", 201, r#"{"id":8}"#);
    let on = client(Provider::Gitlab, true, t);
    let off = client(Provider::Gitlab, false, Arc::new(ScriptTransport::new()));

    let (results, log) = support::with_log(|| {
        runtime.block_on(async {
            (
                actions::perform(on.as_ref(), &target(Provider::Gitlab), JobAction::Retry).await,
                actions::perform(off.as_ref(), &target(Provider::Gitlab), JobAction::Play).await,
            )
        })
    });
    assert!(results.0.is_ok() && results.1.is_err());

    let done = log
        .lines()
        .find(|l| l.contains("job action") && l.contains("outcome=\"done\""))
        .unwrap_or_else(|| panic!("no audit line for the retry:\n{log}"));
    assert_eq!(
        support::level_of(done, "job action").as_deref(),
        Some("INFO")
    );
    for field in [
        "account=work",
        "project=acme-corp/monorepo",
        "job=7",
        "action=\"retry\"",
    ] {
        assert!(done.contains(field), "{field} missing from {done}");
    }
    let refused = log
        .lines()
        .find(|l| l.contains("job action") && l.contains("outcome=\"refused\""))
        .unwrap_or_else(|| panic!("a refused write was not logged:\n{log}"));
    assert!(refused.contains("action=\"play\""), "{refused}");
    assert!(!log.contains("TOKEN-SECRET"), "{log}");
}

#[test]
fn a_log_tails_request_lines_carry_neither_the_token_nor_the_signature() {
    let runtime = support::runtime();
    let t = Arc::new(ScriptTransport::new());
    t.reply_with(
        "/actions/jobs/7/logs",
        HttpResponse {
            location: Some("https://results.blob.example/log.txt?sig=SIGNATURE-SECRET".into()),
            ..HttpResponse::plain(302, "")
        },
    );
    t.reply("results.blob.example", 200, "line\n");
    let gh = client(Provider::Github, false, t);
    let (result, log) = support::with_log(|| {
        runtime.block_on(actions::log_tail(gh.as_ref(), &target(Provider::Github)))
    });
    assert_eq!(result.unwrap().lines, vec!["line"]);
    assert!(
        log.contains("results.blob.example/log.txt"),
        "the second leg was not logged:\n{log}"
    );
    assert!(!log.contains("SIGNATURE-SECRET"), "{log}");
    assert!(!log.contains("TOKEN-SECRET"), "{log}");
}
