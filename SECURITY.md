# Security Policy

## Reporting a vulnerability

Please report privately, not in a public issue.

- **Preferred:** GitHub's private vulnerability reporting. Open the repository's
  **Security** tab and choose **Report a vulnerability**, or go straight to
  <https://github.com/distronode-corporation/bridgewatch/security/advisories/new>.
- **Fallback:** email **distronode@distronode.com** if you cannot use GitHub.

Include what you did, what happened, and what you expected. A proof of concept is welcome
but not required. Never include a real token; if a token is part of the problem, say where
it appeared, not what it was.

Expect an acknowledgement within a few working days. This is a small project with no paid
bug bounty; what you get is credit in the changelog entry for the fix, if you want it.

## Supported versions

bridgewatch is pre-1.0. Only the latest 0.x release is supported. Fixes go into a new
release rather than being backported.

| Version | Supported |
| --- | --- |
| Latest 0.x release | Yes |
| Anything older | No |

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
- HTTP redirects are not followed; a redirect from GitLab or GitHub is an error (a `304`
  answering bridgewatch's own conditional request to GitHub is not a redirect). This keeps
  the `PRIVATE-TOKEN` or `Authorization` header from travelling to whatever host a
  redirect names. GitHub paginates through a `Link` header, and a next-page link that
  names another host is refused rather than followed.
- A plain `http://` `base_url` is accepted (for local and internal instances) but
  validation warns about it unless the host is loopback, because the token would cross
  the network unencrypted. HTTPS uses rustls with certificate verification and no option
  to turn it off. System and environment proxy settings are honoured.

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

Out of scope:

- The OS keyring's own security model, and the fact that an item readable by your user
  is readable by anything running as your user.
- A user deliberately configuring a destructive command; the confirmation exists so that
  is a decision rather than an accident.
- Anything that needs an attacker who can already write your config file or read your
  keyring.
- Load on your own GitLab or GitHub account from a very short poll interval.

## What bridgewatch sends where

- Authenticated `GET` requests to the API of each configured account's `base_url`, and
  nothing else over the network. There is no telemetry, no crash reporting service and
  no update check.
- Your default browser, for links that pass the rule above.
- Local programs: `security` (macOS) or `secret-tool` (Linux) to read a keyring item
  with an empty user name, and the program of a `command` token source, if you configure
  one.
