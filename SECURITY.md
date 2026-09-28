# Security Policy

## Reporting a vulnerability

Please report privately, not in a public issue.

- **Preferred:** GitHub's private vulnerability reporting. Open the repository's
  **Security** tab and choose **Report a vulnerability**, or go straight to
  <https://github.com/distronode-corporation/bridgewatch/security/advisories/new>.
- **Fallback:** email **opensource@distronode.com** if you cannot use GitHub.

Include what you did, what happened, and what you expected. A proof of concept is welcome
but not required. Never include a real token; if a token is part of the problem, say where
it appeared, not what it was.

Expect an acknowledgement within a few working days. This is a small project with no paid
bug bounty; what you get is credit in the changelog entry for the fix, if you want it.

## Supported versions

Only the latest release is supported. Fixes go into a new release rather than being
backported.

| Version | Supported |
| --- | --- |
| 1.1.x (current release: 1.1.0) | Yes |
| 1.0.x | No |
| 0.x | No |

## Security model

What bridgewatch does to protect you, so a report can say which of these it breaks.

### Tokens are never in the config file

- The config file records where a token comes from (a keyring item, bridgewatch's own
  keyring entry, an environment variable, or a command), never the token. A literal token
  string in `token = ...` is refused at load time, and the setup wizard refuses to write
  a file that contains anything shaped like a GitLab token, or for a GitHub account a
  GitHub token.
- In memory a token is a `Secret` whose `Debug` and `Display` print `<redacted>`. The
  request log records method, API path, status and timing, never headers.
- A token pasted into Settings or the wizard goes to the OS keyring (service
  `bridgewatch:<account>`), not to a file.
- The HTTP client follows no redirects: a redirect from GitLab or GitHub, on an API
  request or on a sign-in request, is an error (a `304` answering bridgewatch's own
  conditional request to GitHub is not a redirect). This keeps the `PRIVATE-TOKEN` or
  `Authorization` header from travelling to whatever host a redirect names. GitHub
  paginates through a `Link` header, and a next-page link that names another host is
  refused rather than followed.
- There is one exception, and it is made by hand rather than by the HTTP client: the
  request for a job's log. GitHub's log endpoint answers `302` with a short-lived signed
  URL on its storage host, and an archived GitLab log can redirect to object storage, so
  bridgewatch follows exactly one redirect there, and only to an absolute `https` URL
  with a host and no user name in it. That second request carries no credential header
  at all (the signed URL is its own authorisation) and is marked so that a sign-in's
  token cannot be added to it. The signed URL's query string is a credential for as long
  as it lives, so the request log and the log file record only its host and path.
- A plain `http://` `base_url` is accepted (for local and internal instances) but
  validation warns about it unless the host is loopback, because the token would cross
  the network unencrypted. HTTPS uses rustls with certificate verification and no option
  to turn it off. System and environment proxy settings are honoured.

### Sign-in tokens (OAuth)

`token = { oauth = true }` signs in with the OAuth 2.0 device authorization grant
(RFC 8628): bridgewatch asks the provider for a short user code, you type it on the
provider's own page in your browser, and bridgewatch polls until the provider answers.
There is no client secret and no local redirect listener.

- **Which application.** On github.com the built-in application is the GitHub App
  [bridgewatch-ci](https://github.com/apps/bridgewatch-ci), owned by
  distronode-corporation, whose permissions are Actions (read and write) and Metadata
  (read). A GitHub App's user token asks for no scope: what it can reach is the
  intersection of your own access and those permissions, on the repositories where the
  App is installed. On gitlab.com the built-in application is a non-confidential OAuth
  application, and bridgewatch asks it for `read_api`, or for `api` only on an account
  with `actions = true` (GitLab has no narrower scope that can retry or play a job).
  `oauth = { client_id = "..." }` names your own application instead, which is the only
  way to sign in to GitHub Enterprise Server or a self-managed GitLab.
- **Where the requests go.** GitHub's endpoints are on the web host, never the API host:
  `https://github.com/login/device/code` and `/login/oauth/access_token`, or the same
  paths on a GitHub Enterprise Server. GitLab's are `<base_url>/oauth/authorize_device`
  and `<base_url>/oauth/token`. A device-code answer whose verification page is on any
  other host is refused rather than opened.
- **Where the tokens live.** The access token, the refresh token, their expiry times,
  the granted scopes, the client id and the instance they were issued by are one JSON
  document in the OS credential store (the macOS keychain, or the Secret Service on
  Linux), service `bridgewatch:oauth:<account>`, user `oauth`. The config file says only
  `token = { oauth = true }`. A stored set whose instance differs from the account's
  `base_url` is ignored, so a sign-in is never sent to another host.
- **Refresh.** A token is refreshed when it expires within five minutes, and once after
  a `401` on a token more than a minute old. GitLab rotates the refresh token on every
  refresh, so the new pair is written to the credential store, in one call, before it is
  used. A refresh the provider refuses is not retried: the account reports that you must
  sign in again.
- **Sign out** deletes the credential store item. It does not revoke the grant at the
  provider; revoke it there (GitHub: Settings, Applications; GitLab: Preferences,
  Applications) if you want the tokens dead rather than forgotten.
- No access token, refresh token or device code reaches a log line, an error message or
  a `Debug` rendering; the OAuth requests are logged by path only. The user code is shown
  to you and is never logged.

### Writes are opt in: `actions = true`

bridgewatch reads by default. Only an account with `actions = true` can send the two
writes it knows: GitHub's `POST /repos/{owner}/{repo}/actions/jobs/{id}/rerun`, and
GitLab's `POST /projects/{id}/jobs/{id}/retry` and `/play`. With `actions` off (the
default) the client refuses before a request is built, whatever asks: the popover, the
CLI or a script in the web view. Turning `actions` on from the GUI is one of the changes
that need confirmation (next section). The popover asks before each write, naming the
job, the project and the pipeline, and every write is logged at `info` with the account,
project, job id, action and outcome, never the token. A GitLab sign-in made with
`read_api` is refused before a write is sent. Note that the bridgewatch-ci GitHub App
holds Actions write, so for a GitHub sign-in the gate is `actions`, not the token.

### The log file

The GUI writes a daily log file, because a tray app started from Finder, the Dock or a
login item has nowhere else to log: `~/Library/Logs/bridgewatch/` on macOS, and
`$XDG_STATE_HOME/bridgewatch/` (by default `~/.local/state/bridgewatch/`) on Linux,
named `bridgewatch.<date>.log`, seven days kept. The CLI logs to stderr only. The level
is `warn` unless `[log].level` or `RUST_LOG` asks for more.

Nothing is redacted on the way into the file, because nothing secret is put in a log
event in the first place: tokens are `Secret` values that print `<redacted>`, requests
are logged as method, path, status and timing without headers or bodies, and a signed
log URL as host and path only. What the file does hold, at `info` and `debug`, is
account names, hosts, project ids or paths, branch names, job names and ids, and API
paths. The file is created with your default file mode; read it before attaching it to
an issue.

### A command token source needs confirmation

`token = { command = [...] }` runs a program named in a plain text file, with your
permissions, every time a token is needed. When the GUI (Settings, "Edit as text" or the
wizard) would write a new or changed command source, the Rust side writes nothing and
answers with a description of the change in its own words; the write happens only when
the same text comes back with that confirmation's id. The same check covers a change that
would send an existing credential to a new host: a new `base_url` for an account that
has a token, or a keyring or environment source on a host no account used before.

The command runs without a shell (the argument vector is passed as is), with stdin
closed, a 10 second timeout and a bounded read of its output; only the first line of
stdout is used.

This guards against a script in the web view making the change in one call. It does not
defend against a web view that is already fully compromised, which could read the id and
send it back; that would need a native dialog. Edits you make in your own editor are not
checked: the file is yours.

### Links open only to your configured hosts

Every URL the app opens, from the popover or the tray menu, goes through one function in
the Rust shell. It opens only `http` and `https` URLs whose scheme, host and port match a
configured account's `base_url`, or for a GitHub account its web host (`github.com` for
`api.github.com`, the server itself for GitHub Enterprise Server), and refuses everything
else, including look-alike hosts, other ports, `file:` and `javascript:` URLs. The URLs
come from the provider's API, so this is the boundary against a hostile or compromised
instance.

### The web views have almost no capabilities

The Tauri capability file grants the popover, Settings and wizard windows
`core:event:default` and nothing else. The opener, notification, positioner and autostart
plugins are driven from Rust only. Commands that write the config file validate and gate
the write in Rust, and the file is written by compare-and-swap. `[ui].theme_css` is read
by the Rust side only when the path ends in `.css`, up to 256 KiB. The content security
policy allows scripts from the app itself only.

### The Rhai verdict sandbox

`[verdict].script` runs user-supplied [Rhai](https://rhai.rs). The module resolver is
disabled so `import` cannot read a file, `eval` is disabled, and operations, call depth,
expression depth, string, array and map sizes are capped. The script receives the
pipeline view (no token, no HTTP client) and returns a state name.

### Fixtures carry no personal data

Recorded test fixtures are put through an allow-list of the fields the engine reads, and
a test checks every committed fixture against it, because a raw GitLab payload carries
names, email addresses, commit messages and runner details. Recording from a GitHub
account is refused until GitHub payloads have an allow-list of their own.

## Scope

In scope, in rough order of damage:

- A token reaching the config file, a log line, the request log, a notification, an
  error message shown in the popover, or the output of any CLI subcommand (including
  `config dump` and `check --json`).
- A token being sent to a host other than its account's `base_url`.
- A keyring lookup matching a different item than the one configured.
- A command source running, or a credential moving to a new host, without the
  confirmation above, through any IPC path.
- Argument or shell injection into the command source, or its output leaking anywhere.
- The link opener opening a URL outside the rule above.
- A Rhai script escaping the sandbox, or reaching the token or the network.
- A notification template evaluating anything beyond the values it is handed.
- TLS verification being skippable, or the config file or keyring entry being created
  with permissive modes.
- A sign-in's access token, refresh token or device code reaching a log line, an error
  message or any file; a stored sign-in being sent to a host other than the one that
  issued it; a verification page on another host being opened.
- The log redirect being followed with a credential attached, to a non-`https` URL, more
  than once, or its signed query string reaching the request log or the log file.
- A retry or play being sent from an account without `actions = true`, or without the
  confirmation the popover asks for.
- Anything secret in the log file, or the file being written somewhere other than the
  directory above.

Out of scope:

- The OS keyring's own security model, and the fact that an item readable by your user
  is readable by anything running as your user.
- A user deliberately configuring a destructive command; the confirmation exists so that
  is a decision rather than an accident.
- Anything that needs an attacker who can already write your config file or read your
  keyring.
- Load on your own GitLab or GitHub account from a very short poll interval.

## What bridgewatch sends where

- To the API of each configured account's `base_url`: authenticated `GET` requests
  (pipelines, jobs, bridges and workflow runs, a job's log on request, and who the token
  belongs to when the wizard tests a connection or a sign-in completes).
- To the same API, and only from an account with `actions = true`: the retry and play
  `POST` requests above, one per confirmed click or CLI command.
- For a job's log: one unauthenticated `GET` to the `https` URL the provider redirected
  to (GitHub's storage host, or GitLab's object storage), as described above.
- For a sign-in, and only for an account with `token = { oauth = ... }`: form `POST`
  requests to the provider's device-code and token endpoints, on github.com, the GitHub
  Enterprise Server host, or the GitLab instance.
- There is no telemetry, no crash reporting service and no update check.
- Your default browser, for links that pass the rule above, including a sign-in's
  verification page and the page that installs the bridgewatch-ci App on an
  organisation.
- Local programs: `security` (macOS) or `secret-tool` (Linux) to read a keyring item
  with an empty user name, and the program of a `command` token source, if you configure
  one.
