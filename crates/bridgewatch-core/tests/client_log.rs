//! The promise that a token never reaches a log line.
//!
//! ⛔ A FILE, AND A PROCESS, OF ITS OWN. An integration test file is one test
//! binary, so nothing here can reach the client's `debug!` callsite before the
//! capturing subscriber is installed. That ordering is the whole guarantee:
//! `tracing` caches a per-callsite `Interest` the first time the callsite is
//! hit, and a callsite first hit with no subscriber caches `Interest::never`
//! and stays there. The ~20 tests in `tests/client.rs` drive this same code
//! path with no subscriber, so with both halves in one binary the capture came
//! back EMPTY on Linux on 12 of 12 runs, and every `assert!(!log.contains(..))`
//! below passed against the empty string. The mechanism is written out in full
//! against `tracing-core`'s source in `tests/support/mod.rs`.
//!
//! ⚠ Anything added here must capture through `support::with_log` too, or it
//! reintroduces exactly the race this split exists to remove.

mod support;

use bridgewatch_core::client::ListQuery;
use bridgewatch_core::config::{AuthHeader, ProjectRef};
use support::{Canned, client_with};

/// ⛔ The debug LOG is the other place a credential could escape, and unlike
/// the request ring it is a stream of free text that people paste into issues.
/// `[log].level = "debug"` prints one line per request: the method, the path,
/// the status, the size and the time, and nothing whatever about the header it
/// was sent with. Asserted for both credential spellings, because only one of
/// them has the word "token" in its name.
#[test]
fn the_request_debug_line_never_carries_the_token() {
    let runtime = support::runtime();
    for header in [AuthHeader::PrivateToken, AuthHeader::AuthorizationBearer] {
        let transport = Canned::new(vec![(200, "[]".into(), None)]);
        let (client, _) = client_with(transport, header);
        let (result, log) = support::with_log(|| {
            runtime.block_on(client.list_pipelines(&ProjectRef::Id(1), &ListQuery::scan(10)))
        });
        result.expect("the canned response is a success");

        // ⛔ First, and before anything about what the line does NOT say: three
        // of the four assertions below are absences, and an empty capture
        // satisfies all three. That is not a hypothetical reading of this test,
        // it is how it failed.
        assert!(
            !log.is_empty(),
            "nothing was captured for {header:?}, so the assertions that follow \
             would be about the empty string rather than about a request"
        );
        assert_eq!(
            support::level_of(&log, "request"),
            Some("DEBUG".to_string()),
            "the request line is debug, not something a default run prints: {log}"
        );

        assert!(!log.contains("glpat-SECRET"), "{log}");
        assert!(!log.to_ascii_lowercase().contains("bearer"), "{log}");
        assert!(
            log.contains("path=\"/projects/1/pipelines"),
            "the path is what makes the line worth printing: {log}"
        );
        assert!(
            log.contains("status=200") && log.contains("bytes=2"),
            "{log}"
        );
    }
}
