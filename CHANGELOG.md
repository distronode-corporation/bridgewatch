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

## [2.0.0] - 2026-11-07

bridgewatch 2.0 signs in instead of asking for a token, keeps every stored token with the
server it was saved for, and says more with less noise. A new github.com or gitlab.com
account starts on **Sign in**, and one-click setup then offers your most recently active
projects with the top three ticked. `bridgewatch wait` blocks until a commit's pipeline has
a verdict and exits with its code, for scripts and coding agents. A failure notification
names the first failing line and opens the failed job when clicked; a failing pipeline is
announced once, a burst arrives as one, and a fix says "fixed again". `mine = true` shows
only your own pipelines, and token health warns before a token or sign-in expires. The
popover, Settings and the setup wizard have new layouts. Upgrading from 1.x: read
Breaking first, and
[Upgrading from 1.x](https://github.com/distronode-corporation/bridgewatch/blob/main/docs/tokens.md#upgrading-from-1x).

### Breaking

- **Own tokens are bound to their server, and a changed `base_url` needs a new token.**
  A token saved for an account (`token = { own = true }`, from Settings or the wizard) is
  now stored with the server it was saved for and is sent to that server only. Before,
  changing an account's `base_url` (by hand, or with `bridgewatch init --account`) sent
  the stored token to the new host on the next poll. Now it is not sent: `check`, `watch`,
  `wait`, `log`, `retry` and `play` exit 70 and say which server the token belongs to, and
  in the app only that account's watches show the error.
- **1.x entries bind on first use.** A token stored by bridgewatch 1.x is tied to the
  server the account names the first time 2.0 uses it, before anything is sent. If that
  write fails, nothing is sent. The CLI binds too.
- **1.3 cannot read 2.0 entries, and refuses them without sending.** After a downgrade,
  paste the token again in 1.3's Settings; 2.0 binds it again later.
- **Sign-in is the default for new github.com and gitlab.com accounts** (see Changed).
  `bridgewatch init` with no token flag writes `token = { oauth = true }` for a new
  account there, where 1.x wrote `own = true`, and an account the file already has keeps
  its source instead of being rewritten to `own = true`.
- **`bridgewatch init` refuses to repoint an existing account to another host** unless
  `--account` names it (see Fixed). A script that relied on the derived account name
  being rewritten now gets an error.
- **`bridgewatch auth status` lists own-token accounts and exits 70 unless every account
  it lists is usable.** In 1.x it spoke only of sign-ins, so a file with an own token that
  is missing, or saved for another server, now makes it exit 70.
- **A failing pipeline is announced once.** "finished: failed" is no longer sent after
  its blocking-failure notice, and more jobs failing in the same pipeline say nothing
  more.
- **"Fixed again" replaces a push's other notifications** (`notify.recovered`, on by
  default): the first passing push after a failure sends one "fixed again" notification
  in place of its deploy and finish notifications. On a failing watch, a deploy is
  announced when its pipeline settles, not while post-deploy jobs still run. Set
  `notify.recovered = false` on a watch to keep 1.x's deploy and finish notifications
  for it.
- **More than three notifications in one poll arrive as one** in the app, titled
  "bridgewatch: N updates". `bridgewatch watch` still prints every one.
- **The default notification body and title change.** The body adds the first failing
  line as a second line, and the title ends in ", fixed again" for the new kind. A custom
  `notify.title` or `notify.body` is unchanged. `bridgewatch watch` prints the body's lines
  as one, joined by `; `, so a script matching its text output sees the longer line.
- **`bridgewatch watch` can print new notification kinds**, `recovered` and `token`, in
  its text lines and in `watch --json`.
- **Settings has five panes instead of seven tabs**, and the popover's **Debug**,
  **Settings…** and **Quit** moved to a footer (see Changed). Nothing in the file moves.
- **The `bridgewatch-core` API changes.** `token::set_own_token` takes the server's
  `base_url`, `token::TokenError` has a new `BoundElsewhere` variant and `ClientError` a
  new `Token` variant, and a stored own entry is a small document rather than the bare
  token, so a script that reads the keychain item sees the document.

### Added

- **`bridgewatch init --oauth` and `--oauth-client-id <id>`.** The second names an
  application of your own, as GitHub Enterprise Server and a self-managed GitLab need.
  When the file signs in, `init` ends by printing the next step,
  `bridgewatch auth login --account <name>`.
- **`bridgewatch wait`.** It waits until the pipeline for one commit has a final verdict,
  then exits with the same codes as `check`. `--sha` defaults to the current directory's
  `HEAD`. `--watch` can be left out when only one watch is configured, or only one is
  primary. `--timeout 10m` gives up and exits 124. `--json` prints the verdict as one
  object. A verdict is final only once nothing is in flight, because a post-deploy failure
  can still downgrade a deploy. A parked gate is final too, because only a person moves it.
  A commit that is not among the watch's rows is looked up directly by its sha. A skipped
  pipeline exits 4 at once, and an older pipeline is judged like any row. A commit with no
  pipeline yet is waited for, up to `--no-pipeline-grace` (10 minutes), then exits 4. A
  401, 403 or 404 exits 4 at once, the code `check` gives it, naming the account and the
  status. A 429, a 5xx or a dropped connection is retried on the usual backoff. Both
  durations are at most 365 days; a longer one is a usage error.
- **A "fixed again" notification** (`notify.recovered`, on by default). When a watch that
  was failing passes again, you get one notification that says so, in place of that
  push's deploy and finish notifications. While a watch is failing, its next deploy is
  announced when the pipeline settles, not while post-deploy jobs still run. While a newer
  pipeline is still running, a new push or a failed one being retried, an older green
  pipeline does not count as the fix; a failure that settles meanwhile still counts.
- **More than three notifications at once arrive as one.** It lists the first three
  titles, then "and N more"; the popover has the rest.
- **Pause notifications** in the tray menu, for one hour or until resumed. While notifications
  are paused, the popover says so and offers **Resume**. A pause lasts until the app quits.
- **Clicking a failure notification opens the failed job's page** (`notify.click`, which
  was computed but never delivered). macOS reports the click, and so does a Linux desktop
  whose notification server offers actions; Windows only while the toast is on screen,
  not from the Action Center. The link goes through the same host check as every other
  link the app opens.
- **The first failing line.** A failed job says why: GitLab's failure reason in words
  ("script failure"), or on GitHub the first failure annotation of the job's check run
  (`src/parser.rs:142: assertion failed`). It is the second line of a failure
  notification (`{{failure_line}}` in templates), and it is shown under the failed job in
  the popover. On GitHub it costs one request per failed run, for its first blocking
  failure under `[watches.jobs]`, and nothing for a passing run or on later polls. A fine-grained token needs Checks: read
  for it; without it the line is left out and nothing else changes.
- **Copy failure as Markdown** on a failed job in the popover: the job, where it ran, the
  first failing line in a code block, and the link.
- **My builds: `mine = true` on a watch** shows only the pipelines (GitLab) or workflow
  runs (GitHub) you started. GitLab is asked with `username=`, GitHub with `actor=`. You
  are the signed-in user, or for a token the token's owner, asked once with `GET /user`
  and remembered (a sign-in that names its user needs no request). The Settings toggle,
  "Only my pipelines", is in the watch's Source group.
- **Token health.** bridgewatch tells an expiring token from an expired, a revoked and an
  invalid one, and warns 72 hours and again 24 hours before an expiry, each warning once
  as a notification, and in a strip in the popover for as long as it is true. The expiry
  is GitLab's `expires_at` (`GET /personal_access_tokens/self`, asked once per token
  each time a poller starts), GitHub's `github-authentication-token-expiration` header
  (on every response, so no request of its own), or a sign-in's refresh-token expiry
  (the access token's for a sign-in that cannot renew). GitLab's 401 says whether a
  token was revoked or has expired; GitHub's does not, so a GitHub 401 reads as expired
  only past a known expiry. `bridgewatch auth status` shows each bound own token's
  expiry (one expiry lookup each) and says when a sign-in ends within 72 hours.
- **One-click setup in the wizard.** Once a sign-in or **Test connection** succeeds, the
  Account step lists your ten most recently active projects (GitLab `last_activity_at`) or
  repositories (GitHub `pushed_at`), newest first, with an optional "active 3h ago" label,
  and ticks the top three. **Watch N projects** gives each ticked one the best deploy
  marker the deploy step's ranking finds, then goes to Preferences and Review; Finish
  writes every watch in one atomic write, or none. The list is one request per account.
  Each watch is bound to the account that listed it: a list from another server is
  refused. Running it again adds no watch the file already has for that project and
  branch. The first watch added is primary unless the file already has one; the others
  are secondary. Choose **Next** instead to set up one project step by step.

### Changed

- **The default notification body** adds the first failing line on a second line when
  there is one. A custom `notify.body` is unchanged. `bridgewatch watch` prints the two
  lines as one, joined by `; `.
- **A failing pipeline is announced once.** The blocking-failure notice is now kept per
  pipeline, and "finished: failed" is not sent after it. The ledger an older version
  wrote is read as before, so an upgrade does not repeat a notice.
- **The default notification title** ends in ", fixed again" for the new kind. A custom
  `notify.title` is unchanged.

- **Sign-in is the default way to add an account.** On github.com and gitlab.com, where
  bridgewatch has its own application, **Sign in with GitHub** (or GitLab) is now what a
  new account starts on everywhere:
  - The setup wizard preselects it on a fresh start even when a gh or glab token was
    found. **Use gh's token** (or glab's) is still listed below it. The CLI's token is
    preselected only where sign-in needs a client id of your own, as on GitHub
    Enterprise Server or a self-managed GitLab.
  - Until now the preselection depended on which answer arrived first, so a found gh
    token could win over sign-in from one launch to the next. It is now decided from
    both answers, and never changes a choice you have already made.
  - Settings' **Add account** writes `token = { oauth = true }` for a new github.com or
    gitlab.com account, instead of bridgewatch's own entry.
  - `bridgewatch init` with no token flag writes `token = { oauth = true }` there for a
    new account, and `own = true` on any other host. An account the file already has
    keeps its token source on the same instance, as a re-run of the wizard does; until
    now a flagless `init` rewrote it to `own = true`. A kept source is not re-checked as
    though it had just been typed, an unknown key elsewhere in the file does not hide the
    account, and `--oauth` alone keeps an existing sign-in's client id.
  - In the wizard, the **Sign in** button sits directly under its option, before the
    alternatives. The section is called **Access**, and what a token must be allowed to do
    is said only while a token option is chosen.
- **The popover has a layout system.** Spacing sits on a 4px grid assigned by role, and
  the app uses four type sizes (15, 13, 12 and 11px) instead of seven. Small buttons are
  13px.
  - **The header is one line:** the state, a dot and how long ago the last poll was, with
    **Refresh** and **Pipelines** as icon buttons. **Debug**, **Settings…** and **Quit**
    moved to a footer.
  - **Each pipeline row has two lines.** The first is the ref and its age, and the second
    is the commit, what started it, the deploy job (now labelled "deploy job build") and
    the time estimate. The verdict is said once: the row's state shows only where the
    watch header doesn't already say it, and the deploy pill only where it adds
    something, so "deployed deployed" is gone.
  - **Bridge and job rows are in columns.** The name truncates, and the status and job
    count line up on the right. The trigger job, child pipeline and workflow run links
    are icons, each with the name a screen reader reads. The name keeps at least 12
    characters: a long status gives way first, and in a narrow popover "passed, with
    warnings" reads "warnings", with the full words in its tooltip and accessible name.
    A dead bridge says why in its status ("no child", "never started").
  - **Job tools are 12px words.** **log**, **retry** and **play** show on hover or
    keyboard focus, and always on a failed job.
  - **Watches are separated by space** instead of a tinted band and a rule.

- **The setup wizard's layout.** The access methods are cards, and the chosen method's
  controls (the sign-in button, the token field) sit inside its card instead of after
  the whole list. They stay one radio group: the arrow keys move the choice, and Tab
  goes from the chosen method into its field. The stepper labels only the current
  step, so no label is cut short ("Acco…", "Proj…") at the narrowest window; every step
  keeps its name for screen readers. Text is 13px throughout, with 15px step titles and
  12px hints, and the column is at most 560px wide. Fields that had a fixed width now
  shrink with the window.

- **Settings has a sidebar.** Five panes replace the seven tabs: **Accounts**,
  **Watches**, **General** (the icon and the app), **Verdict**, and **Advanced** (the log,
  and **Edit as text**). Settings opens on the pane you used last. The arrow keys, Home
  and End move between panes. The file's path, **Open file** and **Reload** are in a bar
  at the bottom, and **Setup wizard…** is at the foot of the sidebar.
  - **Every pane is grouped into cards** of rows, at most 600px wide: the name and its
    hint on the left, and the control on the right, or underneath when it needs the
    width. Numbers and menus are as wide as what they hold, not the whole window. Hints
    wrap at about 60 characters.
  - **An account starts with how it signs in**, then its server, then an **Advanced**
    group (API path, auth header, timeout, backoff), folded away. The chosen token
    source's details, including the "Stored for" line, sit in one panel, with its buttons
    in one row under it.
  - **A watch is split into groups** named after its settings: Source, Rows, Polling,
    Child pipelines, Deploy, Jobs and Notifications. Only Source starts open. The
    notification settings lose their "Notify:" prefix. **Add watch** is in the Watches
    pane's header.
- **Shared controls are 13px**, in every window. The setup wizard looks the same as
  before; its confirmation dialog, which did not get the wizard's 13px, now does.

### Fixed

- **A token problem stops only its own account in the app.** One account whose token is
  missing or bound to another server used to stop polling for every account, with the
  tray icon on unknown and "cannot start polling" in the strip. Now that account's watches
  carry the error and the others keep polling, as an account that is not signed in
  always did. The refused account is tried again on its own, in place, at 5 seconds and
  then at doubling intervals up to 5 minutes, and at once after a token is saved for it or
  on **Refresh now**: a locked keychain or a token command that timed out heals without a
  reload, and the other accounts keep their state throughout.
- **`bridgewatch auth status` lists own-token accounts too**: bound to which server, saved
  for another server, a 1.x value that binds on first use, or nothing stored. It exits 70
  unless every account it lists is usable.
- **A sign-in whose stored server, or whose account's `base_url`, is not an http(s) URL is
  never used.** Two such addresses used to count as the same server.
- **Settings says which server an own token is for.** The token field reads "Stored for
  X" and, when the account has since moved, "Stored for X; paste one for Y".
- **The setup wizard forgets a stored token when the instance changes**, as it does a
  sign-in. On a re-run it asks which server the stored token is bound to, so a config
  edited by hand to another host asks for a token too. The confirmation for moving an
  account to another host says an own token or a sign-in stays with the old host (it used
  to say the token would be sent).

- **`bridgewatch init` never moves a token source to another host.** The derived account
  name is the host's first label, so gitlab.com and gitlab.acme.com are both `gitlab`,
  and `init` for the second host rewrote the first account's `base_url` while keeping its
  token source. `init` now refuses to repoint an existing account to another host unless
  `--account` names it, and a source is kept only on the same instance.
- **`init` no longer suggests redirecting onto the config.** Its help, its last line and
  the README said to redirect the output, and a `>` onto the config empties it before
  `init` reads it. They now say to write a new file, check it and move it into place.
  The printed `auth login` step carries `--config` when it was given.
- **The wizard never uses the previous instance's CLI token.** After the instance changed,
  the old host's glab or gh token stayed preselected until the new detection answered, so
  **Test connection** in that gap could send it to the new host. A detected token is now
  tied to the instance it was found for.
- **Settings: signing in through a newly typed client id writes it.** A successful
  sign-in already switched an account to `oauth`. When the account signed in already but
  through another application, the new client id stayed unwritten until **Use this
  source** was pressed. That button is now hidden for the sign-in the file already uses.
  What is written is the client id the sign-in started with, even if the box was edited
  while the code was pending, and "Signed in as" is shown once, not twice.
- **The wizard counts a sign-in only for what it was made with.** A sign-in that finished
  after the client id or the instance changed still counted, and so did a stored one after
  either was edited on a re-run, so the file could name an application or an instance its
  tokens never came from, and the first poll asked to sign in again. The wizard now asks
  for a new sign-in instead.
- **A renamed account keeps its sign-in in the wizard.** A sign-in made after renaming the
  account, on a re-run or after an earlier sign-in, was stored under the old name, and one
  that finished after a rename while the code was pending was recorded under the new
  name, so the saved account had no sign-in either way. A sign-in is now stored under the
  name the form shows when it starts, and Finish moves it to the saved name when the
  account was renamed afterwards.
- **The wizard never keeps a hidden CLI token selected.** When the CLI's token had been
  picked and the instance changed to one where none is found, the option disappeared but
  stayed chosen, and **Next** could not go on. It now falls back to sign-in where the
  instance offers it, else to a pasted token.
- **`init` quotes the account name in the `auth login` step it prints**, so a name with
  a space can be pasted as it is.
- **The popover shrinks again when its content does.** It grew to fit new rows but never
  came back down, so after a watch was collapsed or its pipelines went, the window kept
  the old height with empty space below the last row. It is now sized to what the list
  holds, up to `ui.popover.max_height`, in both directions.
- **Settings: every tab label stays on one line.** At the narrowest window, **Edit as
  text** wrapped onto three lines.
- **A sign-in is sent only to the server that issued it.** `bridgewatch fixture record`
  read an account's stored sign-in and sent its access token to whatever `base_url` the
  file named at the time, even a server the sign-in was never issued by, and sent it as
  `PRIVATE-TOKEN`, which GitLab refuses for an OAuth token. It now records through the
  same client the poller uses: the token goes as `Authorization: Bearer`, and a sign-in
  issued by another server, or for another application, sends nothing and says to sign
  in again.
- **`auth status` and Settings no longer call an unusable sign-in "signed in".** A stored
  sign-in issued by another server than the account's `base_url`, or for another
  application than its client id, is never used to poll. `auth status` now prints
  `signed in for <server>, not <server>; run bridgewatch auth login --account <name>` for
  it and exits 70, and Settings says the sign-in is not used and offers to sign in again.
- **A token stored by bridgewatch 1.x stays with its old host when the account moves.**
  A 1.x token is tied to a server only when it is first used, so moving the account to
  another host before that tied it to the new host and sent it there, though the
  confirmation said it stayed behind. Now Settings, the setup wizard, `bridgewatch init
  --account` and a reload of a file edited in another editor tie it to the old host
  before the move. If the credential store is locked or cannot be read, the move is
  refused with its error and nothing is sent. A file edited while the app is not running
  is not seen as a move: the token is tied to the host the file names when it is first
  used, as before.

## [1.3.0] - 2026-11-06

The CLI ships with every release: on npm as `bridgewatch`, and as three tarballs on the
release, signed and notarised on macOS. The `.dmg` is now notarised and stapled itself,
and the app is on Homebrew.

### Added

- **The CLI ships with every release.** `bridgewatch` is on npm: `npm install -g
  bridgewatch`, or `npx bridgewatch` to run it without installing, on macOS and on Linux
  with glibc 2.35 or newer, x64 and arm64. The package carries the native binary for each
  platform and a small launcher (Node.js 18 or newer) that refuses musl or an older glibc
  with a pointer to the GitHub downloads. The release also carries the CLI as three
  tarballs, `bridgewatch-cli_<version>_<target>.tar.gz` for `universal-apple-darwin`,
  `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, each holding the binary,
  `LICENSE`, `NOTICE` and `THIRD-PARTY-LICENSES-cli.txt`. The macOS binary is signed with
  the Developer ID and notarised. The npm package is built from those same tarballs and
  published with npm provenance, and the tarballs are covered by the release's provenance
  attestation, so a release now carries ten assets (was seven). `cargo install --locked
  --path crates/bridgewatch-cli` still builds it from source.
- **The app is on Homebrew.** `brew install --cask distronode-corporation/tap/bridgewatch`
  installs the release's `.dmg` on macOS.

### Changed

- **The `.dmg` is notarised and stapled itself**, not only the `.app` inside it, so
  Gatekeeper can check the download offline before it is opened.

### Fixed

- **`bridgewatch watch` stops on a Ctrl-C that arrives while it is polling.** One
  during the first poll used to kill the process before it had a verdict, and one
  during a later poll was dropped, so `watch` ran on. It now exits with the last
  verdict's code, or 4 (unknown) when there is none yet.
- **`{{ref}}` renders the ref name in notification templates.** It always rendered empty:
  `ref` is a Rust keyword, and the template context was built with `r#ref`, which MiniJinja
  keys as `r#ref`, so the documented `ref` variable was undefined. It now holds the same
  value as `{{pipeline.ref}}`, which was unaffected. A template that used `{{ref}}` and
  relied on it being empty will now show the ref.

## [1.2.0] - 2026-10-25

Crash reports that stay off until you say yes, the licence texts in every bundle, the
provenance attestation attached to the release, Tauri 2.12 (building from source now
needs Rust 1.90) and MiniJinja 3, and fixes for a pipeline that failed before its first
job reading green and for a quiet watch repeating a notification on every tick.

### Added

- **Opt-in crash reports.** The release builds can send a crash report to Distronode's
  Sentry organisation in the European Union (de.sentry.io) when the tray app panics, and only after the user says yes: the popover
  asks once, defaulting to **Don't send**, and **Settings → UI → Send crash reports**
  (`[ui].send_crash_reports`, unset until answered) changes the answer, taking effect at
  once. With no yes, no crash-report client is created. Home-folder names are replaced
  with `<user>` in every path in a report, and the user, IP and host name are dropped.
  The address is compiled in by the release workflow only (`BRIDGEWATCH_SENTRY_DSN`), so
  a build you compile yourself, or a fork's, never sends or asks. The release
  workflow also uploads the macOS `.dSYM`s and the Linux binaries to Sentry and records
  the release's commits, and the Linux binaries now keep their symbol table so a report
  names its functions. `sentry` 0.49.3 on rustls (no OpenSSL). Proved against a local
  stand-in server by `src-tauri/tests/crash_reports.rs`.
- **Licence files in every bundle.** The `.dmg`, `.deb`, `.rpm` and `.AppImage` now carry
  `LICENSE`, `NOTICE` and the licence texts of the third-party code built into them:
  `THIRD-PARTY-LICENSES-rust.txt`, generated by cargo-about from `Cargo.lock` under the
  same allow-list as `deny.toml`, and `THIRD-PARTY-LICENSES-npm.txt`, generated from the
  npm packages the frontend build actually keeps. On Linux they are in
  `/usr/share/doc/bridgewatch/` (`.deb`) or `/usr/share/licenses/bridgewatch/` (`.rpm`).
  Until now the packages shipped the binary, the icons and a `.desktop` file only.
- **The provenance attestation is attached to the release** as
  `bridgewatch_<version>.intoto.jsonl`, so `gh attestation verify --bundle` works
  offline.
- `.github/SUPPORT.md`, and a "Licence of contributions" section in `CONTRIBUTING.md`.
- `osv-scanner.toml`, mirroring the eight advisories `deny.toml` tolerates (same ids,
  same reasons), so OpenSSF Scorecard's Vulnerabilities check, which reads OSV and not
  `deny.toml`, honours them too.

### Changed

- **Tauri 2.12 on the Rust side too**, matching `@tauri-apps/api` and `@tauri-apps/cli`
  2.12: tauri 2.12.2 and tauri-build 2.7.1, with the plugins autostart 2.7.0,
  notification 2.5.1, opener 2.7.0 and positioner 2.4.0. Tauri 2.12 needs Rust 1.90, so
  building from source now needs Rust 1.90 or newer (was 1.88).
- **MiniJinja 3 renders the notification templates** (was 2.24). The shipped title and
  body render exactly as before. One thing a custom template can see differently: a list
  printed whole, such as `{{failures}}`, now uses Python-style quotes (`['a', 'b']`, was
  `["a", "b"]`); `{{failures | join(', ')}}` is unchanged and is the better spelling.
- The frontend build's source-map-js is 1.2.2 (GHSA-68fv-2mgg-jv7q, a denial of service
  through indexed source-map section offsets; build time only).
- The `.deb`'s Maintainer is `Distronode <opensource@distronode.com>`, and the `.deb` and
  `.rpm` name <https://www.distronode.com/open-source/bridgewatch> as their homepage.
- The example config and the test fixtures use a placeholder GitLab project id,
  `12345678`, and `scripts/record-fixture.sh` now requires the project argument instead
  of defaulting to one.
- The bug report form asks for the provider and host (GitHub.com, GitHub Enterprise
  Server, GitLab.com or self-managed GitLab) and the Linux architecture.
- `bridgewatch watch --json` prints each notification as a JSON line,
  `{"notify": {...}}`, between the snapshots. It used to print plain text there, which
  broke `| jq`.
- Usage mistakes exit 64: an unknown `--account` in `fixture record`, and a
  `fixture scrub` directory holding no recordings (which used to pass `--check`
  having read nothing).
- `init --token-command` takes everything after it, hyphenated arguments included, so
  `--token-command gh auth token --hostname ghe.example.com` works. Put it last.
- `scripts/record-fixture.sh` scrubs through `bridgewatch fixture scrub`, so the Rust
  allow-list is the only one (it now needs `cargo`), takes the project id from the
  recorded pipeline, and refuses a non-numeric pipeline id before any request.
- A host written with a default port or in upper case (`https://GitLab.com:443`) is one
  host everywhere: the CLI keyring item is `glab:gitlab.com:token`, and GitHub's next
  page is followed. Before, each place parsed the URL its own way.
- The notification ledger is written only when it changed, not on every tick, and the
  ledger and `eta.json` keep a symlink and the file's permissions when saved.
- `tauri.conf.json` no longer carries a version; Tauri reads `Cargo.toml`'s.

### Fixed

- **A pipeline that failed before it had a job read green.** A `.gitlab-ci.yml` that
  does not parse, or a GitHub run that ended `startup_failure` or `timed_out` with no
  jobs, showed the outline check and `check` exited 0. It is now a blocking failure,
  `pipeline failed`.
- **One quiet watch could repeat a notification on every tick.** Once the ledger held
  2000 keys, trimming dropped the smallest pipeline id, which next to GitHub's run ids
  was a quiet GitLab project's newest row: the key just recorded. Trimming now spares
  every pipeline still on screen.
- A GitHub secondary rate limit that arrived as a 403 with no `retry-after` read as
  "not authorised, check the token" and was retried within seconds. It is a rate limit,
  and waits a minute.
- **Forget** in Settings left the forgotten token in use until a restart; the account
  now stops polling. Saving or forgetting a token no longer freezes the tray while a
  Keychain prompt waits.
- A sign-in closed or double-clicked while starting went on polling the provider for up
  to 15 minutes.
- Opening the popover ran two polls back to back instead of one.
- An open job log was fetched again, and blanked, on every tick.
- A token command with a quoted argument (`sh -c "pass show …"`) was split on spaces
  when Settings saved it.
- Settings kept saying "valid" after another editor broke the file, said "Not signed
  in." when the keychain could not be read, and said nothing when **Open file**, the
  popover's **Pipelines** or **Overwrite** failed.
- An unreadable config file was reported as "no accounts defined" instead of the read
  error, and "hot reload is off" now says why.
- The wizard refused watch ids with a dot or a space that Settings and the core accept,
  and kept a pasted URL's `?tab=…` or `#…` in the project path.
- A notification template that fails to render warns once, not on every tick.
- `SECURITY.md` described bridgewatch as sending authenticated `GET` requests and
  nothing else, and as following no redirect. Since 1.0 it also signs in through the
  OAuth device flow, and since 1.1 it sends the opt-in retry and play `POST`s and follows
  one redirect, without credentials, to fetch a job's log. It now says so, and documents
  where sign-in tokens are kept, the `actions = true` gate and the log file.

### Security

- rustls-webpki 0.103.16 in Cargo.lock, for RUSTSEC-2026-0382 (CVE-2026-111943):
  0.103.15 could accept a certificate chain through one of the distrusted emSign
  intermediates, whose private keys are now public.
- webpki-root-certs 1.0.10 in Cargo.lock, for RUSTSEC-2026-0383 (the same emSign
  distrust): 1.0.9 still bundled the emSign ECC Root CA - G3. No target bridgewatch ships
  compiles it (it arrives through rustls-platform-verifier on wasm32 only), so this clears
  lock-based scanners and keeps a future target from picking up the old root.
- The frontend tests' undici is 8.12.2 (GHSA-xjj6-f33q-929c, a denial of service through
  excessive content-encodings; test time only, not in the app).

## [1.1.0] - 2026-09-27

### Added

- **Deploy ETA.** A running row now says how long this watch's deploys usually take and
  how far in this one is (`usually ~11m · 6m 30s in` in the popover, the same after the
  row in `bridgewatch check` and `bridgewatch watch`, and an `eta` object in their
  `--json`). It is learned with no extra API requests: each pipeline that deploys adds
  its push-to-marker-finished time, the median of the last 20 is shown once there are 3,
  and it is hidden when the row settles or has run past twice the usual time. Kept in
  `eta.json` beside the notification ledger and shared by the app and the CLI. On by
  default; `eta = false` on a watch, or the new **Deploy ETA** box in Settings, turns it
  off.
- **A log file.** The app now writes its log to a daily file as well as to stderr, in
  `~/Library/Logs/bridgewatch` on macOS and `$XDG_STATE_HOME/bridgewatch` (by default
  `~/.local/state/bridgewatch`) on Linux, keeping the last 7 days. A macOS app started
  from Finder or the Dock sends stderr nowhere, so until now it left no log at all. The
  tray menu's new **Open log folder** opens it. The level is unchanged: `RUST_LOG`
  still wins, then `[log].level`, then `warn`.
- **Linux arm64 and `.rpm` release assets.** Every release now ships a `.deb`, an `.rpm`
  and an `.AppImage` for both x86_64 and arm64 Linux, each built on Ubuntu 22.04 (the
  oldest GitHub runner per architecture), so the glibc floor stays 2.35 on both. The
  `.rpm` requires `/usr/bin/secret-tool`, which resolves to `libsecret` on Fedora and to
  `secret-tool` on openSUSE.
- **The end of a failed job's log, in the popover and on the command line.** A failed
  job (parent or child, GitLab or GitHub) gets a **log** button that shows its last 40
  lines under the row, as plain text with escape sequences, section markers and runner
  timestamps taken out, with a link to the full log. `bridgewatch log <job-url>` prints
  the same. Read on demand only, never polled, and at most 256 KiB of a log is read.
  GitHub serves a log from a signed storage URL: bridgewatch follows that one redirect
  itself, over https only, and sends no token there.
- **Retry and play, opt in.** With `actions = true` on an account, a failed or canceled
  job gets **retry** and a GitLab manual job gets **play**, each confirmed first, and
  `bridgewatch retry <job-url>` and `bridgewatch play <job-url>` do the same (`--yes` to
  skip the question, required without a terminal). Off by default: without it the
  client refuses every write before anything is sent. The token needs the `api` scope on
  GitLab or Actions: write on GitHub, and a GitLab sign-in made with `read_api` is asked
  to sign in again rather than sent. Every write is logged at `info`. The README's
  section [Retry and play](README.md#retry-and-play-opt-in) has the details.

### Changed

- **A tick's requests run side by side.** A watch's pipelines, each pipeline's jobs and
  bridges, the child pipelines behind them, and the watches themselves are fetched
  concurrently, at most 4 requests at a time per account, instead of one after another.
  Measured with `bridgewatch check` on one watch with a deep bridge fan-out, the
  requests after the pipeline list took 11 to 12 s one after another and 3.1 s now. Rows, notifications, the error a watch reports and the order of the
  Debug section's request list are exactly what the one-at-a-time poller produced.
- **Only the accounts a watch uses have their token resolved.** `bridgewatch check
  --watch <id>` no longer reads a credential (or raises a Keychain prompt) for an account
  that watch does not use, and a broken credential on such an account no longer fails
  it.
- **One universal macOS `.dmg`** (`bridgewatch_<version>_universal.dmg`) replaces the
  separate Apple Silicon and Intel downloads. It is signed and notarised as before, and
  the release checks that its binary carries both architectures.

### Fixed

Four of these were seen on real pushes to a GitLab parent/child project.

- **"Deployed" before anything shipped.** With several `deploy_markers`, the first one
  to succeed read as deployed while the others were still queued, so a push showed the
  filled check ~30 s in and ~20 minutes before the main deploy ran. A success now counts
  only once no other marker can still move: none is queued or running in a live
  pipeline, and no live trigger job that `dive` would walk into still has an unread
  child.
- **"Failed" at pipeline birth.** A trigger job that had not yet created its child
  pipeline read as a dead bridge, so every push went red on its first poll and raised a
  `blocking_failure` notification. It now reads by its own status (running, awaiting a
  gate, skipped or canceled), and only a trigger that settled without a child is dead.
- **"Deployed" announced more than once for one push.** The notification was keyed on
  the winning marker's name, which changed as later markers passed. It is now keyed on
  the pipeline, and a ledger written by 1.0.0 is still honoured, so upgrading does not
  repeat a deploy already announced.
- **A skipped pipeline hid the real verdict.** A push that matched no job's rules
  created a `skipped` pipeline that took the newest row and turned the icon `canceled`.
  Skipped pipelines are now passed over and do not count against `show.settled`. A
  canceled pipeline still shows.
- **The setup wizard offered check jobs as deploy markers.** A job whose name or stage
  leads with `verify`, `test`, `lint`, `check`, `validate` or `smoke` (for example
  `verify:origins_deploy_script`) is no longer offered.
- **A keyring lookup that never answers no longer stalls polling.** Reading another
  tool's credential (glab's or gh's empty-user item) now has the same 10 s limit as a
  token command, and running out of it is reported as the store not answering rather
  than as a missing credential.

## [1.0.0] - 2026-09-19

### Added

- **GitHub Actions.** An account takes `provider = "github"` (the default stays
  `gitlab`, so an existing file means what it always did), and `base_url`, `api_path`
  and `header` default to github.com's. GitHub Enterprise Server is `base_url` plus
  `api_path = "/api/v3"`. A GitHub watch names its repository as `owner/repo` and its
  `sources` are workflow events. It reads in one of two forms:
  - `group = "run"`, the default: one workflow run is one row, optionally narrowed to one
    workflow with `workflow = "ci.yml"` (a file name or a numeric id).
  - `group = "commit"`: every run one push started (same commit, event and branch,
    created within `fan_out_secs`, default 90, of the newest) is one row, with each run
    as a bridge. The verdict rules are the ones a GitLab parent pipeline gets, so a push
    that deployed while a sibling workflow failed reads `deployed_with_failure`, and
    `dive` selects workflows by name.
- **`expect`**, for a workflow that never started. On a commit group,
  `expect = ["ci.yml", "release.yml"]` names workflow files every push must start. Once
  the group has settled and its window has passed, a missing one is a dead bridge, read
  as a GitLab trigger job that created no child is; until then it is a pending job, so
  the row reads running rather than green. `config validate` warns about the shapes that
  would read a group dead for the wrong reason.
- **Setting up GitHub from the wizard, Settings and the CLI.** The wizard offers GitHub
  Actions beside GitLab, github.com or GitHub Enterprise, gh's keyring item when one
  exists (the empty-user item under `gh:<host>`, which follows `gh auth switch`),
  repositories by `owner/repo`, workflow events and an optional workflow. **Test
  connection** lists a classic token's scopes and warns when `repo` is missing. New
  GitHub watches poll at 30 s live and 120 s idle. `bridgewatch init` gains
  `--provider`, `--gh` and `--workflow`. Settings dims the keys that do not apply to an
  account's provider, and changing an account's provider drops the old provider's
  default `base_url`, `api_path` and `header` while keeping values you customised.
- **GitHub's rate limit is handled as one.** A 403 or 429 carrying
  `x-ratelimit-remaining: 0` or `retry-after` backs off until GitHub's reset time rather
  than being read as a bad token. Requests to GitHub are conditional (ETags, kept in
  memory), and an authenticated 304 does not count against the 5,000 an hour, so a
  settled GitHub watch costs nothing.
- `examples/github.toml`, a GitHub example beside the GitLab one, following this
  repository's own workflows in both forms.
- The job view links a bridge to the trigger job itself, beside the existing link to the
  child pipeline it created. They are different pages in GitLab and either can be the one
  that explains a verdict. A bridge with no URL shows no link.
- On macOS the tray tooltip names the current state, for example "bridgewatch: deployed".
  Linux has no tray tooltip, and its tray still reports the binary name as its title over
  D-Bus: the only lever Tauri 2 offers there draws text in the panel beside the icon.

### Changed

- API responses are fetched gzip-compressed. A 20-row pipelines list drops from 7550 bytes
  to 1480, and an installed build was measured at about 70 MB an hour while a pipeline is
  live, which matters on a metered connection.
- `[log].level = "info"`, the default, now says something. One line each time a
  configuration is loaded or reloaded, naming the file, its account and watch counts and
  the log level actually in force, and one line for every watch whose verdict changes.
  `debug` adds per-tick and per-request detail, never a token. The configuration line now
  survives a first run, where the setup wizard writes the file after startup.

### Fixed

- `bridgewatch init` and the setup wizard no longer add a second primary watch to a file
  that already has one. The new watch is written as `role = "secondary"`, with a note on
  stderr from the CLI and a note on the wizard's review step; `--primary`, or **Make
  this watch primary too** in the wizard, keeps it primary. A fresh config is written as
  before.
- `check --watch` and `watch --watch` now answer for the watches you name. Naming only
  secondary watches printed `icon: unknown` and exited 4 over a verdict that had been read
  perfectly well, so a failed schedule never exited 1. Without `--watch`, and in the tray,
  nothing changes: primary watches alone decide.
- Log output no longer carries ANSI colour escapes when stderr is not a terminal, so a
  piped `bridgewatch check` and the desktop app's lines in the systemd journal are plain
  text. `NO_COLOR` is honoured.
- The `bridgewatch` CLI now follows the logging rule the app and the README already
  described: `[log].level` applies to bridgewatch's own crates and leaves everything else
  at `warn`, and an empty `RUST_LOG` counts as unset instead of silencing the run. A
  non-empty `RUST_LOG` still wins whole.
- Configuration docs corrected where they contradicted the code, in the README, the
  example config and the hints the Settings window shows: `[icon].theme` describes the
  `<glyph>.png` and `<glyph>@1x.png` files that are actually read, and `deploy_markers`
  says that the order they are listed in decides which success is reported, not which
  job ran first.

### Removed

- The "First run" banner on the Settings accounts tab. Nothing has seeded an example
  config since the setup wizard replaced that, so the banner could never appear.

### Security

- On macOS a keyring token source with `user = ""` now asks for the item whose account
  is empty (`security find-generic-password -a ""`). It used to ask by service alone,
  and when a service holds more than one item macOS returns a named one: gh keeps two
  under `gh:<host>`, so the lookup could read a different account's token than the one
  configured once somebody ran `gh auth switch`. glab writes only one item, so its
  lookup reads what it always did.
- A GitHub account's links open on its web host (`github.com` for `api.github.com`, the
  server itself for Enterprise Server), matched by scheme, host and port like every
  other link; look-alike hosts, other ports and `gist.github.com` are still refused. A
  GitHub pagination `Link` that names another host is refused rather than followed, so
  the token never travels with it.
- quick-xml 0.38.4 -> 0.42.0 (through plist 1.10.1) for RUSTSEC-2026-0194 and
  RUSTSEC-2026-0195, found by the new `cargo deny` job before GitHub's database had them.
- CI now runs `cargo deny` (advisories, licences, sources), zizmor over the workflows,
  dependency review on pull requests and a weekly OpenSSF Scorecard.
- Release builds run in a tag-only `release` environment, which holds the macOS signing
  identity as its own environment secrets, so no branch workflow and no pull request can
  read it. `v*` tags are protected against being moved or deleted.
- A weekly job re-runs `cargo deny` and `npm audit --audit-level=high`. Advisories land
  against code that did not change, so a scan that only runs on a push never sees them.
  The npm audit covers the whole tree, dev dependencies included, because this is a Vite
  app and they are what gets bundled into the shipped frontend.
- Dependabot's patch and minor updates now enable GitHub auto-merge and land on their own
  once the eight checks the `main` ruleset requires have passed, dependency review among
  them. A major update waits for a person. The workflow triggers on `pull_request`, never
  `pull_request_target`, and never checks out the pull request's code.
- deny.toml now lists all seven unmaintained transitive crates by advisory id, each with
  the path it arrives by and what would retire it: proc-macro-error through Tauri's GTK 3
  stack, smartstring through rhai, and five unic crates through urlpattern and
  tauri-utils. `cargo deny` now judges unmaintained crates across the whole tree instead
  of only the workspace's direct dependencies, so those seven are tolerated by name and a
  new abandoned crate anywhere fails CI. No dependency changed.

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

[Unreleased]: https://github.com/distronode-corporation/bridgewatch/compare/v2.0.0...HEAD
[2.0.0]: https://github.com/distronode-corporation/bridgewatch/compare/v1.3.0...v2.0.0
[1.3.0]: https://github.com/distronode-corporation/bridgewatch/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/distronode-corporation/bridgewatch/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/distronode-corporation/bridgewatch/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/distronode-corporation/bridgewatch/compare/v0.1.0...v1.0.0
[0.1.0]: https://github.com/distronode-corporation/bridgewatch/releases/tag/v0.1.0
