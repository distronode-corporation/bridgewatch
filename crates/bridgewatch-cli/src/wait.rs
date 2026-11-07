//! `bridgewatch wait`: block until one commit's pipeline on one watch has a
//! verdict, then exit with that verdict's code.
//!
//! For scripts and coding agents: "I pushed; tell me when it is green, and
//! whether it deployed". It is `check` in a loop, narrowed to one commit: the
//! same poller, the same rules, the same exit codes. It reads and never
//! writes, so a read-only token is enough, and it keeps no notification
//! ledger.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use bridgewatch_core::client::ClientError;
use bridgewatch_core::config::Config;
use bridgewatch_core::poll::{CommitLookup, Poller};
use bridgewatch_core::verdict::{IconState, PipelineView, Snapshot};
use clap::Args;

use super::{ConfigArgs, Failure, Outcome, build_poller, exit, load, start_logging};

#[derive(Debug, Args)]
pub struct WaitArgs {
    /// The watch the push lands on. May be left out when only one watch is
    /// configured, or only one is primary.
    #[arg(long = "watch", value_name = "ID")]
    pub watch: Option<String>,
    /// The commit, full or abbreviated (at least 7 hex digits). Defaults to
    /// `git rev-parse HEAD` in the current directory.
    #[arg(long, value_name = "SHA")]
    pub sha: Option<String>,
    /// Give up after this long and exit 124: 90s, 10m, 1h30m, or a number of
    /// seconds. Without it, wait for as long as it takes.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub timeout: Option<Duration>,
    /// Give up, exiting 4, when the commit still has no pipeline after this
    /// long. A fresh push often has none for its first seconds or minutes.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration, default_value = "10m")]
    pub no_pipeline_grace: Duration,
    /// Print the final verdict as one JSON object instead of a line of text.
    #[arg(long)]
    pub json: bool,
    /// Answer from a recorded fixture directory instead of the network, as
    /// `check --fixture` does.
    #[arg(long, value_name = "DIR")]
    pub fixture: Option<PathBuf>,
}

/// Parse `90`, `90s`, `10m`, `1h30m`, `2h`. At least one second.
pub fn parse_duration(raw: &str) -> Result<Duration, String> {
    let text = raw.trim();
    let bad = || format!("{raw:?} is not a duration; write e.g. 90s, 10m or 1h30m");
    if text.is_empty() {
        return Err(bad());
    }
    if let Ok(secs) = text.parse::<u64>() {
        return (secs > 0)
            .then(|| Duration::from_secs(secs))
            .ok_or_else(bad);
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    let mut last_unit = u64::MAX;
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit = match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(bad()),
        };
        // Units in descending order, each once: "10m1h" and "5s5s" are typos.
        if digits.is_empty() || unit >= last_unit {
            return Err(bad());
        }
        last_unit = unit;
        let n: u64 = digits.parse().map_err(|_| bad())?;
        total = n
            .checked_mul(unit)
            .and_then(|v| total.checked_add(v))
            .ok_or_else(bad)?;
        digits.clear();
    }
    if !digits.is_empty() || total == 0 {
        return Err(bad());
    }
    Ok(Duration::from_secs(total))
}

/// A commit as given: lowercase hex, at least 7 digits.
fn normalise_sha(raw: &str) -> Result<String, Failure> {
    let sha = raw.trim().to_ascii_lowercase();
    if sha.len() < 7 || sha.len() > 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Failure::new(
            exit::USAGE,
            anyhow::anyhow!("--sha {raw:?} is not a commit sha (7 to 64 hex digits)"),
        ));
    }
    Ok(sha)
}

/// The current directory's `HEAD`.
fn git_head() -> Result<String, Failure> {
    let no_head = |why: String| {
        Failure::new(
            exit::USAGE,
            anyhow::anyhow!("no --sha given, and {why}; pass --sha <commit>"),
        )
    };
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| no_head(format!("git could not be run ({e})")))?;
    if !out.status.success() {
        return Err(no_head(
            "the current directory is not a git repository with a commit".into(),
        ));
    }
    normalise_sha(&String::from_utf8_lossy(&out.stdout))
}

/// The watch to wait on: the one named, else the only one, else the only
/// primary one.
fn pick_watch(config: &Config, named: Option<&str>) -> Result<String, Failure> {
    let known: Vec<&str> = config.watches.iter().map(|w| w.id.as_str()).collect();
    if let Some(id) = named {
        return if known.contains(&id) {
            Ok(id.to_string())
        } else {
            Err(Failure::new(
                exit::USAGE,
                anyhow::anyhow!(
                    "no watch with id {id:?}; configured watches are [{}]",
                    known.join(", ")
                ),
            ))
        };
    }
    if let [only] = known.as_slice() {
        return Ok((*only).to_string());
    }
    let primaries: Vec<&str> = config
        .watches
        .iter()
        .filter(|w| w.role.is_primary())
        .map(|w| w.id.as_str())
        .collect();
    if let [only] = primaries.as_slice() {
        return Ok((*only).to_string());
    }
    Err(Failure::new(
        exit::USAGE,
        anyhow::anyhow!(
            "more than one watch could carry this push; name one with --watch (configured: [{}])",
            known.join(", ")
        ),
    ))
}

/// The newest pipeline for this commit on this watch. A re-run of the same
/// commit is a new pipeline, and the newest one is the answer.
fn find_row<'a>(snapshot: &'a Snapshot, watch: &str, sha: &str) -> Option<&'a PipelineView> {
    snapshot
        .watches
        .iter()
        .filter(|w| w.id == watch)
        .flat_map(|w| &w.rows)
        .filter(|r| r.sha.to_ascii_lowercase().starts_with(sha))
        .max_by_key(|r| r.id)
}

/// Whether a row's verdict is final.
///
/// ⚠️ Not while anything is in flight, even when the state already reads
/// `failed` or `deployed`: a post-deploy failure can still downgrade a deploy,
/// and a failed pipeline can still deploy. A parked gate is final, because
/// nothing moves until a person does, and waiting on that is what `--timeout`
/// would otherwise be spent on.
fn settled(row: &PipelineView) -> bool {
    !row.live && !matches!(row.state, IconState::Running | IconState::Unknown)
}

fn report(
    watch: &str,
    sha: &str,
    row: Option<&PipelineView>,
    code: i32,
    timed_out: bool,
    reason: Option<&str>,
) -> String {
    serde_json::json!({
        "watch": watch,
        "sha": sha,
        "state": row.map(|r| r.state.as_str()),
        "exit": code,
        "timed_out": timed_out,
        // Why there is no verdict: skipped, no pipeline, a refused request.
        "reason": reason,
        "pipeline": row,
    })
    .to_string()
}

/// `1h30m`, `10m`, `90s`: a duration as `--timeout` takes one.
fn human(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 || out.is_empty() {
        out.push_str(&format!("{s}s"));
    }
    out
}

/// What one wait is for.
struct Plan {
    watch: String,
    /// The watch's account, named in a refusal.
    account: String,
    sha: String,
    timeout: Option<Duration>,
    grace: Duration,
}

impl Plan {
    fn sha7(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }
}

/// How a wait ended.
#[derive(Debug)]
enum Ended {
    /// The commit's pipeline settled.
    Verdict(Box<PipelineView>),
    /// Its pipeline was skipped: it ran nothing, so there is no verdict.
    Skipped,
    /// Still no pipeline after the grace.
    NoPipeline,
    /// A request was refused in a way no retry fixes: the token (401, 403, a
    /// sign-in that has to be redone) or the project (404).
    Refused(String),
    /// `--timeout` ran out; the last row seen, if any.
    TimedOut(Option<PipelineView>),
    /// Ctrl-C; the last row seen, if any.
    Interrupted(Option<PipelineView>),
}

impl Ended {
    /// The exit code. ⛔ A refusal is what `check` exits with for the same
    /// answer: the watch reads `unknown`, so 4. 70 is a token that does not
    /// RESOLVE, before anything is sent, and `wait` inherits that one from
    /// building the poller exactly as `check` does.
    fn code(&self) -> i32 {
        match self {
            Ended::Verdict(row) => super::code_for(row.state),
            Ended::Skipped | Ended::NoPipeline | Ended::Refused(_) => exit::UNKNOWN,
            Ended::TimedOut(_) => exit::TIMEOUT,
            Ended::Interrupted(Some(_)) => exit::PENDING,
            Ended::Interrupted(None) => exit::UNKNOWN,
        }
    }
}

/// An answer no retry will change: the token is refused or has to be signed
/// in again, or the project is not there. A 429, a 5xx or a dropped
/// connection is not one, and keeps the poller's backoff.
fn refusal(plan: &Plan, error: &ClientError, text: Option<&str>) -> Option<String> {
    let status = match error {
        ClientError::Auth { status, .. } => format!(" (HTTP {status})"),
        ClientError::NotFound { .. } => " (HTTP 404)".to_string(),
        ClientError::SignInAgain { .. } => String::new(),
        _ => return None,
    };
    let text = text.map_or_else(|| error.to_string(), str::to_string);
    Some(format!("account {}: {text}{status}", plan.account))
}

fn line(watch: &str, row: &PipelineView) -> String {
    let url = row
        .web_url
        .as_deref()
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    format!(
        "{watch} {} #{}: {} (deploy: {}){url}",
        row.sha7, row.id, row.state, row.deploy
    )
}

pub async fn run(config_args: &ConfigArgs, args: WaitArgs) -> Outcome {
    let loaded = load(config_args)?;
    start_logging(&loaded);
    let mut config = loaded.config;
    let watch = pick_watch(&config, args.watch.as_deref())?;
    let sha = match &args.sha {
        Some(raw) => normalise_sha(raw)?,
        None => git_head()?,
    };
    // Only the one watch is polled, so only its account's token is read.
    config.watches.retain(|w| w.id == watch);
    let account = config
        .watches
        .first()
        .map(|w| w.account.clone())
        .unwrap_or_default();

    let mut poller = build_poller(&config, args.fixture.as_ref())?;
    let plan = Plan {
        watch,
        account,
        sha,
        timeout: args.timeout,
        grace: args.no_pipeline_grace,
    };

    // ⛔ One Ctrl-C listener for the whole run, installed before the first
    // tick, as `watch` does and for the same reasons (see there).
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    if std::future::poll_fn(|cx| std::task::Poll::Ready(interrupt.as_mut().poll(cx)))
        .await
        .is_ready()
    {
        return Ok(Ended::Interrupted(None).code());
    }
    let ended = wait_for(&mut poller, &plan, interrupt).await;
    let code = ended.code();
    let (row, reason) = match &ended {
        Ended::Verdict(row) => {
            if args.json {
                println!(
                    "{}",
                    report(&plan.watch, &plan.sha, Some(row), code, false, None)
                );
            } else {
                println!("{}", line(&plan.watch, row));
            }
            return Ok(code);
        }
        Ended::Interrupted(_) => return Ok(code),
        Ended::Skipped => (None, format!("pipeline for {} was skipped", plan.sha7())),
        Ended::NoPipeline => (
            None,
            format!(
                "no pipeline for {} after {}",
                plan.sha7(),
                human(plan.grace)
            ),
        ),
        Ended::Refused(why) => (None, why.clone()),
        Ended::TimedOut(last) => (
            last.as_ref(),
            format!(
                "timed out after {}s waiting for {} on {} (last seen: {})",
                plan.timeout.map(|t| t.as_secs()).unwrap_or_default(),
                plan.sha,
                plan.watch,
                last.as_ref()
                    .map(|r| r.state.as_str())
                    .unwrap_or("no pipeline")
            ),
        ),
    };
    if args.json {
        let timed_out = matches!(ended, Ended::TimedOut(_));
        println!(
            "{}",
            report(&plan.watch, &plan.sha, row, code, timed_out, Some(&reason))
        );
    }
    eprintln!("{reason}");
    Ok(code)
}

/// Wait until the commit has an answer: its pipeline settles, it turns out to
/// have none, a request is refused for good, time runs out, or `interrupt`.
///
/// ⛔ A commit missing from the watch's rows is looked up DIRECTLY, never
/// inferred from what the rows hold: the rows keep only the newest settled
/// pipelines and never a skipped one, and an old commit can still get a new
/// pipeline (a re-run, a manual one). The lookup only runs on a tick whose
/// list request for this watch worked, so it never jumps a backoff.
async fn wait_for<F>(poller: &mut Poller, plan: &Plan, mut interrupt: Pin<&mut F>) -> Ended
where
    F: Future,
{
    let started = tokio::time::Instant::now();
    let expired = async {
        match plan.timeout {
            Some(t) => tokio::time::sleep_until(started + t).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(expired);
    let mut last: Option<PipelineView> = None;
    let mut said = String::new();

    loop {
        let tick = tokio::select! {
            tick = poller.tick() => tick,
            _ = &mut interrupt => return Ended::Interrupted(last),
            _ = &mut expired => return Ended::TimedOut(last),
        };
        let view = tick.snapshot.watches.iter().find(|w| w.id == plan.watch);
        if let Some(e) = tick.list_errors.get(&plan.watch)
            && let Some(why) = refusal(plan, e, view.and_then(|v| v.error.as_deref()))
        {
            return Ended::Refused(why);
        }

        let mut row = find_row(&tick.snapshot, &plan.watch, &plan.sha).cloned();
        let mut lookup_error: Option<String> = None;
        let mut none_yet = false;
        if row.is_none() && view.is_some_and(|v| v.error.is_none()) {
            let looked = tokio::select! {
                looked = poller.look_up_commit(&plan.watch, &plan.sha) => looked,
                _ = &mut interrupt => return Ended::Interrupted(last),
                _ = &mut expired => return Ended::TimedOut(last),
            };
            match looked {
                Ok(CommitLookup::Found(found)) => row = Some(*found),
                Ok(CommitLookup::Skipped(_)) => return Ended::Skipped,
                Ok(CommitLookup::NotFound) => none_yet = true,
                Err(e) => match refusal(plan, &e, None) {
                    Some(why) => return Ended::Refused(why),
                    None => lookup_error = Some(e.to_string()),
                },
            }
        }

        if let Some(r) = &row {
            // A watch whose every match was skipped still shows the newest one,
            // as `canceled`; for a wait it is the same answer as the lookup's.
            if r.status == "skipped" {
                return Ended::Skipped;
            }
            if settled(r) {
                return Ended::Verdict(Box::new(r.clone()));
            }
        } else if none_yet && last.is_none() && started.elapsed() >= plan.grace {
            return Ended::NoPipeline;
        }

        // Progress goes to stderr, once per change, so stdout carries only
        // the answer.
        let now = match (&row, view.and_then(|v| v.error.as_deref()), lookup_error) {
            (Some(r), _, _) => format!("waiting: {}", line(&plan.watch, r)),
            (None, Some(e), _) => format!("waiting: {}: {e}", plan.watch),
            (None, None, Some(e)) => format!("waiting: {}: {e}", plan.watch),
            (None, None, None) => format!(
                "waiting: no pipeline for {} on {} yet",
                plan.sha, plan.watch
            ),
        };
        if now != said {
            eprintln!("{now}");
            said = now;
        }
        if row.is_some() {
            last = row;
        }

        tokio::select! {
            _ = tokio::time::sleep(tick.next_interval) => {}
            _ = &mut interrupt => return Ended::Interrupted(last),
            _ = &mut expired => return Ended::TimedOut(last),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Ended, Plan, human, normalise_sha, parse_duration, wait_for};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use bridgewatch_core::client::script::ScriptTransport;
    use bridgewatch_core::client::{RequestRing, client_for};
    use bridgewatch_core::config::parse_str;
    use bridgewatch_core::poll::Poller;
    use bridgewatch_core::token::Secret;

    // -----------------------------------------------------------------------
    // The wait loop, against a scripted GitLab on a paused clock
    // -----------------------------------------------------------------------

    /// The commit being waited on, and two later ones on the same ref.
    const A: &str = "aaaaaaa0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbb0bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "ccccccc0cccccccccccccccccccccccccccccccc";

    /// The list request a tick sends, and the direct lookup `wait` sends.
    /// ⚠️ Neither is a substring of the other: the lookup's `sha=` sits
    /// between `ref` and `order_by`.
    const TICK: &str = "pipelines?ref=main&order_by";
    const LOOKUP: &str = "pipelines?ref=main&sha=";

    /// One watch that shows only its newest settled pipeline, so an older
    /// commit is out of its rows as soon as anything newer settles.
    fn poller(script: &Arc<ScriptTransport>) -> Poller {
        poller_with(script, "")
    }

    /// The same watch, with more of its keys (`poll = { .. }`).
    fn poller_with(script: &Arc<ScriptTransport>, extra: &str) -> Poller {
        let raw = r#"
[accounts.gitlab]
token = { env = "BRIDGEWATCH_TEST_TOKEN_NEVER_SET" }

[[watches]]
id = "main-push"
account = "gitlab"
project = 1
ref = "main"
show = { settled = 1 }
"#;
        let raw = format!("{raw}{extra}\n");
        let config = parse_str(&raw, Path::new("wait.toml"))
            .expect("config")
            .config;
        let ring = RequestRing::default();
        let client = client_for(
            &config.accounts["gitlab"],
            &Secret::new("t"),
            script.clone(),
            ring.clone(),
        )
        .expect("client");
        Poller::with_clients(
            &config,
            BTreeMap::from([("gitlab".to_string(), client)]),
            ring,
        )
        .expect("poller")
    }

    fn plan() -> Plan {
        Plan {
            watch: "main-push".into(),
            account: "gitlab".into(),
            sha: A.into(),
            // An hour, so a wait that never answers fails as `TimedOut`
            // rather than hanging the test run.
            timeout: Some(Duration::from_secs(3600)),
            grace: Duration::from_secs(600),
        }
    }

    /// One GitLab pipeline row.
    fn pipeline(id: u64, sha: &str, status: &str) -> String {
        format!(
            r#"{{"id":{id},"iid":{id},"project_id":1,"sha":"{sha}","ref":"main",
                 "status":"{status}","source":"push",
                 "web_url":"https://gitlab.com/p/-/pipelines/{id}",
                 "created_at":"2026-11-07T00:00:00Z","updated_at":"2026-11-07T00:10:00Z"}}"#
        )
    }

    fn list(rows: &[String]) -> String {
        format!("[{}]", rows.join(","))
    }

    /// The same answer, many times: a wait polls until it has its answer.
    fn always(script: &ScriptTransport, contains: &str, status: u16, body: &str) {
        for _ in 0..200 {
            script.reply(contains, status, body);
        }
    }

    /// A pipeline with no jobs and no bridges: judged from its own status.
    fn empty_detail(script: &ScriptTransport, id: u64) {
        always(script, &format!("pipelines/{id}/jobs"), 200, "[]");
        always(script, &format!("pipelines/{id}/bridges"), 200, "[]");
    }

    async fn wait(poller: &mut Poller, plan: &Plan) -> Ended {
        let never = std::future::pending::<()>();
        tokio::pin!(never);
        wait_for(poller, plan, never).await
    }

    fn requests_to(script: &ScriptTransport, contains: &str) -> usize {
        script
            .requests()
            .iter()
            .filter(|r| r.url.contains(contains))
            .count()
    }

    /// ⛔ A skipped pipeline is never one of a watch's rows. Looked up
    /// directly, it is an answer at once: there will be no verdict.
    #[tokio::test(start_paused = true)]
    async fn a_skipped_pipeline_is_an_answer_not_a_wait() {
        let script = Arc::new(ScriptTransport::new());
        always(&script, LOOKUP, 200, &list(&[pipeline(10, A, "skipped")]));
        always(&script, TICK, 200, &list(&[pipeline(20, B, "success")]));
        empty_detail(&script, 20);
        let mut poller = poller(&script);

        let ended = wait(&mut poller, &plan()).await;
        assert!(matches!(ended, Ended::Skipped), "{ended:?}");
        assert_eq!(ended.code(), crate::exit::UNKNOWN);
    }

    /// ⛔ A commit whose pipeline has dropped out of the rows is judged from
    /// that pipeline, with the code `check` gives its state. Never from its age.
    #[tokio::test(start_paused = true)]
    async fn a_commit_out_of_the_rows_is_judged_from_its_own_pipeline() {
        let script = Arc::new(ScriptTransport::new());
        always(&script, LOOKUP, 200, &list(&[pipeline(10, A, "failed")]));
        always(
            &script,
            TICK,
            200,
            &list(&[
                pipeline(30, C, "success"),
                pipeline(20, B, "success"),
                pipeline(10, A, "failed"),
            ]),
        );
        for id in [10, 20, 30] {
            empty_detail(&script, id);
        }
        let mut poller = poller(&script);

        let ended = wait(&mut poller, &plan()).await;
        let Ended::Verdict(row) = &ended else {
            panic!("{ended:?}");
        };
        assert_eq!(row.id, 10, "the commit's own pipeline, not the newest");
        assert_eq!(row.status, "failed");
        assert_eq!(ended.code(), crate::code_for(row.state));
        assert_eq!(ended.code(), crate::exit::FAILED);
    }

    /// An abbreviated sha is expanded through the provider before the lookup,
    /// because GitLab's `sha` (and GitHub's `head_sha`) match a full sha only.
    #[tokio::test(start_paused = true)]
    async fn an_abbreviated_sha_is_expanded_before_it_is_looked_up() {
        let script = Arc::new(ScriptTransport::new());
        always(
            &script,
            "repository/commits/aaaaaaa0",
            200,
            &format!(r#"{{"id":"{A}"}}"#),
        );
        always(
            &script,
            &format!("sha={A}"),
            200,
            &list(&[pipeline(10, A, "failed")]),
        );
        always(
            &script,
            TICK,
            200,
            &list(&[pipeline(20, B, "success"), pipeline(10, A, "failed")]),
        );
        for id in [10, 20] {
            empty_detail(&script, id);
        }
        let mut poller = poller(&script);

        let plan = Plan {
            sha: "aaaaaaa0".into(),
            ..plan()
        };
        let ended = wait(&mut poller, &plan).await;
        let Ended::Verdict(row) = &ended else {
            panic!("{ended:?}");
        };
        assert_eq!(row.id, 10);
    }

    /// A fresh push has no pipeline for a while. Waited for, not given up on.
    #[tokio::test(start_paused = true)]
    async fn a_pipeline_not_created_yet_is_waited_for() {
        let script = Arc::new(ScriptTransport::new());
        always(&script, LOOKUP, 200, "[]");
        script.reply(TICK, 200, "[]");
        script.reply(TICK, 200, "[]");
        always(&script, TICK, 200, &list(&[pipeline(10, A, "success")]));
        empty_detail(&script, 10);
        let mut poller = poller(&script);

        let ended = wait(&mut poller, &plan()).await;
        let Ended::Verdict(row) = &ended else {
            panic!("{ended:?}");
        };
        assert_eq!(row.id, 10);
        assert_eq!(ended.code(), crate::exit::OK);
        assert_eq!(
            requests_to(&script, LOOKUP),
            2,
            "looked up while it had none"
        );
    }

    /// ⛔ ...but not forever: still none after the grace is an answer, `unknown`.
    #[tokio::test(start_paused = true)]
    async fn a_pipeline_never_created_ends_after_the_grace() {
        let script = Arc::new(ScriptTransport::new());
        always(&script, LOOKUP, 200, "[]");
        always(&script, TICK, 200, "[]");
        let mut poller = poller(&script);

        let started = tokio::time::Instant::now();
        let ended = wait(&mut poller, &plan()).await;
        assert!(matches!(ended, Ended::NoPipeline), "{ended:?}");
        assert_eq!(ended.code(), crate::exit::UNKNOWN);
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(600),
            "not before the grace: {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(700),
            "but soon after it: {waited:?}"
        );
    }

    /// ⛔ A refused token or a missing project is an answer at once, with the
    /// code `check` exits with for the same tick, and one request sent.
    #[tokio::test(start_paused = true)]
    async fn a_401_or_404_ends_at_once_with_checks_code() {
        for (status, body) in [
            (401, r#"{"message":"401 Unauthorized"}"#),
            (404, r#"{"message":"404 Project Not Found"}"#),
        ] {
            let script = Arc::new(ScriptTransport::new());
            always(&script, TICK, status, body);
            let mut check = poller(&script);
            let check_code = crate::exit_code(&check.tick().await.snapshot);

            let script = Arc::new(ScriptTransport::new());
            always(&script, TICK, status, body);
            let mut poller = poller(&script);
            let ended = wait(&mut poller, &plan()).await;
            let Ended::Refused(why) = &ended else {
                panic!("{status}: {ended:?}");
            };
            assert!(why.starts_with("account gitlab: "), "{why}");
            assert!(why.ends_with(&format!("(HTTP {status})")), "{why}");
            assert_eq!(ended.code(), check_code, "{status}");
            assert_eq!(ended.code(), crate::exit::UNKNOWN, "{status}");
            assert_eq!(script.requests().len(), 1, "{status}: no retry");
        }
    }

    /// A 503 is weather: retried on the poller's backoff, then answered.
    ///
    /// ⚠️ On the real clock, with a one-second interval: the backoff gate
    /// reads the wall clock (it must survive a laptop's sleep), so a paused
    /// tokio clock would spin through deferred ticks until real time caught up.
    #[tokio::test]
    async fn a_503_is_retried() {
        let script = Arc::new(ScriptTransport::new());
        script.reply(TICK, 503, "");
        always(&script, TICK, 200, &list(&[pipeline(10, A, "success")]));
        empty_detail(&script, 10);
        let mut poller = poller_with(&script, "poll = { live_secs = 1, idle_secs = 1 }");

        let ended = wait(&mut poller, &plan()).await;
        assert!(matches!(ended, Ended::Verdict(_)), "{ended:?}");
        assert_eq!(requests_to(&script, TICK), 2);
    }

    #[test]
    fn a_grace_reads_as_it_was_written() {
        assert_eq!(human(Duration::from_secs(600)), "10m");
        assert_eq!(human(Duration::from_secs(5400)), "1h30m");
        assert_eq!(human(Duration::from_secs(90)), "1m30s");
        assert_eq!(human(Duration::from_secs(5)), "5s");
    }

    #[test]
    fn durations_parse_as_a_person_writes_them() {
        for (raw, secs) in [
            ("90", 90),
            ("90s", 90),
            ("10m", 600),
            ("1h30m", 5400),
            ("2h", 7200),
            ("1h0m5s", 3605),
        ] {
            assert_eq!(parse_duration(raw), Ok(Duration::from_secs(secs)), "{raw}");
        }
        for raw in [
            "",
            "0",
            "0s",
            "m",
            "10x",
            "5s5s",
            "10m1h",
            "1.5h",
            "-1",
            "1h 30m",
            "99999999999999999999h",
        ] {
            assert!(parse_duration(raw).is_err(), "{raw:?} should be refused");
        }
    }

    #[test]
    fn a_sha_is_seven_to_sixty_four_hex_digits() {
        assert_eq!(normalise_sha(" 4CFACED\n").unwrap(), "4cfaced");
        for raw in ["4cface", "main", "4cfaced9z", ""] {
            assert!(normalise_sha(raw).is_err(), "{raw:?}");
        }
    }
}
