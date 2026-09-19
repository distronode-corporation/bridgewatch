//! What each log level is FOR, held to it.
//!
//! `[log].level = "info"` is what somebody debugging "why is my tray wrong"
//! turns on, so it has to carry the two things they need at human frequency:
//! which configuration answered, and every watch whose verdict moved. `debug`
//! is the per-tick and per-request detail underneath it. The whole application
//! used to have exactly one `info` call, which made the default level
//! indistinguishable from silence.

mod support;

use bridgewatch_core::config::edit::Edit;
use bridgewatch_core::poll::Poller;
use support::{runtime, with_log};

fn poller_for(fixture: &str) -> Poller {
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

    let (_, first) = with_log(tracing::Level::INFO, || runtime.block_on(poller.tick()));
    assert!(
        first.contains("watch verdict changed"),
        "the first tick says what it found: {first}"
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

    let (_, second) = with_log(tracing::Level::INFO, || runtime.block_on(poller.tick()));
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

    let (_, info) = with_log(tracing::Level::INFO, || runtime.block_on(poller.tick()));
    assert!(!info.contains("tick complete"), "{info}");

    let (_, debug) = with_log(tracing::Level::DEBUG, || runtime.block_on(poller.tick()));
    assert!(debug.contains("tick complete"), "{debug}");
    assert!(debug.contains("icon=deployed"), "{debug}");
    // The request detail is the other half of `debug`, and it is what says
    // whether a tick asked for anything at all.
    assert!(debug.contains("bridgewatch_core::client"), "{debug}");
}

/// `describe_load` is the line both front ends print when they have read a
/// configuration, and the log level is part of it: `[log].level` is read from
/// the file being reported, but `RUST_LOG` beats it, and nothing else would say
/// so.
#[test]
fn the_config_line_names_the_file_the_counts_and_the_level() {
    let config = support::config_with(&[] as &[Edit]);
    let line = bridgewatch_core::config::describe_load(
        "using /tmp/bw/config.toml",
        Some(&config),
        "warn,bridgewatch_core=info",
    );
    assert!(line.contains("/tmp/bw/config.toml"), "{line}");
    assert!(line.contains("3 watch(es)"), "{line}");
    assert!(line.contains("1 account(s)"), "{line}");
    assert!(line.contains("warn,bridgewatch_core=info"), "{line}");

    let broken = bridgewatch_core::config::describe_load("reloaded /tmp/bw/config.toml", None, "");
    assert!(broken.contains("does not load"), "{broken}");
    assert!(broken.contains("nothing is watched"), "{broken}");
}
