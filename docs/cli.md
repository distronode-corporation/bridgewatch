# CLI

The CLI runs the same core as the GUI, so anything the icon knows it can print: in a
shell prompt, a status bar or a script. Install it with npm or from the release
tarballs (see [Install](install.md#install)), or build it with Rust 1.90 or newer, from a clone of
this repository:

```
cargo install --locked --path crates/bridgewatch-cli
```

The CLI does not depend on Tauri, so it needs none of the GUI build packages.

```
bridgewatch check                        # resolve every watch once, print a summary
bridgewatch check --watch main-push      # only this watch (repeatable)
bridgewatch check --json                 # the whole snapshot as JSON
bridgewatch check --fixture <dir>        # answer from a recorded fixture, no network, no token

bridgewatch watch                        # poll until Ctrl-C, one line per change
bridgewatch watch --json                 # one JSON object per line: a snapshot per change,
                                         # {"notify": {...}} per notification
bridgewatch watch --ticks 3              # stop after three ticks

bridgewatch wait                         # block until HEAD's pipeline has a verdict, exit with it
bridgewatch wait --sha 4cfaced --watch main-push --timeout 20m --json
bridgewatch wait --no-pipeline-grace 15m # give up (exit 4) if no pipeline appears by then
bridgewatch wait --fixture <dir>         # answer from a recorded fixture, as check does

bridgewatch auth login --account <name>  # sign in (the OAuth device flow)
bridgewatch auth status [--account <name>]
                                         # who each account is signed in as; own tokens' servers
bridgewatch auth logout --account <name> # forget the sign-in

bridgewatch init --project group/project --glab --deploy-marker deploy:production
                                         # print the config the wizard would write
bridgewatch init --provider github --project octo-org/octo-repo --gh --workflow ci.yml
                                         # the same for a GitHub repository
bridgewatch config path                  # the config file in effect
bridgewatch config validate              # every problem, with line and column
bridgewatch config schema                # JSON Schema for editor completion
bridgewatch config dump                  # the config with every default filled in

bridgewatch fixture record <pipeline-id> [--out <dir>] [--account <name>] [--project <id|path>] [--list]
bridgewatch fixture scrub <dir>... [--check]

bridgewatch log <job-url> [--account <name>] [--json]
                                         # the last 40 lines of a job's log
bridgewatch retry <job-url> [--account <name>] [--yes | -y]
bridgewatch play <job-url> [--account <name>] [--yes | -y]
                                         # retry a job, or start a manual one (GitLab)
```

`--config <path>` (or `-c`) is accepted before or after any subcommand. `watch` does not
write the GUI's notification ledger; it keeps its own.

**`wait`** is for scripts and coding agents: push, then `bridgewatch wait`, and the exit
code says whether it went green and whether it deployed, with the codes `check` uses
(see [Exit codes](#exit-codes)). It waits for one commit, `--sha` (7 or more hex digits)
or else `git rev-parse HEAD` in the current directory, on one watch, `--watch <id>`, which
may be left out when only one watch is configured or only one is primary. It polls with
the same core, rules and intervals as `check` and `watch` and stops at the newest pipeline
for that commit once nothing in it is in flight: a row that already reads `failed` while
other jobs run is not final yet, because a deploy can still follow. A pipeline parked at
a manual gate is final (exit 3), since nothing moves until somebody presses it. A commit
that is not among the watch's rows (they keep only the newest settled pipelines, and
never a skipped one) is looked up directly by its sha. If its pipeline was skipped, `wait`
exits 4 with "pipeline for <sha7> was skipped". If its pipeline is older than the rows, it
is judged like any row and exits with that verdict's code. A commit with no pipeline yet is
waited for, since a fresh push often has none for a minute or two. If there is still none
after `--no-pipeline-grace` (10 minutes by default), it exits 4 with "no pipeline for <sha7>
after 10m". A request that no retry will fix ends the wait at once with exit 4, the code
`check` gives the same answer: a 401 or 403 (a token that is refused or lacks the scope),
a 404 (no such project, or one the token cannot see), or a sign-in that has to be redone.
The message names the account and the status. A 429, a 5xx or a dropped connection is
retried on the usual backoff. `--timeout <dur>` (`90s`, `10m`, `1h30m`, or seconds) gives
up with exit 124, the code `timeout(1)` uses. Both durations are at most 365 days; a longer
one is a usage error (exit 64). stdout carries only the answer: one line, or
with `--json` one object, `{"watch", "sha", "state", "exit", "timed_out", "reason",
"pipeline"}`, where `reason` says why there is no verdict (or is `null`) and `pipeline` is
the row as `check --json` prints it (or `null`). Progress goes to stderr, once per change. It is read-only, so a token with read scope is enough,
it reads only that watch's account's token, and it keeps no notification ledger.

```
git push && bridgewatch wait --timeout 30m && echo "shipped"
```

**`init`** is the wizard without the UI or the network. It takes `--provider` (`gitlab`,
the default, or `github`), `--project` (required; an id, a path or a project URL on
GitLab, `owner/repo` or a repository URL on GitHub), `--base-url`, `--account`, `--ref`,
`--watch-id`, `--workflow` (GitHub only), `--source` (repeatable; a workflow event on
GitHub), `--deploy-marker` (repeatable), `--schedule`, `--preflight <glob>` (GitLab
only), `--primary`, `--live-secs`, and one token source (`--oauth`, `--oauth-client-id
<id>`, `--glab`, `--gh`, `--token-env <var>`, `--token-keyring <service>` or
`--token-command <program> <arg>...`). With none, an account the file already has keeps
its source, and a new one gets `oauth = true` on github.com and gitlab.com and `own =
true` elsewhere. `--token-command` takes everything after it, arguments starting with `-`
included (`--token-command gh auth token --hostname ghe.example.com`), so it goes last. It reads no token, fetches nothing and writes nothing: it prints the file
to stdout, and if a config file already exists it prints that file edited, with its
comments and other watches kept. When that file already has a primary watch, the new
watch is added as secondary with a note on stderr; `--primary` keeps it primary. A new
GitHub watch polls at 30 s live and 120 s idle unless `--live-secs` says otherwise.
Once it reads right, write it to a new file, check it, and move that over the config.
Never redirect it onto the config itself, which the shell empties before `init` reads it.

**`log`, `retry` and `play`** take a job's page URL, as the popover links to it:
`https://<host>/<group>/<project>/-/jobs/<id>` on GitLab (a `base_url` with a path prefix
included) or `https://github.com/<owner>/<repo>/actions/runs/<run>/job/<id>` on GitHub.
The URL is matched to a configured account by its host, the same rule links are opened
by; when two accounts share a host, `--account` names one. `retry` and `play` are writes:
they need `actions = true` on the account (exit 78 otherwise, before any token is read),
ask `Retry job ... ? [y/N]` on the terminal, and without a terminal refuse unless `--yes`
is given (exit 64). Answering no exits 1 and sends nothing. `play` on a GitHub account is
a usage error.

**`fixture record`** walks one parent pipeline and its children into a fixture directory
for the core's tests; `fixture scrub` re-applies the allow-list that keeps personal data
out of fixtures. It reads each named directory's own files, not subdirectories, and a
directory with no recorded files in it is refused with exit 64. Recording is GitLab-only: a github account is refused with exit 64. See
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Exit codes

Verdict codes and error codes never overlap, so a script can tell a red pipeline from a
typo in its own command line. For `check` and `watch` the code is the tray icon's state,
that is the worst state among the primary watches.

`--watch <id>` makes the watches you name the subject, so when none of them is primary
the code is the worst state among **all** of them: `check --watch hourly` on a failed
schedule exits 1, not 4. Name a primary watch alongside and the tray rule applies again,
because a secondary watch must not be able to outvote a primary one.

```
  0   check/watch/wait: deployed or succeeded_no_deploy; any other command: success
  1   check/watch/wait: failed; fixture scrub --check: a fixture would change; retry/play: answered no
  2   check/watch/wait: deployed_with_failure
  3   check/watch: running, parked_gate or canceled; wait: parked_gate or canceled, or interrupted
  4   check/watch: unknown (nothing matched, or a request failed; errors on stderr); wait: skipped, no pipeline after --no-pipeline-grace, or a refused request (401, 403, 404)
  64  usage error: bad arguments, a --watch, --account or job URL that names nothing configured, a fixture scrub directory with no recordings, or retry/play with no terminal and no --yes
  70  any other error (token, fixture, recording, I/O, or a log or job request that failed)
  78  the configuration is missing, unreadable or invalid, or retry/play on an account without actions = true
  124 wait: --timeout ran out before the commit's pipeline had a verdict
```

For `wait` the code is the state of the one pipeline it waited for, never the tray's.
