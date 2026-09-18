# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Release workflow note: `.github/workflows/release.yml` extracts the section for a tag out
of this file and uses it verbatim as the GitHub Release body. The extraction is
index-based on the `## [x.y.z]` heading and stops at the next such heading or at the first
line beginning with `[`, so keep the link definitions at the bottom and do not start a
body line with a bracket.

## [Unreleased]

## [0.1.0] - 2026-09-18

First release. GitLab only, macOS and Linux.

### GitHub support is coming

GitHub Actions is planned for 0.2 and is deliberately not in this release. bridgewatch's
value is walking a parent pipeline through its trigger jobs into the child pipelines.
GitHub has no parent/child pipelines: a push fans out into independent workflow runs,
related only by commit (`head_sha`) and event. Until bridgewatch has a commit-group
mapping that assembles those runs into one unit, a GitHub mode would be the same
per-workflow status dot other monitors already show.

### Added

- **Bridge-aware pipeline reading.** Walks a parent pipeline's trigger jobs (bridges)
  into their child pipelines and reads the child job lists, `dive.depth` levels deep,
  selected by `dive.bridges`, `dive.exclude` and `dive.only_when`. A bridge whose child
  was never created is reported as dead.
- **Source-aware watches.** Each watch filters by ref (exact, glob or `re:` regex) and by
  pipeline source, so pushes, schedules and preflight branches on one project are
  separate watches. Primary watches drive the tray icon and notifications; secondary
  watches only add rows. With several primary watches the worst state wins, and
  validation warns about it.
- **Deploy verdict from marker jobs.** `deploy_markers` names the jobs whose success
  means "deployed", found in the parent or any dived child. `sibling_failure` and
  `post_deploy_failure` (`downgrade`, `fail`, `ignore`) decide what a failure elsewhere
  does to a successful deploy. A marker that failed counts as a failure even when the job
  was allowed to fail.
- **Eight icon states**: `unknown`, `failed`, `deployed_with_failure`, `deployed`,
  `running`, `canceled`, `parked_gate`, `succeeded_no_deploy`. A pipeline whose job lists
  could not be read is `unknown`, never green. Monochrome template glyphs on macOS,
  coloured on Linux, remappable per state with `[icon].states`, or replaced from a theme
  directory of PNGs.
- **Per-job classes** through `[watches.jobs]` (literal or `re:` keys, first match wins):
  `gate`, `warning`, `blocking`, `ignore`. The table applies to bridges too. `gate` does
  not excuse a gated job that ran and failed.
- **Full job view in the popover.** Every pipeline row expands into the parent's jobs
  and one row per bridge with its child's jobs, grouped by stage in pipeline order, with
  a status dot, a live marker and a locally ticking duration per job. Expansion survives
  refreshes. `[ui].jobs` (`all` by default, or `failures`) and a per-watch
  `watches.show.jobs` choose what is listed. Jobs carry `started_at`, `finished_at` and
  `duration`.
- **Faster live polling.** `poll.live_secs` defaults to 5 s (idle stays 60 s). Opening
  the popover polls at once when the last poll is at least 2 s old, and the popover shows
  "updated Ns ago". GitLab offers no push channel to a desktop app, so this is polling;
  the per-tick request cost is documented in the README.
- **First-run setup wizard.** Opens when there is no config file at the default path:
  account and token (glab's keyring item detected when present, a pasted token, an
  environment variable or a command), a connection test that names the user and warns
  about project tokens and missing `read_api`, a project list or typed id/path/URL,
  branch and sources with optional schedule and preflight watches, suggested deploy
  markers, notifications and poll speed, and a review of the exact `config.toml`. Always
  skippable ("Skip, I'll edit config.toml" writes nothing). Re-openable from the tray
  menu and Settings, and on an existing file it edits rather than replaces.
- **`bridgewatch` CLI** (`crates/bridgewatch-cli`, built from source): `check` and
  `watch` (with `--watch`, `--json`, `--fixture`, and `--ticks` on `watch`), `init`
  (the wizard from flags: offline, prints and never writes), `config path`, `config
  validate`, `config schema`, `config dump`, `fixture record` and `fixture scrub`.
  `--config` is accepted before or after the subcommand.
- **Documented exit codes.** 0 deployed or green, 1 failed, 2 deployed with failure,
  3 running, parked or canceled, 4 unknown, 64 usage error, 70 other error, 78 bad
  configuration. Verdict codes and error codes never overlap.
- **`crates/bridgewatch-core`**, a Rust library with no Tauri dependency: config, token
  resolution, GitLab client, verdict engine, poller and notifications, tested against
  pipelines recorded from a real project and put through a personal-data allow-list.
- **Tauri 2 GUI.** Tray icon per state; popover with per-watch rows, the job view, an
  error strip and a Debug section (config path, last poll, request log); Settings with
  Accounts, Watches, Icon, Verdict, UI and Log tabs plus an "Edit as text" tab with
  diagnostics by line and column; tray menu with Show status, Open pipelines page,
  Refresh now, Reload config, Open config file, Settings, Setup wizard, Launch at login
  and Quit. No Dock icon on macOS. UI built on shadcn-svelte components (vendored) and
  Tailwind CSS 4, following the OS light or dark appearance, with a popover vibrancy
  material on macOS.
- **GUI and file parity.** Every config key has a Settings control and every control a
  key, enforced by a test against the generated JSON Schema.
- **Config editing that keeps your file.** Reads and writes go through `toml_edit`, so
  comments and key order survive a GUI save. The file is hot-reloaded; an invalid file
  leaves the last good configuration running and shows its errors. GUI saves are
  compare-and-swap, so an open Settings window cannot overwrite a change made on disk.
  Unknown keys and values that name nothing are warnings, not errors.
- **Credential sources**: an OS keyring item (including glab's, stored with an empty
  user name, with its `go-keyring` wrapper unwrapped), bridgewatch's own keyring entry
  (the default), an environment variable, or the first line of a command's output (no
  shell, stdin closed, 10 s timeout). A literal token in the config file is refused.
- **Security checks in the shell.** Writing a command token source, or sending a
  credential to a host it has not been sent to before, needs an explicit confirmation
  described by the Rust side. Every link opens through one Rust function that allows
  only the scheme, host and port of a configured account. The web views hold no plugin
  permission beyond events. HTTP redirects are refused so a token never follows one, and
  a plain `http://` base URL off loopback is warned about.
- **Launch at login** through the OS login item, reflected in the tray menu, Settings and
  the config file.
- **Logging**: `[log].level` applies to the app and is re-applied on reload; a non-empty
  `RUST_LOG` wins.
- **`[verdict].script`**: an optional sandboxed Rhai script (no file imports, no `eval`,
  capped operations and call depth) that replaces the built-in rules. A failing script
  yields `unknown`.
- **Resilience.** A failed token resolution or poller build retries with backoff (5 s
  up to 300 s) instead of stopping; a crashed poll loop is restarted and reported; a tray
  image that fails to draw falls back to a visible glyph and is re-asserted periodically.
- **`examples/distronode.toml`**, a complete worked configuration with three watches.
- **Release assets**: `.dmg` for Apple Silicon and Intel (signed with the Distronode
  Corporation Developer ID and notarised by Apple), `.deb` (depends on
  `libsecret-tools`) and `.AppImage` for x86_64 Linux, built on Ubuntu 22.04, each with a
  GitHub build provenance attestation.

### Known limitations

- Glob and regex refs, and watches with more than one source, are filtered client-side
  over the newest page of pipelines (30 to 100), because GitLab filters `ref` by exact
  name only.
- Desktop notifications cannot open a URL when clicked (the Tauri notification plugin
  passes only title, body, icon and sound), so `notify.click` has no effect yet.
- No poll on wake from sleep; opening the popover polls at once.
- The CLI is not included in the release assets.
- The wizard's deploy-marker suggestions look at the parent and direct children only.
- Reading a glab keyring item for a self-managed instance follows glab's
  `glab:<host>:token` naming, which has not been checked against a live install.
- Windows is not targeted.

[Unreleased]: https://github.com/distronode-corporation/bridgewatch/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/distronode-corporation/bridgewatch/releases/tag/v0.1.0
