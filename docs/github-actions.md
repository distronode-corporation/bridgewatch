# GitHub Actions

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

## Quick start: the wizard

Pick **GitHub Actions** on the first step of the [setup wizard](setup.md#first-run-the-setup-wizard),
then **github.com** or **GitHub Enterprise**. On github.com, **Sign in with GitHub** is
already chosen: approve bridgewatch in your browser with the short code it shows, and the
sign-in is kept in the system keychain and renews itself. There is no token to create or
paste. If you have run `gh auth login`, **Use gh's token** is listed below it, and is the
one chosen where sign-in needs an application of your own (GitHub Enterprise). It writes
the keyring source for gh's item (see [Reusing gh's login](#reusing-ghs-login)) and never
copies the token. Then pick a repository (or type `owner/repo` or its URL), the events to
watch, and optionally one workflow file. A new GitHub watch polls every 30 s while live
and every 120 s when idle, because of the [rate limit](#rate-limits).

The command-line equivalent prints the same file without touching the network, then
signs in:

```
npx bridgewatch init --provider github --project owner/repo
npx bridgewatch auth login --account github
```

`init` writes nothing itself. Write what it prints to a new file, check it, then move it
over the file `bridgewatch config path` names, before signing in. Never redirect `init`
onto the config itself: the shell empties that file before `init` reads it, and every other
account, watch and comment is lost. When `init` writes a sign-in source, its last line on
stderr is the `auth login` command for that account. With no token flag, a new account
signs in on github.com and gitlab.com, and one the file already has keeps its source; a
token source never moves to another host, and `init` will not repoint an existing account
to one unless `--account` names it. `--gh`,
`--token-env` and the others choose another source, and `--oauth-client-id` names an
application of your own on GitHub Enterprise.

The wizard asks which form you want. **One row per push** writes `group = "commit"` and
offers `expect`: the workflow files the newest three pushes on the branch ran, read from
one page of runs, with the ones every push ran already ticked (leave out any whose `paths`
or `branches` filters can skip a push). Otherwise it writes a `group = "run"` watch: one
row per workflow run, of the one workflow you named or of every workflow. A re-run of the
wizard sets a commit group on an existing watch when you ask for one, and never takes one
away.

## A hand-written config for github.com

The shipped [`examples/github.toml`](../examples/github.toml) follows this repository's own
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

## Tokens for GitHub

A **classic** personal access token needs the `repo` scope to read a private repository,
and no scope for a public one. There is no narrower classic scope for reading Actions, and
`workflow` is not one: it grants write access to workflow files. A **fine-grained** token
needs **Actions: read** and **Metadata: read** on the repositories you watch.
**Test connection** in the wizard lists a classic token's scopes and warns when `repo` is
missing; GitHub reports no scopes for a fine-grained token, so the wizard says it cannot
check one. GitHub reads a token only from the `Authorization` header, which is
the default for a github account, and `header = "PRIVATE-TOKEN"` on one is an error.

### Reusing gh's login

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
[Command sources need confirmation](tokens.md#command-sources-need-confirmation).

## GitHub Enterprise Server

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

## Rate limits

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

An exhausted limit is a 403 or 429 carrying `x-ratelimit-remaining: 0` or `retry-after`, or
one whose message says it is a rate limit (GitHub's secondary limit can arrive with neither
header). bridgewatch treats it as a rate limit, not a bad token, and waits until GitHub's
reset time, the `retry-after`, or one minute when neither applies (a secondary limit with
budget left and no `retry-after`), before asking again, whatever
`rate_limit_backoff.max_secs` says.

## `expect`: a workflow that never started

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

## What GitHub does not have

- **No `allow_failure`.** `continue-on-error` does not appear in GitHub's jobs API, so
  every failed job counts as a failure. Use a [`[watches.jobs]`](configuration.md#reference) override to
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
