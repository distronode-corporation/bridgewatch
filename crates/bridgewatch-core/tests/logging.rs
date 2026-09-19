//! What each log level is FOR, held to it.
//!
//! `[log].level = "info"` is what somebody debugging "why is my tray wrong"
//! turns on, so it has to carry the two things they need at human frequency:
//! which configuration answered, and every watch whose verdict moved. `debug`
//! is the per-tick and per-request detail underneath it. The whole application
//! used to have exactly one `info` call, which made the default level
//! indistinguishable from silence.
//!
//! ⛔ EVERY TEST IN THIS FILE CAPTURES LOGS, and that is a rule rather than an
//! accident. The binary installs one shared subscriber the first time
//! `support::with_log` or `support::start_capture` is called, and a `tracing`
//! callsite reached before that is cached as DISABLED for the life of the
//! process, so a test added here that drives a poller outside a capture would
//! silently empty another test's. The mechanism is written out against
//! `tracing-core`'s source in `tests/support/mod.rs`. `describe_load` has no
//! callsite of its own and is tested in `tests/config.rs` with the rest of the
//! configuration surface.
//!
//! ⚠ The level a line was logged at is asserted ON the line
//! (`support::level_of`) rather than by installing one subscriber per level.
//! It is the same guarantee, stated where the decision is actually made.

mod support;

use std::sync::Arc;

use bridgewatch_core::client::http::{HttpRequest, HttpResponse, Transport};
use bridgewatch_core::client::{ClientError, GitHubClient, ListQuery, RequestRing};
use bridgewatch_core::config::edit::Edit;
use bridgewatch_core::config::{Account, ProjectRef, Provider};
use bridgewatch_core::poll::Poller;
use bridgewatch_core::token::Secret;
use support::{level_of, runtime, with_log};

fn poller_for(fixture: &str) -> Poller {
    // Building a poller logs nothing today, but "nothing logs here yet" is
    // exactly the kind of property that decays: the subscriber has to exist
    // before any code under test can reach a callsite.
    support::start_capture();
    let config = support::config_with(&[] as &[Edit]);
    let dir = support::fixtures_dir().join(fixture);
    support::fixture_poller(&config, &dir).0
}

/// A watch whose verdict moved is the one thing that happens at a rate a person
/// can read, so it is the one thing `info` says about a running poll loop. The
/// first tick has no previous verdict and says so rather than inventing
/// `unknown` as one.
#[test]
fn a_verdict_transition_is_logged_at_info_and_only_when_it_moves() {
    let runtime = runtime();
    let mut poller = poller_for("mixed-primary-green-secondary-red");

    let (_, first) = with_log(|| runtime.block_on(poller.tick()));
    assert!(
        first.contains("watch verdict changed"),
        "the first tick says what it found: {first}"
    );
    assert_eq!(
        level_of(&first, "watch verdict changed"),
        Some("INFO".to_string()),
        "a transition is the line a default-ish run is FOR: {first}"
    );
    assert!(
        first.contains("watch=\"main-push\"") && first.contains("to=\"deployed\""),
        "{first}"
    );
    // ⚠ The secondary watch is logged too. Its verdict never reaches the tray,
    // but it is on screen in the popover, and "the hourly schedule went red" is
    // exactly the line somebody is looking for.
    assert!(
        first.contains("watch=\"hourly\"") && first.contains("to=\"failed\""),
        "{first}"
    );
    assert!(first.contains("from=\"-\""), "no previous verdict: {first}");

    let (_, second) = with_log(|| runtime.block_on(poller.tick()));
    assert!(
        !second.is_empty(),
        "the second tick logged nothing at all, so the assertion below would \
         hold for the wrong reason: {second}"
    );
    assert!(
        !second.contains("watch verdict changed"),
        "nothing moved, so nothing is said: {second}"
    );
}

/// The per-tick line is `debug`: at the live interval it is one line every few
/// seconds, which is not a level anybody leaves on.
#[test]
fn the_per_tick_line_is_debug_and_not_info() {
    let runtime = runtime();
    let mut poller = poller_for("4cfaced9-deployed");

    let (_, log) = with_log(|| runtime.block_on(poller.tick()));
    assert!(log.contains("tick complete"), "{log}");
    assert_eq!(
        level_of(&log, "tick complete"),
        Some("DEBUG".to_string()),
        "one line every few seconds is not something `info` prints: {log}"
    );
    assert!(log.contains("icon=deployed"), "{log}");
    // The request detail is the other half of `debug`, and it is what says
    // whether a tick asked for anything at all.
    assert!(log.contains("bridgewatch_core::client"), "{log}");
    assert_eq!(
        level_of(&log, "bridgewatch_core::client"),
        Some("DEBUG".to_string()),
        "and it is debug too: {log}"
    );
}

/// A transport that answers one empty page and records nothing.
#[derive(Debug)]
struct OneEmptyPage;

#[async_trait::async_trait]
impl Transport for OneEmptyPage {
    async fn execute(&self, _request: HttpRequest) -> Result<HttpResponse, ClientError> {
        Ok(HttpResponse {
            status: 200,
            body: r#"{"total_count":0,"workflow_runs":[]}"#.to_string(),
            next_page: None,
            ratelimit_remaining: Some(4_999),
            ratelimit_reset: None,
            retry_after: None,
            etag: None,
            link: None,
            oauth_scopes: None,
        })
    }
}

/// ⛔ The debug LOG is where a credential escapes if it escapes at all, because
/// unlike the request ring it is free text that people paste into issues. The
/// GitHub client's per-request line carries the method, the path, the status,
/// the size and the time, and nothing whatever about the `Authorization` header
/// it was sent with, not even the word.
#[test]
fn the_github_request_line_never_carries_the_token() {
    // First, so that nothing below can reach a callsite before the shared
    // subscriber exists (the rule at the top of this file).
    support::start_capture();
    let runtime = runtime();
    let client = GitHubClient::new(
        &Account::for_provider(Provider::Github),
        &Secret::new("ghp_SECRET"),
        Arc::new(OneEmptyPage),
        RequestRing::new(4),
    );

    let (result, log) = with_log(|| {
        runtime.block_on(client.list_pipelines(
            &ProjectRef::Path("acme-corp/monorepo".into()),
            &ListQuery::exact("main", None, 5),
        ))
    });
    result.expect("the scripted empty page");

    assert!(
        !log.is_empty(),
        "nothing was captured, so the absences that follow would pass on an empty log"
    );
    assert!(!log.contains("ghp_SECRET"), "{log}");
    assert!(
        !log.to_ascii_lowercase().contains("bearer"),
        "not even the scheme, which is half the header: {log}"
    );
    assert!(
        log.contains("path=\"/repos/acme-corp/monorepo/actions/runs"),
        "the path is what makes the line worth printing: {log}"
    );
    assert!(log.contains("status=200"), "{log}");
}
