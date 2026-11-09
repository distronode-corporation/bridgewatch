# Using the app

## Running it

- **No Dock icon on macOS.** bridgewatch is a menu bar item. Left-click the icon for the
  popover, right-click for the menu.
- **Many Linux tray hosts deliver no left-click** (GNOME's extension included), so the
  first menu item, **Show status**, opens the popover. The menu also has **Open pipelines
  page** (the repository's Actions page on GitHub), **Refresh now**, **Pause notifications** (for 1 hour, or until resumed; while paused the
  submenu reads **Notifications paused** and offers **Resume**),
  **Reload config**, **Open config file**, **Open log folder**, **Settings…**, **Setup
  wizard…**, **Launch at login** and **Quit bridgewatch**.
- **Config problems** appear in a strip at the top of the popover, with a button that
  opens Settings. A file that fails validation does not take effect; the last good
  configuration keeps running.
- **The Debug section**, opened from **Debug** in the popover's footer, shows the config path, the last
  poll and the recent API requests with status, timing and rate-limit headers. A 404
  there usually means the project id or the token's scope (GitHub answers 404, not 403,
  for a private repository the token cannot see), a 401 means the token, and no requests
  at all for a watch means its ref or source filter matched nothing. On GitHub a 304 is a
  request the rate limit did not count, see [Rate limits](github-actions.md#rate-limits).
- **Arguments.** `--config <path>` (also `-c <path>` or `--config=<path>`), `--help`,
  `--version`. `BRIDGEWATCH_CONFIG` names the config file too; an empty value counts as
  unset.
- **Logging** goes to stderr and to a log file, one per day as
  `bridgewatch.<date>.log`, of which the last 7 are kept. The folder is
  `~/Library/Logs/bridgewatch` on macOS (Console.app lists it) and
  `$XDG_STATE_HOME/bridgewatch` on Linux (by default `~/.local/state/bridgewatch`);
  **Open log folder** in the tray menu opens it. On macOS the file is the only log an
  app started from Finder, the Dock or a login item has, because its stderr goes
  nowhere. If the folder cannot be created or written, the app keeps running with stderr
  alone and says so once, at `warn`. The level is the same for both: a non-empty
  `RUST_LOG` wins for the life of the process; otherwise `[log].level` applies (to
  bridgewatch's own crates; everything else stays at `warn`) and is re-applied on
  reload. A file with no `[log]` table gets the default, `info`. Until a configuration
  has loaded (or when none can be loaded), the level is `warn`. The `bridgewatch` CLI
  logs to stderr only.
- **Not Mac App Store eligible.** The popover's transparent window uses Tauri's
  `macos-private-api` feature.

## The job view, and what "live" means

Each pipeline row in the popover expands into its jobs: the parent's own jobs, then one
row per bridge with that child pipeline's (or workflow run's) jobs, each list grouped by
stage in pipeline order, with a status dot per job, a marker on running jobs and a duration that ticks
locally between polls. Live pipelines start expanded and settled ones collapsed; a bridge
that failed or lost its child starts expanded either way. Each row's second line starts with the
commit: its short sha, then what started it and, with `[ui].commit_titles` on (the
default), its title. What you open or close stays
that way across refreshes. Every job links to its page on GitLab or GitHub. A GitHub
workflow in `expect` that never started shows as a bridge marked **never started**, linked
to the workflow's page.

A failed job, in the parent or in a child pipeline, on GitLab or GitHub, has a **log**
button. It opens the end of that job's log under the row: the last 40 lines, in a
scrollable box, with a link to the full log on its page (opened through the same
[host rule](#opening-links) as every other link). The log is read when you open it and
never by the poller, and at most 256 KiB of it is read, from the end. It is shown as
plain text: terminal colour codes, GitLab's collapsible-section markers, lines a
progress bar overwrote, control characters and the runner's per-line timestamps
(GitHub's, and GitLab's when the runner adds them) are taken out. GitHub serves a log
from a signed storage URL that its API redirects to. bridgewatch follows that one
redirect itself, only to an `https` URL, and sends no token or credential header there:
the signature in the URL is the authorisation, and the request log shows that leg as
`(redirect) host/path` without the signature.

`[ui].jobs` picks what an expanded row lists: `all` (every job, the default) or
`failures` (only failed jobs, tolerated failures and manual gates, and only the bridges
that have something to say). `watches.show.jobs` overrides it per watch. This is
presentation only: the verdict never depends on it.

**Neither GitLab nor GitHub offers a desktop app a push channel.** Webhooks need a public
inbound URL, and the web UIs' live updates use internal channels. "Real time" in
bridgewatch therefore means polling: every `poll.live_secs` (default 5 s) while any
pipeline in any watch is running, and every `poll.idle_secs` (default 60 s) when
everything has settled. The popover's header shows how long ago the last poll was ("9s
ago"), and opening it polls at once if the last poll is at
least 2 s old. Waking the machine from sleep polls at once too: every 5 s bridgewatch
compares the wall clock with the monotonic clock, which stops during sleep, and a jump of
10 s or more between two readings means the machine slept (or the clock was set
forward).

What a live tick costs, per GitLab watch: one pipeline-list request, plus two (`/jobs` and
`/bridges`) for each pipeline that is live or changed, plus one for each dived child
pipeline that is live or moved. A settled, unchanged pipeline is served from cache, so an
idle watch costs its list request only. For the shipped example (a push pipeline with
four child pipelines, plus two secondary watches) that is 9 requests a tick, 108 a
minute at 5 s. GitLab.com currently allows authenticated API traffic of 2,000 requests a
minute per user. GitLab.com has also published proposed per-plan limits, not in effect
as of this release, whose Free-plan figure is 100 a minute; under those a busy estate on
a Free account would be rate limited while a pipeline runs, and bridgewatch would back
off (up to `rate_limit_backoff.max_secs`). Raise `live_secs` if that matters to you.
GitHub's budget is far smaller and counts differently, see [Rate limits](github-actions.md#rate-limits).

## My builds

`mine = true` on a watch keeps only the pipelines (GitLab) or workflow runs (GitHub) you
started, on a busy project where everyone's pushes would otherwise share the rows:

```toml
[[watches]]
id = "my-pushes"
account = "gitlab"
project = "group/project"
ref = "main"
mine = true
```

The filter is applied by the server: GitLab is asked with `username=`, GitHub with
`actor=`. "You" is the signed-in user, or for a token source the token's owner, asked once
(`GET /user`) and remembered; a sign-in that already names its user needs no request. In
Settings it is **Only my pipelines**, in the watch's Source group. It applies to the rows
and so to everything drawn from them: the icon, notifications, `check` and `wait`.

## Deploy ETA

While a push is still on its way, a row says how long this watch's deploys usually take
and how far in this one is: `usually ~11m · 6m 30s in` in the popover, and
`usually ~11m, 6m 30s in` in `bridgewatch check` and `bridgewatch watch`. It costs no
API requests: it is learned from pipelines the watch has already fetched.

- **What is measured.** When a pipeline's deploy first reads `live` (a marker succeeded
  and no other marker can still move), bridgewatch records the time from the pipeline's
  creation to the marker job finishing. A pipeline is measured once. A missing or
  unreadable time is no sample, and so is a zero or negative one. Any watch with
  `deploy_markers` learns, primary or secondary; one without has nothing to learn from.
- **What is shown.** The median of the last 20 samples, once there are at least 3, on a
  row whose deploy is still in progress, or that is running on a watch with markers
  none of which has appeared yet. It disappears when the row settles, and once the
  elapsed time passes twice the usual one, where a typical figure no longer says
  anything.
- **Where it is kept.** `eta.json` beside the notification ledger, in the state
  directory: `~/Library/Application Support/bridgewatch` on macOS (the data directory,
  as macOS has no state directory) and `$XDG_STATE_HOME/bridgewatch` on Linux (by
  default `~/.local/state/bridgewatch`). The app and the CLI share it; each re-reads it
  and merges before writing, and writes to a temporary file that is renamed over it. A
  corrupt file costs its samples and one warning, never a poll. `check --fixture` and
  `watch --fixture` learn in memory only.
- **Keyed by the watch's `id`.** Renaming a watch starts its history again.

`eta = false` on a watch turns all of it off: nothing is recorded and nothing is shown.
In `check --json` and `watch --json` a running row with an estimate carries
`"eta": { "typical_secs": 660, "elapsed_secs": 390 }`; every other row has no `eta` key.

## Opening links

Every URL the app opens (a job or pipeline in the popover, **Open pipelines page** in the
tray menu) goes through one function in the Rust shell. It opens only `http` and `https`
URLs whose scheme, host and port are those of a configured account's `base_url`, or for a
github account its web host (`https://github.com` for `https://api.github.com`, the
server itself for Enterprise Server), and refuses and logs anything else, look-alike
hosts and other ports included. The URLs come from the provider's API, so this is what
stops a hostile or compromised instance from making the app open an arbitrary page or a
`file:` URL. The web views have no permission to open URLs themselves.

## Crash reports (opt in)

The release builds can send a crash report to Distronode's Sentry organisation in the
European Union (de.sentry.io) when the tray app fails. They are off until you turn them on. The popover asks once, with **Don't
send** as the default, and **Settings → General → Send crash reports** changes the answer
later; both write `[ui].send_crash_reports`. Turning it off takes effect at once.

- **Nothing is sent before a yes.** With the key unset or `false`, no crash-report client
  is created at all. A test (`src-tauri/tests/crash_reports.rs`) points the client at a
  local server and checks that a panic sends zero requests with consent off, one with
  consent on, and none again once consent is taken back.
- **What a report holds:** the error message, the stack (function names, and source
  file names and line numbers where the build has them), the app's release, the OS name
  and version, the CPU architecture and the list of loaded libraries.
- **What it does not:** every home-directory path in it (`/home/<name>`, `/Users/<name>`,
  `C:\Users\<name>`) has the name replaced with `<user>` before it leaves the machine,
  and the user, IP address and host name fields are removed. The config file, tokens,
  pipelines and logs are not attached. Sentry still sees the connection's IP address, as
  any server does, and is set not to store it.
- **Only the release builds made by this repository's workflows can send,** and only
  they ask: the release workflow compiles the reporting address in
  (`BRIDGEWATCH_SENTRY_DSN`), and the code does not contain it, so a build you compile
  yourself, or a fork's, never asks and never sends. The CLI has no crash reporting.
