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
use std::time::Duration;

use bridgewatch_core::config::Config;
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
) -> String {
    serde_json::json!({
        "watch": watch,
        "sha": sha,
        "state": row.map(|r| r.state.as_str()),
        "exit": code,
        "timed_out": timed_out,
        "pipeline": row,
    })
    .to_string()
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

    let mut poller = build_poller(&config, args.fixture.as_ref())?;
    let deadline = args.timeout.map(|t| tokio::time::Instant::now() + t);
    let mut last: Option<PipelineView> = None;
    let mut said = String::new();

    // ⛔ One Ctrl-C listener for the whole run, installed before the first
    // tick, as `watch` does and for the same reasons (see there).
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let interrupted_code = |last: &Option<PipelineView>| {
        if last.is_some() {
            exit::PENDING
        } else {
            exit::UNKNOWN
        }
    };
    if std::future::poll_fn(|cx| std::task::Poll::Ready(interrupt.as_mut().poll(cx)))
        .await
        .is_ready()
    {
        return Ok(interrupted_code(&last));
    }
    let expired = async {
        match deadline {
            Some(at) => tokio::time::sleep_until(at).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(expired);

    loop {
        let tick = tokio::select! {
            tick = poller.tick() => tick,
            _ = &mut interrupt => return Ok(interrupted_code(&last)),
            _ = &mut expired => break,
        };

        let row = find_row(&tick.snapshot, &watch, &sha);
        if let Some(row) = row
            && settled(row)
        {
            let code = super::code_for(row.state);
            if args.json {
                println!("{}", report(&watch, &sha, Some(row), code, false));
            } else {
                println!("{}", line(&watch, row));
            }
            return Ok(code);
        }

        // Progress goes to stderr, once per change, so stdout carries only
        // the answer.
        let now = match row {
            Some(r) => format!("waiting: {}", line(&watch, r)),
            None => {
                let error = tick
                    .snapshot
                    .watches
                    .iter()
                    .find(|w| w.id == watch)
                    .and_then(|w| w.error.clone());
                match error {
                    Some(e) => format!("waiting: {watch}: {e}"),
                    None => format!("waiting: no pipeline for {sha} on {watch} yet"),
                }
            }
        };
        if now != said {
            eprintln!("{now}");
            said = now;
        }
        last = row.cloned();

        tokio::select! {
            _ = tokio::time::sleep(tick.next_interval) => {}
            _ = &mut interrupt => return Ok(interrupted_code(&last)),
            _ = &mut expired => break,
        }
    }

    let waited = args.timeout.map(|t| t.as_secs()).unwrap_or_default();
    if args.json {
        println!(
            "{}",
            report(&watch, &sha, last.as_ref(), exit::TIMEOUT, true)
        );
    }
    eprintln!(
        "timed out after {waited}s waiting for {sha} on {watch} (last seen: {})",
        last.as_ref()
            .map(|r| r.state.as_str())
            .unwrap_or("no pipeline")
    );
    Ok(exit::TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::{normalise_sha, parse_duration};
    use std::time::Duration;

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
