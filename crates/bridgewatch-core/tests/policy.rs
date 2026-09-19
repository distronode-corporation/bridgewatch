//! The poll schedule and its backoff.

use std::time::Duration;

use bridgewatch_core::client::ClientError;
use bridgewatch_core::config::{PollConfig, RateLimitBackoff};
use bridgewatch_core::poll::{PollNow, PollPolicy};

fn policy() -> PollPolicy {
    PollPolicy::new(
        &PollConfig {
            live_secs: 20,
            idle_secs: 60,
        },
        &RateLimitBackoff { max_secs: 300 },
    )
}

/// Twenty seconds while something is in flight, sixty when it is not.
#[test]
fn the_interval_follows_whether_anything_is_live() {
    let p = policy();
    assert_eq!(p.interval(true), Duration::from_secs(20));
    assert_eq!(p.interval(false), Duration::from_secs(60));
}

/// Errors double the interval, capped at the account's ceiling.
#[test]
fn errors_back_off_and_success_clears_it() {
    let mut p = policy();
    p.on_error(&ClientError::Server { status: 503 });
    assert_eq!(p.interval(true), Duration::from_secs(40));
    assert_eq!(p.consecutive_errors(), 1);

    p.on_error(&ClientError::Transport("connection reset".into()));
    assert_eq!(p.interval(true), Duration::from_secs(80));

    for _ in 0..10 {
        p.on_error(&ClientError::Server { status: 500 });
    }
    assert_eq!(
        p.interval(true),
        Duration::from_secs(300),
        "capped at max_secs"
    );
    assert_eq!(p.interval(false), Duration::from_secs(300));

    p.on_success();
    assert_eq!(p.consecutive_errors(), 0);
    assert_eq!(
        p.interval(true),
        Duration::from_secs(20),
        "one good tick clears it"
    );
}

/// `Retry-After` wins over the doubling, because the server knows when it will
/// forgive us and we do not.
#[test]
fn retry_after_is_honoured() {
    let mut p = policy();
    p.on_error(&ClientError::RateLimited {
        retry_after: Some(120),
        reset: None,
    });
    assert_eq!(
        p.interval(true),
        Duration::from_secs(120),
        "120 beats the 40 the doubling would have chosen"
    );

    let mut p = policy();
    p.on_error(&ClientError::RateLimited {
        retry_after: Some(900),
        reset: None,
    });
    assert_eq!(
        p.interval(true),
        Duration::from_secs(900),
        "and a Retry-After above the ceiling is still honoured; ignoring it is how a \
         client gets banned rather than throttled"
    );
}

/// A 429 with no `Retry-After` still backs off.
#[test]
fn a_rate_limit_without_a_header_still_backs_off() {
    let mut p = policy();
    p.on_error(&ClientError::RateLimited {
        retry_after: None,
        reset: None,
    });
    assert_eq!(p.interval(false), Duration::from_secs(120));
}

/// Opening the popover repeatedly must not become a request storm.
#[test]
fn the_poll_now_hint_is_rate_gated_except_for_wake() {
    let mut p = policy();
    assert!(
        p.allows_poll_now(PollNow::PopoverOpened),
        "the first time is always allowed"
    );

    p.mark_polled();
    assert!(
        !p.allows_poll_now(PollNow::PopoverOpened),
        "a second open inside the window is dropped"
    );
    assert!(
        p.allows_poll_now(PollNow::Wake),
        "waking from sleep is not rate gated"
    );
    assert!(p.allows_poll_now(PollNow::ConfigChanged));
    assert!(p.allows_poll_now(PollNow::Manual));
    assert!(!PollNow::PopoverOpened.bypasses_gate());
}

/// A `Retry-After` big enough to overflow a clock is a broken header, not an
/// instruction.
///
/// ⛔ The value is attacker- and accident-controlled and it ends up added to an
/// [`std::time::Instant`], where overflow **panics**. A panic in the poll task
/// is permanent and silent: the tray keeps its last icon and nothing works again
/// until the process is restarted. So the header is clamped, and what the clamp
/// has to guarantee is exactly this — that the interval can be added to a clock.
#[test]
fn an_absurd_retry_after_is_clamped_rather_than_believed() {
    let mut p = policy();
    p.on_error(&ClientError::RateLimited {
        retry_after: Some(u64::MAX),
        reset: None,
    });

    let interval = p.interval(false);
    assert!(
        interval <= bridgewatch_core::poll::MAX_RETRY_AFTER,
        "clamped to a day, got {interval:?}"
    );
    assert!(
        std::time::Instant::now().checked_add(interval).is_some(),
        "and the poll task can still schedule it without overflowing a clock"
    );
}

/// Every class of error backs off, not only the ones `should_back_off` names.
///
/// A 404 (a project renamed, a token that lost sight of it), a body that did not
/// decode and a status nothing expected will not fix themselves inside a minute
/// either, and asking at full pace is how one misconfiguration becomes a rate
/// limit on the whole account.
#[test]
fn a_404_a_decode_failure_and_a_surprise_status_all_back_off() {
    for error in [
        ClientError::NotFound {
            path: "/projects/1/pipelines".into(),
        },
        ClientError::Decode {
            path: "/projects/1/pipelines".into(),
            message: "expected a list".into(),
        },
        ClientError::Unexpected {
            status: 418,
            path: "/projects/1/pipelines".into(),
        },
        ClientError::Auth { status: 401 },
    ] {
        let mut p = policy();
        assert!(
            !error.should_back_off(),
            "{error} is not in the back-off class, which is the point"
        );
        p.on_error(&error);
        assert_eq!(
            p.interval(true),
            Duration::from_secs(40),
            "and it backs off anyway: {error}"
        );
    }
}

/// A watch that is backing off sits the tick out.
///
/// ⛔ Backoff is per watch; the sleep between ticks is the `min` over all of
/// them. Without this gate a watch that has just been told 429 is asked again on
/// the healthiest watch's cadence, and `Retry-After` is honoured in name only.
#[test]
fn a_backing_off_watch_defers_until_its_own_interval_has_passed() {
    let mut p = policy();
    p.mark_polled_ago(Duration::from_secs(1), Duration::from_secs(1));
    assert!(
        !p.should_defer(false),
        "a watch with no errors never defers: every request-count claim rests on it"
    );

    p.on_error(&ClientError::RateLimited {
        retry_after: Some(120),
        reset: None,
    });
    p.mark_polled_ago(Duration::from_secs(30), Duration::from_secs(30));
    assert!(
        p.should_defer(false),
        "30s into a 120s Retry-After, this watch has nothing to gain by asking"
    );

    p.mark_polled_ago(Duration::from_secs(121), Duration::from_secs(121));
    assert!(!p.should_defer(false), "and it comes back on its own");
}

/// The gate on an out-of-band poll uses a clock that does not stop when the
/// machine does.
///
/// ⚠️ [`std::time::Instant`] excludes suspend on both macOS (`CLOCK_UPTIME_RAW`)
/// and Linux (`CLOCK_MONOTONIC`), so a laptop that slept for an hour reports an
/// elapsed time of seconds — and the popover a user opens on resume, which is
/// exactly when the view is most stale, would be refused as too soon.
#[test]
fn the_poll_gate_counts_time_the_machine_spent_asleep() {
    let mut p = policy();
    p.mark_polled_ago(Duration::from_millis(1), Duration::from_secs(3600));
    assert!(
        p.allows_poll_now(PollNow::PopoverOpened),
        "the monotonic clock slept through it; the wall clock did not"
    );

    // And the other way round: a wall clock that jumped backwards under NTP
    // must not take away a gap the monotonic clock really did measure.
    let mut p = policy();
    p.mark_polled_ago(Duration::from_secs(3600), Duration::from_millis(1));
    assert!(p.allows_poll_now(PollNow::PopoverOpened));
}

/// Opening the popover polls at once unless the last poll is under two seconds
/// old. Five seconds made a popover opened just after a tick show a view up to
/// a whole live interval stale, which at `live_secs = 5` is most of it.
#[test]
fn the_popover_gate_is_two_seconds() {
    use bridgewatch_core::poll::POLL_NOW_MIN_GAP;
    assert_eq!(POLL_NOW_MIN_GAP, Duration::from_secs(2));

    let mut p = policy();
    p.mark_polled_ago(Duration::from_millis(2100), Duration::from_millis(2100));
    assert!(p.allows_poll_now(PollNow::PopoverOpened), "2.1 s is enough");

    p.mark_polled_ago(Duration::from_millis(1500), Duration::from_millis(1500));
    assert!(
        !p.allows_poll_now(PollNow::PopoverOpened),
        "1.5 s is not: a flurry of opens stays one request"
    );
}

/// The default is five seconds while anything is live and sixty when nothing
/// is.
#[test]
fn the_default_schedule_is_five_live_and_sixty_idle() {
    let p = PollPolicy::new(&PollConfig::default(), &RateLimitBackoff::default());
    assert_eq!(p.interval(true), Duration::from_secs(5));
    assert_eq!(p.interval(false), Duration::from_secs(60));
}
