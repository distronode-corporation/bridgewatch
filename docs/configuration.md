# Configuration

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
key without a control fails CI. Settings has five panes in a sidebar: **Accounts**,
**Watches**, **General** (the icon and the app), **Verdict**, and **Advanced** (the log,
and **Edit as text**, which edits the same file directly with diagnostics by line and
column). It opens on the pane you used last.

`bridgewatch config validate` reports every problem with its line and column.
`bridgewatch config schema` prints the JSON Schema (the same one committed as
`src/lib/config.schema.json`) for editor completion. `bridgewatch config dump` prints the
configuration with every default filled in.

## Reference

Every key is optional unless marked required. Defaults in brackets.

**`[accounts.<name>]`**, one per GitLab instance or GitHub host (at least one is required).
Accounts of both providers can sit in one file:

| Key | Meaning |
| --- | --- |
| `provider` | `gitlab` or `github` [`gitlab`, so a file written before GitHub support means what it always did]. `base_url`, `api_path` and `header` default per provider. |
| `base_url` | Instance root, no trailing slash [`https://gitlab.com`, or `https://api.github.com` for github]. For GitHub Enterprise Server, the server's root. `http://` is accepted, with a warning unless the host is loopback. |
| `api_path` | API prefix [`/api/v4`, or empty for github]. GitHub Enterprise Server needs `/api/v3`. |
| `token` | Token source, see [Tokens](tokens.md) [`{ own = true }`]. |
| `header` | `"PRIVATE-TOKEN"` (GitLab personal and project access tokens) or `"Authorization: Bearer"` (GitLab OAuth and CI job tokens, and every GitHub token) [`PRIVATE-TOKEN`, or `Authorization: Bearer` for github, where `PRIVATE-TOKEN` is an error]. |
| `timeout_secs` | Per-request timeout [15]. |
| `rate_limit_backoff.max_secs` | Ceiling for the doubled interval after rate limiting or errors [300]. A `retry-after`, or GitHub's rate-limit reset time, is waited out in full even when it is longer. |
| `actions` | Allow retrying jobs and starting manual ones from the popover and the CLI [false]. The token then needs more than read access, see [Retry and play](retry-and-play.md). |

**`[[watches]]`**, in display order:

| Key | Meaning |
| --- | --- |
| `id` | Required. Unique; used by `--watch`, notifications and the GUI. |
| `account` | Required. An `[accounts.<name>]` key. |
| `project` | Required. GitLab: numeric id or `group/path`. GitHub: `owner/repo` (a numeric id is an error). |
| `ref` | Exact name, glob (`pf/*`), or `re:` plus a regex [`main`]. A `re:` pattern is unanchored; write `re:^…$` to match the whole ref. |
| `sources` | Pipeline sources (GitLab: `push`, `schedule`, `web`, `merge_request_event`, ...) or workflow events (GitHub: `push`, `schedule`, `pull_request`, `workflow_dispatch`, ...) to accept. Empty means all [`[]`]. |
| `workflow` | GitHub only. One workflow, by file name (`ci.yml`) or numeric id; each run is a row. Absent means every workflow [unset]. |
| `group` | GitHub only. `run` (one workflow run is one row) or `commit` (every run of one push is one row, each run a bridge) [`run`]. See [GitHub Actions](github-actions.md). |
| `fan_out_secs` | GitHub only, with `group = "commit"`. Runs of one commit, event and branch created within this many seconds of the group's newest run are one row [90]. |
| `expect` | GitHub only, with `group = "commit"`. Workflow file names every group must hold; one with no run is a dead bridge once the group has settled and its window has passed. See [`expect`](github-actions.md#expect-a-workflow-that-never-started) [`[]`]. |
| `mine` | Only the pipelines or runs you started: the signed-in user, or the token's owner. GitLab is asked with `username=`, GitHub with `actor=`; who you are is asked once [false]. |
| `role` | `primary` (drives the icon, may notify) or `secondary` (rows only) [`primary`]. |
| `show.max_rows` | Most rows this watch shows [5]. |
| `show.settled` | Settled pipelines kept below the unsettled ones [1]. A `skipped` pipeline ran nothing, is not listed and does not count. |
| `show.jobs` | `all` or `failures`; overrides `[ui].jobs` for this watch [unset]. |
| `poll.live_secs` | Interval while anything is live [5; the wizard writes 30 for GitHub]. |
| `poll.idle_secs` | Interval when everything has settled [60; the wizard writes 120 for GitHub]. |
| `dive.bridges` | Glob over trigger-job names to walk into, or over workflow names on a GitHub commit group; `""` walks none [`*`]. |
| `dive.exclude` | Trigger-job (or workflow) name globs to skip [`[]`]. |
| `dive.depth` | Levels of child pipeline to walk [1]. GitLab only: GitHub runs do not nest. |
| `dive.only_when` | Walk only into bridges in this status, e.g. `"failed"` [unset]. |
| `deploy_markers` | Job names or `re:` patterns whose success means "deployed". With several, a success counts only once none of the others is still in flight; config order then decides which success is reported [`[]`]. |
| `sibling_failure` | What a failed or dead bridge other than the marker's does to a deploy: `downgrade`, `fail` or `ignore` [`downgrade`]. |
| `post_deploy_failure` | The same for a blocking failure after the marker, or in the marker's own pipeline [`downgrade`]. |
| `eta` | Learn how long this watch's deploys take and show it on a running row, see [Deploy ETA](app.md#deploy-eta) [true]. |
| `[watches.jobs]` | Job-name pattern (literal or `re:`) to `gate`, `warning`, `blocking` or `ignore`. File order, first match wins. Applies to bridges too. On GitHub it is the only way to tolerate a failure. |
| `notify.deployed`, `notify.blocking_failure`, `notify.finished` | Notify on these events [true]. A pipeline is reported failing once, see [Notifications and noise](notifications.md). |
| `notify.recovered` | "Fixed again": the watch's newest settled pipeline passed where the one before it failed. It replaces that pipeline's other notifications [true]. |
| `notify.started`, `notify.gate_opened` | Notify on these events [false]. |
| `notify.title`, `notify.body` | MiniJinja templates over the pipeline view (`watch.id`, `kind`, `sha7`, `state`, `failures`, `failure_line`, `warnings`, `gates`, `ref`, `source`, `url`, ...). The default title adds ", fixed again" when `kind` is `recovered`; the default body puts the first failing line (`failure_line`) under the failing job names. |
| `notify.click` | `first_failure_or_pipeline`, `pipeline`, `marker_job` or `none`: what clicking the notification opens. See [Failure alerts](notifications.md#failure-alerts) for which desktops report a click. |

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
| `popover.width`, `popover.max_height` | Popover size in logical pixels [440, 720]: always `width` wide, and as tall as its content up to `max_height`, growing and shrinking with it. A reload applies a new value at once. |
| `popover.hide_on_blur` | Hide the popover when it loses focus [true]. |
| `theme_css` | Path to a `.css` file (at most 256 KiB) layered over the built-in theme [`""`]. |
| `launch_at_login` | Register a login item. The checkbox in Settings and the tray menu set the OS login item and mirror its state here [false]. |
| `send_crash_reports` | Send a crash report when the app fails, see [Crash reports (opt in)](app.md#crash-reports-opt-in). Unset means not answered yet, and the popover asks once; only `true` sends [unset]. |
| `jobs` | `all` or `failures`, see [the job view](app.md#the-job-view-and-what-live-means) [`all`]. |
| `hotkey` | A global shortcut that shows or hides the popover, such as `CmdOrCtrl+Shift+B` (modifiers `CmdOrCtrl`, `Ctrl`, `Alt`, `Shift`, `Super`, joined to one key with `+`) [`""`, none]. A value that does not parse is logged and ignored. On Linux it works under X11 only, see [Limitations](limitations.md). |
| `commit_titles` | Show each pipeline row's commit title, its first line, in the popover [true]. GitHub's runs carry it; on GitLab it costs one request per new commit (`/repository/commits/:sha`), at most three per watch per poll, and each is asked once. The CLI never shows titles. |

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

## Walk-through of the examples

The GitLab example, [`examples/distronode.toml`](../examples/distronode.toml), is the
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

The GitHub example, [`examples/github.toml`](../examples/github.toml), follows this
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
