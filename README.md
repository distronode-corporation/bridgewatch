# bridgewatch

[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/distronode-corporation/bridgewatch/badge)](https://scorecard.dev/viewer/?uri=github.com/distronode-corporation/bridgewatch)

GitHub and GitLab pipelines in one tray app.

bridgewatch is a tray monitor for GitLab CI and GitHub Actions. It shows one row per push
with every downstream pipeline as a bridge, tells a scheduled pipeline apart from a push,
and reports whether the thing you care about actually deployed, instead of the raw
pipeline status.

A Rust core crate, a `bridgewatch` CLI and a Tauri 2 GUI (Svelte 5). macOS and Linux.
Apache-2.0.

## Why another CI tray monitor

Most CI tray monitors define "project status" as the status of the newest pipeline on a
branch. On a real project three things go wrong with that.

**Scheduled pipelines dominate the branch.** A project with an hourly schedule on `main`
has far more scheduled pipelines than pushes, and a schedule can be red by design (a
reconciler that fails on purpose so drift is visible, for example). A monitor that reads
the newest pipeline then shows red most of the day and is soon ignored. It is not wrong
about any single pipeline; it is answering a different question.

**Fan-out hides the real result.** On GitLab a parent pipeline fans out to child
pipelines through trigger jobs (GitLab calls them bridges), and the parent's status is an
aggregate. On GitHub a push fans out into several independent workflow runs, and nothing
above them has a status at all. Either way nothing says *which* component broke, or "the
website deployed while a sibling failed", which is the most common shape of a partly-bad
push, and nothing notices a workflow that should have run and never started.

**"Pipeline succeeded" is not "it shipped".** A pipeline can be green because the deploy
job was skipped, is manual, or never ran. The question worth putting in a tray icon is
whether a named job ran and succeeded.

So bridgewatch is:

- **bridge-aware.** One row is one push, and each downstream is a bridge on it. On
  GitLab that is the parent pipeline, whose trigger jobs bridgewatch walks into their
  child pipelines (`dive.depth` levels deep, default 1). On GitHub it is a commit group
  (`group = "commit"`): every workflow run one push started, folded into one row with
  each run as a bridge. A bridge that never produced its downstream (a trigger job that
  created no child, or a workflow listed in `expect` that never started) reads dead.
- **source-aware.** A watch filters by pipeline source or workflow event (`push`,
  `schedule`, ...) and by ref, so pushes, schedules and preflight branches are separate
  watches on the same project. Only `primary` watches drive the tray icon and
  notifications; `secondary` watches are rows in the popover and nothing else.
- **verdict-driven.** The icon state comes from marker jobs you name (for example
  `deploy:origins`), found anywhere in the parent or a child, not from the parent's
  status field.

## Icon states

| State | Glyph (builtin) | Means |
| --- | --- | --- |
| `failed` | red octagon | The pipeline carrying the deploy marker failed before any marker succeeded, a bridge is dead (it never produced its downstream) and no marker exists, a blocking job in the parent failed, or (with no marker) a bridge failed. |
| `deployed_with_failure` | amber triangle | A marker succeeded, and a sibling bridge failed or a blocking job failed around it, under the default `downgrade` policies. |
| `deployed` | green filled check | A marker succeeded and nothing is holding it back. |
| `running` | blue arrows | A marker is in flight, or (with no marker) something is still running. |
| `canceled` | grey slash | The parent was canceled or skipped, or every marker was. |
| `parked_gate` | amber hourglass | No marker, nothing running, nothing broken, and a manual job or bridge is waiting for a person. |
| `succeeded_no_deploy` | green outline check | Everything settled green and no marker ran. Normal for a lane that does not deploy. |
| `unknown` | grey question mark | No data yet, the pipeline's job lists could not be read, a marker is in a status this build does not know, or a verdict script failed. |

The icon reflects the newest pipeline of each primary watch. With more than one primary
watch the worst state wins (in the order `failed`, `deployed_with_failure`, `running`,
`canceled`, `parked_gate`, `unknown`, `deployed`, `succeeded_no_deploy`), and
`config validate` warns about it. On macOS the glyphs are monochrome template images
that follow the menu bar; on Linux they are coloured. The exact rules are under
[Verdict rules](#verdict-rules).

## GitHub Actions

GitHub has no parent/child pipelines. A push fans out into independent workflow runs,
related only by their commit, event and branch. bridgewatch reads a GitHub watch in one of
two forms, chosen with the watch's `group` key:

- **`group = "run"`** (the default): one workflow run is one row, and the run's jobs are
  its job list. `workflow` narrows the watch to one workflow file; without it every
  workflow's runs are rows of their own.
- **`group = "commit"`**: every run of one push (same commit, event and branch, created
  within `fan_out_secs` of the newest run) is one row, and each run is a bridge on it. This
  is the GitHub equivalent of a GitLab parent pipeline, and it is what gives the verdict
  something to say: a push that deployed while a sibling workflow failed reads
  `deployed_with_failure`, exactly as a GitLab parent with a failed bridge does. The row's
  status is the worst of its runs, and its link is the commit's checks page. `dive`
  selects which runs' jobs are fetched, by workflow name.

`expect` adds the one thing the runs alone cannot show: a workflow that should have run
and never started (its `on:` did not match, or GitHub refused the file). See
[`expect`](#expect-a-workflow-that-never-started).

### Quick start: the wizard

Pick **GitHub Actions** on the first step of the [setup wizard](#first-run-the-setup-wizard),
then **github.com** or **GitHub Enterprise**. If you have run `gh auth login`, the wizard
offers **Use gh's token**: it writes the keyring source for gh's item (see
[Reusing gh's login](#reusing-ghs-login)) and never copies the token. Then pick a
repository (or type `owner/repo` or its URL), the events to watch, and optionally one
workflow file. A new GitHub watch polls every 30 s while live and every 120 s when idle,
because of the [rate limit](#rate-limits).

The command-line equivalent prints the same file without touching the network:

```
bridgewatch init --provider github --project octo-org/octo-repo --gh --workflow ci.yml
```

The wizard writes a `group = "run"` watch: one row per workflow run, of the one workflow
you named or of every workflow. To get one row per push, set **One row is** to `commit` on
the watch in Settings, and add `expect` there, or write it by hand as below.

### A hand-written config for github.com

The shipped [`examples/github.toml`](examples/github.toml) follows this repository's own
workflows in both forms. The minimal versions:

```toml
[accounts.github]
provider = "github"            # base_url, api_path and header default to github.com's
token = { keyring = { service = "gh:github.com", user = "" } }

# One row per push, each workflow run a bridge.
[[watches]]
id = "main-push"
account = "github"
project = "octo-org/octo-repo"          # owner/repo; GitHub has no numeric form
ref = "main"
sources = ["push"]
group = "commit"
expect = ["ci.yml", "release.yml"]      # workflow FILES every push to main must start
deploy_markers = ["deploy"]
poll = { live_secs = 30, idle_secs = 120 }

# One workflow, one row per run.
[[watches]]
id = "nightly"
account = "github"
project = "octo-org/octo-repo"
ref = "main"
sources = ["schedule"]
workflow = "nightly.yml"                # the file name, or the workflow's numeric id
role = "secondary"
poll = { live_secs = 30, idle_secs = 120 }
```

Write the `poll` line yourself in a hand-written file: without it a watch gets the
defaults of 5 s and 60 s, which are sized for GitLab.

### Tokens for GitHub

A **classic** personal access token needs the `repo` scope to read a private repository,
and no scope for a public one. There is no narrower classic scope for reading Actions, and
`workflow` is not one: it grants write access to workflow files. A **fine-grained** token
needs **Actions: read** and **Metadata: read** on the repositories you watch.
**Test connection** in the wizard lists a classic token's scopes and warns when `repo` is
missing; GitHub reports no scopes for a fine-grained token, so the wizard says it cannot
check one. GitHub reads a token only from the `Authorization` header, which is
the default for a github account, and `header = "PRIVATE-TOKEN"` on one is an error.

#### Reusing gh's login

If you have run `gh auth login`, gh keeps its token in the OS keyring under service
`gh:<host>` (`gh:github.com` for github.com). gh writes two items there: one under your
login and one with an **empty** user, which is the active account that `gh auth switch`
moves. bridgewatch reads the empty-user item, so switching accounts in gh carries over:

```toml
token = { keyring = { service = "gh:github.com", user = "" } }
```

`bridgewatch init --gh` and the wizard's **Use gh's token** write exactly that, with the
host taken from the account (`gh:ghe.example.com` for an Enterprise Server host). The
portable alternative runs gh itself, and works wherever gh does, including when gh keeps
its token in a plain file rather than the keyring:

```toml
token = { command = ["gh", "auth", "token"] }
# GitHub Enterprise Server: ["gh", "auth", "token", "--hostname", "ghe.example.com"]
```

A command source asks for confirmation before the app writes it, see
[Command sources need confirmation](#command-sources-need-confirmation).

### GitHub Enterprise Server

github.com serves its API on its own host with no path prefix, so a github account
defaults to `base_url = "https://api.github.com"` and an empty `api_path`. Enterprise
Server serves it under a path on the server's own host:

```toml
[accounts.ghe]
provider = "github"
base_url = "https://ghe.example.com"
api_path = "/api/v3"
token = { keyring = { service = "gh:ghe.example.com", user = "" } }
```

The wizard and `bridgewatch init --provider github --base-url https://ghe.example.com`
write the `/api/v3` for you. Links open on the web host: the server itself for
Enterprise Server, and `https://github.com` for an account on `api.github.com`.

### Rate limits

A GitHub token gets 5,000 API requests an hour, shared with everything else that uses the
same token, gh included. That is about 83 a minute, where gitlab.com allows 2,000 a
minute, which is why a GitHub watch should poll more slowly than a GitLab one.

What a tick costs, per GitHub watch: one request for the list of runs, plus one (`/jobs`)
for each run that is live or changed and whose jobs are read. GitHub has no trigger jobs to
fetch, and a commit group's bridges come from the list itself.

bridgewatch sends conditional requests to GitHub (`If-None-Match` with the ETag of the
last answer), and GitHub does not count an authenticated `304 Not Modified` against the
limit. So a watch whose runs have not changed costs nothing: at the 120 s idle interval a
settled watch sends 30 requests an hour and spends none of its budget. The validators are
kept in memory only, so the first poll after a start is paid for in full. The Debug
section shows the 304s and the remaining budget.

An exhausted limit is a 403 or 429 carrying `x-ratelimit-remaining: 0` or `retry-after`.
bridgewatch treats it as a rate limit, not a bad token, and waits until GitHub's reset time
(or the `retry-after`) before asking again, whatever `rate_limit_backoff.max_secs` says.

### `expect`: a workflow that never started

On GitLab a trigger job that created no child is still there to be read as dead. On
GitHub a workflow that did not start leaves nothing at all: no run, so nothing to be red.
`expect` names the workflows every commit group must hold, and an absent one becomes a
dead bridge.

- Entries are workflow **file** names (`ci.yml`, or the full `.github/workflows/ci.yml`),
  not the display names the bridges show. A file name does not change when someone edits
  the workflow's `name:`.
- **Timing.** A group is decided once every run in it has settled and `fan_out_secs` has
  passed since its newest run. Until then a missing workflow is a pending job, and the
  row reads `running`, not green. After that it is a dead bridge: the row reads `failed`,
  or `deployed_with_failure` when a marker has already succeeded (under the default
  `sibling_failure = "downgrade"`), and a primary watch raises a `blocking_failure`
  notification. A real failure in a run that does exist still turns the row red at once.
- A run that ended in `startup_failure` is a failed run, not an absence.
- `expect` applies to every group the watch shows, so list only workflows that run on
  **every** event in `sources`, and none whose `paths` or `branches` filters can
  legitimately skip a push. A workflow that starts later than `fan_out_secs` after the
  others lands in a row of its own.

`config validate` warns about `expect` on a GitLab watch or with `group = "run"`, about
duplicates, empty entries and entries that are not `.yml` or `.yaml` file names, about an
entry that `workflow` rules out, and about `expect` with an empty `sources`.

### What GitHub does not have

- **No `allow_failure`.** `continue-on-error` does not appear in GitHub's jobs API, so
  every failed job counts as a failure. Use a [`[watches.jobs]`](#reference) override to
  tolerate one (`"lint" = "warning"`). On a commit group the same table applies to the
  bridges, so it can tolerate a whole workflow by its name.
- **No `dive.depth`.** GitHub runs do not nest: a called (reusable) workflow's jobs are in
  its caller's job list, named `caller / callee`, and bridgewatch groups them by the part
  before ` / `. `dive.depth` above 1 on a GitHub watch is a warning and is ignored.
- **No stages.** The ` / ` prefix above is the only grouping a GitHub job list has.
- **No preflight watch in the wizard.** A `pf/*` watch is a GitLab habit; on GitHub add a
  watch with a glob `ref` in Settings or by hand if you want one.
- **No fixture recording.** `bridgewatch fixture record` refuses a github account (exit
  64) until GitHub recordings have their own personal-data allow-list.

## Platform support

| Platform | Status |
| --- | --- |
| macOS 10.15 or newer, Apple Silicon and Intel | Supported. Release builds are signed and notarised, see below. |
| Linux x86_64, glibc 2.35 or newer (Ubuntu 22.04 or newer, and equivalents) | Supported. |
| Windows | Not targeted. |

Linux release builds are made on Ubuntu 22.04. glibc is forward-compatible but not
backward-compatible, so that is the floor: the `.deb` and `.AppImage` run on 22.04 and
newer, and not on older.

On Linux the tray is a StatusNotifierItem through `libayatana-appindicator`. GNOME has no
built-in host for that protocol, so on GNOME you need the AppIndicator/KStatusNotifierItem
extension enabled or the icon never appears. KDE, Cinnamon and XFCE work as they are.

## Install

Download from the [Releases page](https://github.com/distronode-corporation/bridgewatch/releases).

| File | For |
| --- | --- |
| `bridgewatch_<version>_aarch64.dmg` | macOS, Apple Silicon |
| `bridgewatch_<version>_x64.dmg` | macOS, Intel |
| `bridgewatch_<version>_amd64.deb` | Debian and Ubuntu, x86_64 |
| `bridgewatch_<version>_amd64.AppImage` | Other glibc 2.35+ Linux, x86_64 |

Each `.dmg` is single-architecture; pick the one for your Mac. Every asset carries a
GitHub build provenance attestation:

```
gh attestation verify bridgewatch_1.0.0_amd64.deb --repo distronode-corporation/bridgewatch
```

The release assets contain the GUI only. The `bridgewatch` CLI is built from source,
see [CLI](#cli).

### macOS: signed and notarised

The `.app` inside each `.dmg` is signed with the Developer ID **Distronode Corporation
(R935BA6767)** and notarised by Apple, with the ticket stapled, so it opens like any other
downloaded app. To check a copy yourself:

```
codesign --verify --deep --strict /Applications/bridgewatch.app
spctl --assess --type execute --verbose /Applications/bridgewatch.app
```

The second command should end with `source=Notarized Developer ID`.

### Linux: .deb

```
sudo apt install ./bridgewatch_1.0.0_amd64.deb
```

The package depends on `libsecret-tools`, because bridgewatch runs `secret-tool` to read a
keyring item that has an empty user name, which is how glab stores its token and how gh
stores its active account's (see [Tokens](#tokens)).

### Linux: AppImage

```
chmod +x bridgewatch_1.0.0_amd64.AppImage
./bridgewatch_1.0.0_amd64.AppImage
```

An AppImage declares no dependencies. If you reuse glab's or gh's token, install
`secret-tool` yourself (`sudo apt install libsecret-tools` on Debian and Ubuntu). A token stored in
bridgewatch's own keyring entry goes through the Secret Service over D-Bus and does not
need it; either way a Secret Service (GNOME Keyring, KWallet) has to be running.

## First run: the setup wizard

When there is no config file at the default path, bridgewatch opens a setup wizard. It is
optional: **Skip, I'll edit config.toml** writes nothing and opens Settings on the **Edit
as text** tab, where you can write the file yourself (start from
[`examples/distronode.toml`](examples/distronode.toml) for GitLab or
[`examples/github.toml`](examples/github.toml) for GitHub). Saving from that tab creates
it.

The wizard's steps:

1. **Account.** GitLab (the default) or GitHub Actions; gitlab.com or a self-managed
   URL, or github.com or a GitHub Enterprise URL; and a token source: the provider CLI's
   keyring item (glab's or gh's, offered when one exists for that host), a pasted token
   (stored in bridgewatch's own keyring entry), an environment variable, or a command.
   **Test connection** names the user the token authenticates as. On GitLab it warns
   when the token is a project access token or lacks the `read_api` scope; on GitHub it
   lists a classic token's scopes and warns when `repo` is missing, see
   [Tokens for GitHub](#tokens-for-github).
2. **Project.** Pick from the projects (or repositories) the token can list, or type an
   id, a `group/project` path or a project URL on GitLab, or `owner/repo` or a
   repository URL on GitHub. A GitLab project access token cannot list projects, so it
   always gets the typing form.
3. **What to watch.** The default branch and push pipelines are pre-filled. On GitHub
   the events are workflow events, and an optional workflow file narrows the watch to
   one workflow. Optionally add a secondary watch for scheduled pipelines on the same
   branch, and on GitLab one for a preflight ref glob such as `pf/*`.
4. **Deploy detection.** Suggested marker jobs, ranked from the latest pipeline's (or
   workflow run's) job names and stages (parent and direct children), or none.
5. **Notifications and polling.** Which events notify, launch at login, and the live
   poll interval (5 s for GitLab, 30 s for GitHub).
6. **Review config.toml.** The exact file it will write, then **Finish**.

The wizard writes through the same path as Settings. Run on an existing file, it edits
that file rather than replacing it, and it refuses to write text that does not load or
that contains something shaped like a GitLab token (or, for a GitHub account, a GitHub
token). When the file already has a primary watch, the new watch is added as
`role = "secondary"`, so it does not take over the tray icon; tick **Make this watch
primary too** on the review step to keep it primary. Re-open the wizard any time from the
tray menu (**Setup wizard…**) or from Settings.

`bridgewatch init` is the same logic from the command line, see [CLI](#cli).

A `--config` or `BRIDGEWATCH_CONFIG` path that does not exist is treated as a typo, not
as a first run: bridgewatch reports it and creates nothing.

## Running it

- **No Dock icon on macOS.** bridgewatch is a menu bar item. Left-click the icon for the
  popover, right-click for the menu.
- **Many Linux tray hosts deliver no left-click** (GNOME's extension included), so the
  first menu item, **Show status**, opens the popover. The menu also has **Open pipelines
  page** (the repository's Actions page on GitHub), **Refresh now**, **Reload config**,
  **Open config file**, **Settings…**, **Setup wizard…**, **Launch at login** and **Quit
  bridgewatch**.
- **Config problems** appear in a strip at the top of the popover, with a button that
  opens Settings. A file that fails validation does not take effect; the last good
  configuration keeps running.
- **The Debug section** at the bottom of the popover shows the config path, the last
  poll and the recent API requests with status, timing and rate-limit headers. A 404
  there usually means the project id or the token's scope (GitHub answers 404, not 403,
  for a private repository the token cannot see), a 401 means the token, and no requests
  at all for a watch means its ref or source filter matched nothing. On GitHub a 304 is a
  request the rate limit did not count, see [Rate limits](#rate-limits).
- **Arguments.** `--config <path>` (also `-c <path>` or `--config=<path>`), `--help`,
  `--version`. `BRIDGEWATCH_CONFIG` names the config file too; an empty value counts as
  unset.
- **Logging** goes to stderr. A non-empty `RUST_LOG` wins; otherwise `[log].level`
  applies (to bridgewatch's own crates; everything else stays at `warn`) and is
  re-applied on reload; with neither, the level is `warn`.
- **Not Mac App Store eligible.** The popover's transparent window uses Tauri's
  `macos-private-api` feature.

## The job view, and what "live" means

Each pipeline row in the popover expands into its jobs: the parent's own jobs, then one
row per bridge with that child pipeline's (or workflow run's) jobs, each list grouped by
stage in pipeline order, with a status dot per job, a marker on running jobs and a duration that ticks
locally between polls. Live pipelines start expanded and settled ones collapsed; a bridge
that failed or lost its child starts expanded either way. What you open or close stays
that way across refreshes. Every job links to its page on GitLab or GitHub. A GitHub
workflow in `expect` that never started shows as a bridge marked **never started**, linked
to the workflow's page.

`[ui].jobs` picks what an expanded row lists: `all` (every job, the default) or
`failures` (only failed jobs, tolerated failures and manual gates, and only the bridges
that have something to say). `watches.show.jobs` overrides it per watch. This is
presentation only: the verdict never depends on it.

**Neither GitLab nor GitHub offers a desktop app a push channel.** Webhooks need a public
inbound URL, and the web UIs' live updates use internal channels. "Real time" in
bridgewatch therefore means polling: every `poll.live_secs` (default 5 s) while any
pipeline in any watch is running, and every `poll.idle_secs` (default 60 s) when
everything has settled. The popover shows "updated Ns ago", and opening it polls at once if the last poll is at
least 2 s old.

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
GitHub's budget is far smaller and counts differently, see [Rate limits](#rate-limits).

## Tokens

bridgewatch never writes a token to its config file, and the config loader refuses a
literal token string with an error that lists the alternatives. The file records *where*
the token comes from; it is resolved when the poller starts and again on every reload.

```toml
token = { keyring = { service = "glab:gitlab.com:token", user = "" } }  # glab's item
token = { keyring = { service = "gh:github.com", user = "" } }          # gh's active account
token = { own = true }                                                  # bridgewatch's own item
token = { env = "BRIDGEWATCH_TOKEN_GITLAB" }                            # an environment variable
token = { command = ["pass", "gitlab/pat"] }                            # a program's output
```

| Source | Notes |
| --- | --- |
| `keyring` | Any OS keyring item, addressed by service and user. Use it to reuse glab's or gh's login. |
| `own` | bridgewatch's own keyring item, service `bridgewatch:<account>`, written when you paste a token into Settings or the wizard. This is the default when an account has no `token` key. |
| `env` | Read from the process environment, so it works when bridgewatch is started from a shell or a service unit that sets the variable. |
| `command` | Runs the program (no shell) with stdin closed and a 10 s timeout, and uses the first line of its stdout. Writing one from Settings or the wizard needs an explicit confirmation, see below. |

A token source table names exactly one source; a table naming two is refused.

### Reusing glab's login

If you have run `glab auth login`, the token is already in your OS keyring under service
`glab:<host>:token` (so `glab:gitlab.com:token` for gitlab.com) with an **empty** user
name. The empty user is required: glab's keyring library writes the item that way and a
lookup with any other user misses. The wizard and `bridgewatch init --glab` write this
source for you. The self-managed `glab:<host>:token` form follows glab's naming but has
not been checked against a live self-managed install. gh's login works the same way, see
[Reusing gh's login](#reusing-ghs-login).

On macOS glab's keyring library (which gh uses too) stores the value wrapped as
`go-keyring-base64:` plus base64 (older versions: `go-keyring-encoded:` plus hex);
bridgewatch unwraps either wrapper, and passes an unwrapped value through unchanged, so the same config works on macOS and Linux.

macOS may ask whether bridgewatch may read an item another program created. **Always
Allow** stops it asking again each time the token is read.

⚠️ `secret-tool lookup service glab:gitlab.com:token username ""` **prints the token**.
If you check the Linux item by hand, pipe it to `wc -c`, never into a log or an issue.

### Command sources need confirmation

A `command` source turns the config file into something that runs a program with your
permissions every time a token is needed. So when Settings or the wizard would write one
(a new one, or a changed command), the app writes nothing and shows the exact command for
you to confirm first. The same applies to a change that would send a credential to a host
it has not been sent to before: a new `base_url` for an account that already has a
token, or a keyring or environment source on a host no account used. Edits you make in your own editor are not
subject to this: the check is on the app's write path, and the file is yours.
`config validate` warns about every command source.

### GitLab project access tokens

A GitLab project access token can read only its own project, so it cannot list projects.
The wizard then asks you to type the project id or path instead of offering a list, and a
watch can always name a project by id. To watch several projects, use a personal access
token with `read_api`, or add one account per project.

## Configuration

`config.toml` lives in the OS config directory:

| OS | Path |
| --- | --- |
| Linux | `~/.config/bridgewatch/config.toml` (or under `$XDG_CONFIG_HOME`) |
| macOS | `~/Library/Application Support/bridgewatch/config.toml` |

`--config <path>` wins, then `BRIDGEWATCH_CONFIG`, then that default.
`bridgewatch config path` prints the one in effect.

The file is edited through `toml_edit`, so comments, key order and formatting survive a
save from the GUI, and it is hot-reloaded: save it in an editor and the running app picks
it up. The GUI saves by compare-and-swap, so a Settings window holding an old copy cannot
overwrite a change made on disk since; it tells you to re-read instead.

**GUI and file parity.** Every key in the file has a control in Settings and every control
maps to a key; tests compare the Settings registry with the generated JSON Schema, so a
key without a control fails CI. Settings has tabs for Accounts, Watches, Icon,
Verdict, UI and Log, plus **Edit as text**, which edits the same file directly with
diagnostics by line and column.

`bridgewatch config validate` reports every problem with its line and column.
`bridgewatch config schema` prints the JSON Schema (the same one committed as
`src/lib/config.schema.json`) for editor completion. `bridgewatch config dump` prints the
configuration with every default filled in.

### Reference

Every key is optional unless marked required. Defaults in brackets.

**`[accounts.<name>]`**, one per GitLab instance or GitHub host (at least one is required).
Accounts of both providers can sit in one file:

| Key | Meaning |
| --- | --- |
| `provider` | `gitlab` or `github` [`gitlab`, so a file written before GitHub support means what it always did]. `base_url`, `api_path` and `header` default per provider. |
| `base_url` | Instance root, no trailing slash [`https://gitlab.com`, or `https://api.github.com` for github]. For GitHub Enterprise Server, the server's root. `http://` is accepted, with a warning unless the host is loopback. |
| `api_path` | API prefix [`/api/v4`, or empty for github]. GitHub Enterprise Server needs `/api/v3`. |
| `token` | Token source, see [Tokens](#tokens) [`{ own = true }`]. |
| `header` | `"PRIVATE-TOKEN"` (GitLab personal and project access tokens) or `"Authorization: Bearer"` (GitLab OAuth and CI job tokens, and every GitHub token) [`PRIVATE-TOKEN`, or `Authorization: Bearer` for github, where `PRIVATE-TOKEN` is an error]. |
| `timeout_secs` | Per-request timeout [15]. |
| `rate_limit_backoff.max_secs` | Ceiling for the doubled interval after rate limiting or errors [300]. A `retry-after`, or GitHub's rate-limit reset time, is waited out in full even when it is longer. |

**`[[watches]]`**, in display order:

| Key | Meaning |
| --- | --- |
| `id` | Required. Unique; used by `--watch`, notifications and the GUI. |
| `account` | Required. An `[accounts.<name>]` key. |
| `project` | Required. GitLab: numeric id or `group/path`. GitHub: `owner/repo` (a numeric id is an error). |
| `ref` | Exact name, glob (`pf/*`), or `re:` plus a regex [`main`]. A `re:` pattern is unanchored; write `re:^…$` to match the whole ref. |
| `sources` | Pipeline sources (GitLab: `push`, `schedule`, `web`, `merge_request_event`, ...) or workflow events (GitHub: `push`, `schedule`, `pull_request`, `workflow_dispatch`, ...) to accept. Empty means all [`[]`]. |
| `workflow` | GitHub only. One workflow, by file name (`ci.yml`) or numeric id; each run is a row. Absent means every workflow [unset]. |
| `group` | GitHub only. `run` (one workflow run is one row) or `commit` (every run of one push is one row, each run a bridge) [`run`]. See [GitHub Actions](#github-actions). |
| `fan_out_secs` | GitHub only, with `group = "commit"`. Runs of one commit, event and branch created within this many seconds of the group's newest run are one row [90]. |
| `expect` | GitHub only, with `group = "commit"`. Workflow file names every group must hold; one with no run is a dead bridge once the group has settled and its window has passed. See [`expect`](#expect-a-workflow-that-never-started) [`[]`]. |
| `role` | `primary` (drives the icon, may notify) or `secondary` (rows only) [`primary`]. |
| `show.max_rows` | Most rows this watch shows [5]. |
| `show.settled` | Settled pipelines kept below the unsettled ones [1]. |
| `show.jobs` | `all` or `failures`; overrides `[ui].jobs` for this watch [unset]. |
| `poll.live_secs` | Interval while anything is live [5; the wizard writes 30 for GitHub]. |
| `poll.idle_secs` | Interval when everything has settled [60; the wizard writes 120 for GitHub]. |
| `dive.bridges` | Glob over trigger-job names to walk into, or over workflow names on a GitHub commit group; `""` walks none [`*`]. |
| `dive.exclude` | Trigger-job (or workflow) name globs to skip [`[]`]. |
| `dive.depth` | Levels of child pipeline to walk [1]. GitLab only: GitHub runs do not nest. |
| `dive.only_when` | Walk only into bridges in this status, e.g. `"failed"` [unset]. |
| `deploy_markers` | Job names or `re:` patterns whose success means "deployed". With several, config order decides which success is reported [`[]`]. |
| `sibling_failure` | What a failed or dead bridge other than the marker's does to a deploy: `downgrade`, `fail` or `ignore` [`downgrade`]. |
| `post_deploy_failure` | The same for a blocking failure after the marker, or in the marker's own pipeline [`downgrade`]. |
| `[watches.jobs]` | Job-name pattern (literal or `re:`) to `gate`, `warning`, `blocking` or `ignore`. File order, first match wins. Applies to bridges too. On GitHub it is the only way to tolerate a failure. |
| `notify.deployed`, `notify.blocking_failure`, `notify.finished` | Notify on these events [true]. |
| `notify.started`, `notify.gate_opened` | Notify on these events [false]. |
| `notify.title`, `notify.body` | MiniJinja templates over the pipeline view (`watch.id`, `sha7`, `state`, `failures`, `warnings`, `gates`, `ref`, `source`, `url`, ...). |
| `notify.click` | `first_failure_or_pipeline`, `pipeline`, `marker_job` or `none`. Computed, but desktop notifications cannot open a URL yet, see [Limitations](#limitations). |

**`[icon]`**:

| Key | Meaning |
| --- | --- |
| `mode` | `template` (monochrome, macOS inverts it), `color`, or `auto` (template on macOS, colour elsewhere) [`auto`]. |
| `theme` | `builtin`, or a directory of `<name>.png` files (`<name>@1x.png` also works). A missing file falls back to the builtin one [`builtin`]. |
| `states` | State name to glyph: `question`, `octagon`, `triangle`, `check`, `check-outline`, `arrows`, `slash`, `hourglass`, or another state's name [`{}`]. |

**`[verdict]`**: `script`, a path to a [Rhai](https://rhai.rs) script (`~` expanded), or
`script_source`, an inline script that takes precedence. The script receives each
pipeline's view as `pipeline` and returns an icon-state name, replacing the built-in rules
for every watch. Sandboxed: `import` cannot load files, `eval` is disabled, and operations
and call depth are capped. A script that errors or returns an unknown name yields
`unknown` and an error line, never a confident wrong colour.

**`[ui]`**:

| Key | Meaning |
| --- | --- |
| `popover.width`, `popover.max_height` | Popover size in logical pixels [440, 720]. |
| `popover.hide_on_blur` | Hide the popover when it loses focus [true]. |
| `theme_css` | Path to a `.css` file (at most 256 KiB) layered over the built-in theme [`""`]. |
| `launch_at_login` | Register a login item. The checkbox in Settings and the tray menu set the OS login item and mirror its state here [false]. |
| `jobs` | `all` or `failures`, see [the job view](#the-job-view-and-what-live-means) [`all`]. |

**`[log]`**:

| Key | Meaning |
| --- | --- |
| `level` | `error`, `warn`, `info`, `debug`, `trace` or `off` [`info`], applied to bridgewatch's own crates with everything else left at `warn`. A non-empty `RUST_LOG` wins instead, whole and unscoped; an empty one counts as unset. Both the app and `bridgewatch` follow this. |
| `keep_requests` | Recent requests kept for the Debug section; 0 disables it [50]. |

An unknown key, and a value of the right type that names nothing (a misspelt source,
glyph or state, an unknown icon mode), is a warning and is ignored, so a file written for
a newer build still loads on an older one. A key that belongs to the other provider
(`workflow` on a GitLab watch, `dive.depth` above 1 on a GitHub one) is a warning too, so
a watch can move between accounts without making the file unloadable. `config validate` lists every warning with its
line and column. A value of the wrong type, or a literal token string, is an error.

### Walk-through of the examples

The GitLab example, [`examples/distronode.toml`](examples/distronode.toml), is the
configuration the project was built against: one gitlab.com account and three watches on
the same project.

- **`main-push`** is the only primary watch: push pipelines on `main`. It dives into every
  bridge, treats `deploy:origins` or `deploy:marketing` succeeding as "deployed", and
  lets a failure elsewhere downgrade a deploy to `deployed_with_failure` rather than
  hiding it or calling the deploy a failure. Its `[watches.jobs]` table marks a job that
  is manual on `main` as a `gate` (so waiting on it is not a failure, while a run of it
  that fails still is) and ignores a scanner job entirely.
- **`hourly`** is the same branch with `sources = ["schedule"]`, secondary, so a red
  scheduled pipeline shows as a row and never touches the icon.
  `dive.only_when = "failed"` skips the child-pipeline requests unless a bridge failed.
- **`preflights`** watches the `pf/*` branch glob with `dive.bridges = ""`: for a
  preflight, the parent's status is the answer, and one request a tick is enough.

The GitHub example, [`examples/github.toml`](examples/github.toml), follows this
repository's own workflows on github.com through gh's keyring item.

- **`main-push`** is the primary watch: push runs on `main`, folded by `group = "commit"`
  into one row per push, with CI and Scorecard as its bridges. `expect` names both files,
  so a push where either never started reads `failed` once the group settles. Nothing on
  `main` deploys, so it has no marker; `config validate` warns about that, deliberately,
  and the file's comment says why.
- **`audit`** follows one workflow, `audit.yml`, on its weekly schedule: secondary, one
  row per run.

Both GitHub watches poll at 30 s and 120 s, what the wizard writes for GitHub.

Job classes: `blocking` is the default for a job that is not allowed to fail, and a red
one fails the verdict. `warning` shows in the popover and does not fail it (a GitLab job
with `allow_failure: true` is a warning already, and `blocking` promotes it; GitHub
reports no such flag, so there an override is the only way to get one). `gate` marks a
manual or never-run job as a deliberate wait; it does **not** excuse a gated job that ran
and failed. `ignore` removes the job from every verdict and list.

## Verdict rules

For the newest pipeline of each watch that matches its `ref` and `sources`, bridgewatch
reads the parent's jobs and bridges, then the jobs of every child pipeline that `dive`
selects, classifies each job, and looks for the `deploy_markers` across all of them. On
GitHub the "parent" is a workflow run (with no bridges), or with `group = "commit"` the
push, whose bridges are its runs and whose own jobs are empty. The same rules apply to
both. The state is the first rule that matches:

1. **`unknown`** if the pipeline's job lists could not be read and nothing was cached.
2. **`failed`** if no marker succeeded and the markers' own pipeline has a blocking
   failure (a marker that failed counts even when it was allowed to fail), or no marker
   job exists and a bridge is dead (a trigger job that never created its child pipeline,
   or an expected GitHub workflow that never started), or a blocking job in the parent
   failed (unless it started after a successful marker, which rule 4 handles), or there
   is no marker and any bridge failed.
3. **`unknown`** if a marker is in a status this build does not recognise.
4. If a marker succeeded: **`failed`** when a sibling failure or post-deploy failure
   applies with policy `fail`; **`deployed_with_failure`** when one applies with policy
   `downgrade`; otherwise **`deployed`**. A sibling failure is a failed or dead bridge
   other than the one carrying the marker; a post-deploy failure is a blocking job that
   started after the marker, or one in the marker's own pipeline.
5. **`running`** if a marker is in flight, or there is no marker and something is live.
   A job waiting on a manual gate is not live.
6. **`canceled`** if the parent was canceled or skipped, or every marker was.
7. **`parked_gate`** if there is no marker, nothing is live, nothing blocking failed,
   and something is waiting on a manual gate. A marker that is itself a manual job counts
   as "no marker" here.
8. **`succeeded_no_deploy`** if there is no marker, nothing is live and nothing blocking
   failed.

`[verdict].script` replaces all of this for a watch's pipelines if your project needs
different rules. `bridgewatch check --json` prints each pipeline's facts (`deploy`,
`failures`, `warnings`, `gates`, `sibling_failures`, `post_deploy_failures`, the bridges
and their jobs), which is the fastest way to see why a pipeline got the state it did.

## Opening links

Every URL the app opens (a job or pipeline in the popover, **Open pipelines page** in the
tray menu) goes through one function in the Rust shell. It opens only `http` and `https`
URLs whose scheme, host and port are those of a configured account's `base_url`, or for a
github account its web host (`https://github.com` for `https://api.github.com`, the
server itself for Enterprise Server), and refuses and logs anything else, look-alike
hosts and other ports included. The URLs come from the provider's API, so this is what
stops a hostile or compromised instance from making the app open an arbitrary page or a
`file:` URL. The web views have no permission to open URLs themselves.

## CLI

The CLI runs the same core as the GUI, so anything the icon knows it can print: in a
shell prompt, a status bar or a script. It is not in the release assets; build it
with Rust 1.88 or newer, from a clone of this repository:

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
bridgewatch watch --json                 # one JSON snapshot per change instead
bridgewatch watch --ticks 3              # stop after three ticks

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
```

`--config <path>` (or `-c`) is accepted before or after any subcommand. `watch` does not
write the GUI's notification ledger; it keeps its own.

**`init`** is the wizard without the UI or the network. It takes `--provider` (`gitlab`,
the default, or `github`), `--project` (required; an id, a path or a project URL on
GitLab, `owner/repo` or a repository URL on GitHub), `--base-url`, `--account`, `--ref`,
`--watch-id`, `--workflow` (GitHub only), `--source` (repeatable; a workflow event on
GitHub), `--deploy-marker` (repeatable), `--schedule`, `--preflight <glob>` (GitLab
only), `--primary`, `--live-secs`, and one token source (`--glab`, `--gh`, `--token-env
<var>`, `--token-keyring <service>` or `--token-command <arg>...`; none means
`own = true`). It reads no token, fetches nothing and writes nothing: it prints the file
to stdout, and if a config file already exists it prints that file edited, with its
comments and other watches kept. When that file already has a primary watch, the new
watch is added as secondary with a note on stderr; `--primary` keeps it primary. A new
GitHub watch polls at 30 s live and 120 s idle unless `--live-secs` says otherwise.
Redirect it yourself once it reads right.

**`fixture record`** walks one parent pipeline and its children into a fixture directory
for the core's tests; `fixture scrub` re-applies the allow-list that keeps personal data
out of fixtures. Recording is GitLab-only: a github account is refused with exit 64. See
[CONTRIBUTING.md](CONTRIBUTING.md).

### Exit codes

Verdict codes and error codes never overlap, so a script can tell a red pipeline from a
typo in its own command line. For `check` and `watch` the code is the tray icon's state,
that is the worst state among the primary watches.

`--watch <id>` makes the watches you name the subject, so when none of them is primary
the code is the worst state among **all** of them: `check --watch hourly` on a failed
schedule exits 1, not 4. Name a primary watch alongside and the tray rule applies again,
because a secondary watch must not be able to outvote a primary one.

```
  0   check/watch: deployed or succeeded_no_deploy; any other command: success
  1   check/watch: failed; fixture scrub --check: a fixture would change
  2   check/watch: deployed_with_failure
  3   check/watch: running, parked_gate or canceled
  4   check/watch: unknown (nothing matched, or a request failed; errors on stderr)
  64  usage error: bad arguments, or --watch names no configured watch
  70  any other error before a verdict (token, fixture, recording, I/O)
  78  the configuration is missing, unreadable or invalid
```

## Build from source

Requirements: Rust 1.88 or newer (via [rustup](https://rustup.rs); the repository pins
the stable channel), Node.js 24 (CI's version; `package.json` also accepts 20.19+ and
22.12+), and the platform toolchain.

**macOS:** Xcode Command Line Tools (`xcode-select --install`).

**Ubuntu 22.04 or newer:**

```
sudo apt-get update
sudo apt-get install -y \
  build-essential curl file \
  libwebkit2gtk-4.1-dev \
  libjavascriptcoregtk-4.1-dev \
  libsoup-3.0-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf \
  xdg-utils \
  libsecret-tools
```

`libwebkit2gtk-4.1` (not 4.0) is the Tauri 2 line and `libayatana-appindicator3` puts the
icon in the tray; without the latter the app runs with no icon and no window, which looks
like a crash. `libsecret-tools` is a runtime dependency (`secret-tool`), not a build one.
No OpenSSL is needed: HTTPS is rustls.

Then:

```
git clone https://github.com/distronode-corporation/bridgewatch
cd bridgewatch
npm ci
npm run tauri build
```

Bundles land in `target/release/bundle/` at the repository root. For development, see
[CONTRIBUTING.md](CONTRIBUTING.md).

## Limitations

- **Polling, not push.** See [the job view](#the-job-view-and-what-live-means).
- **Glob and regex refs, and more than one source, are filtered client-side over one
  page.** GitLab's pipelines API filters `ref` by exact name and `source` by one value,
  and GitHub's runs API filters `branch` and `event` the same way, so those watches read
  the newest page for the project (4 × `show.max_rows` pipelines or runs, between 30 and
  100) and filter it. A matching pipeline older than that page is not seen. On GitHub a
  page without `workflow` holds every workflow's runs, so it covers less time.
- **Notifications cannot be clicked through on desktop.** The Tauri notification plugin
  passes only title, body, icon and sound to macOS and Linux, so `notify.click` has no
  effect yet. The popover row carries the same link.
- **No poll on wake from sleep.** The first data after resume can be up to one interval
  stale; opening the popover polls at once.
- **Windows is not targeted.** Nothing is tested there, the keyring code least of all.
- **No merge-request pairing.** `merge_request_event` works as a source like any other,
  but a merged-result pipeline is not paired with its branch pipeline.
- **The wizard's deploy-marker suggestions look one level deep** (parent and direct
  children).
- **The wizard does not write a GitHub commit group.** It writes one row per run; set
  `group = "commit"` and `expect` in Settings or by hand.
- **A GitHub commit group is formed by time.** A run that starts more than
  `fan_out_secs` after the rest of its push (a slow `workflow_dispatch`, a re-push of the
  same commit) is a row of its own. See [`expect`](#expect-a-workflow-that-never-started).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The inner loop is `cargo test -p
bridgewatch-core`, which runs against recorded fixtures and scripted responses with no
network and no token.

## Security

See [SECURITY.md](SECURITY.md). Report vulnerabilities privately through
[GitHub's private vulnerability reporting](https://github.com/distronode-corporation/bridgewatch/security/advisories/new),
not in a public issue.

What guards the code and the release path, on every push to `main` unless the item says
otherwise:

- CodeQL (Rust, TypeScript, Actions) and [zizmor](https://docs.zizmor.sh) over the
  workflows, both reporting to code scanning.
- `cargo deny` against [deny.toml](deny.toml): RustSec advisories, a permissive-only
  licence allow-list, crates.io as the only source. Tolerated advisories are listed
  there with their reason.
- A weekly re-run of `cargo deny` and of `npm audit --audit-level=high`, because an
  advisory lands against code that did not change and so no push is coming to catch it.
  The npm side audits the whole tree, dev dependencies included: this is a Vite app, so
  they are what gets bundled into the shipped frontend.
- Dependency review on every pull request, Dependabot for Cargo, npm and the pinned
  action SHAs, and a weekly [OpenSSF Scorecard](https://scorecard.dev/viewer/?uri=github.com/distronode-corporation/bridgewatch).
- Dependabot's patch and minor updates merge themselves, but only once the eight checks
  the `main` ruleset requires have passed. A major waits for a person.
- Release builds run in a `release` environment that only a `v*` tag can reach and that
  holds the macOS signing identity as its own environment secrets, so no branch and no
  pull request can read it. Release tags cannot be moved or deleted.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
