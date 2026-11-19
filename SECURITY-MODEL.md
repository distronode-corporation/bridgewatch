# Security model

bridgewatch holds tokens for your GitLab and GitHub accounts and reads pipelines with them. This
document says what it protects, who it defends against, what it trusts, and how the code holds
each line. The control-by-control detail (how tokens are bound to their server, the OAuth flow,
the log file, what is sent where) is in the "Security model" section of [SECURITY.md](SECURITY.md#security-model),
and how the parts fit together is in [ARCHITECTURE.md](ARCHITECTURE.md).

This document is bridgewatch's assurance case: the argument, with its evidence, that the
security requirements in [SECURITY.md](SECURITY.md#security-model) are met. Its parts are the
threat model, the trust boundaries, the secure design principles applied and the common
weaknesses countered, and [the last section](#the-argument-and-its-evidence) says how they fit
together and what keeps the evidence current.

## Assets

- **Your tokens and sign-ins.** A GitLab or GitHub token can read, and with `actions = true` retry
  or start, jobs on every project it reaches.
- **Where a token may go.** Each token belongs to one server; sending it anywhere else is the
  same as leaking it.
- **Your config file**, because a `command` token source in it runs a program as you.
- **Your machine's integrity** against the content bridgewatch renders: job names, branch names,
  logs and URLs all come from the provider's API.

## Threat model

Who can do what, and what stands in their way:

| Actor | What they can do | What stops them |
|---|---|---|
| A hostile or compromised GitLab or GitHub instance, or a project's contents | Return any JSON, any URL, any job log, any redirect, any amount of data. | Typed decoding; no redirects followed (one exception for job logs, to `https` only and with no credential); a next-page link to another host refused; logs stripped of control characters; links opened only on configured hosts; capped reads. |
| Anyone on the network path | Watch or alter traffic. | HTTPS through rustls with certificate verification that cannot be turned off; a warning for a plain `http://` `base_url` that is not loopback. |
| A script running in one of the app's web views | Call any IPC command the shell exposes. | The capability file grants `core:event:default` only; a command source, a credential moving to a new host and `actions = true` all need a confirmation the Rust side words and checks; the CSP allows scripts from the app only. |
| A verdict script you or a shared config supplied | Run code inside the process that holds the tokens. | The Rhai engine has no module resolver, no `eval`, and caps on operations, depth and sizes; it gets the pipeline view only, no token and no HTTP client. |
| Someone who reads your logs, issues or fixtures | Read whatever bridgewatch wrote down. | Tokens are `Secret` values that print `<redacted>`; requests are logged without headers; signed log URLs as host and path only; fixtures are filtered through an allow-list and checked by a test. |

Out of scope, as [SECURITY.md](SECURITY.md#scope) lists: the OS keyring's own model, anything that
runs as your user already, an attacker who can write your config file or read your keyring, and a
web view that is already fully compromised (it could read a confirmation id and send it back).

## Trust boundaries

| Party | Trusted? | Why |
|---|---|---|
| You, through your own editor and terminal | **Yes** | The config file is yours; edits made outside the app are not second-guessed. |
| The OS credential store and the account it runs as | **Yes** | It holds the tokens. bridgewatch reads gh's and glab's items in place and never copies them. |
| The Rust shell and core | **Yes** | They decide every write, every request and every link. |
| The web views (`src/`) | **No** | They render untrusted text and can reach IPC, so a sensitive change they ask for is parked until confirmed. |
| GitLab and GitHub responses | **No** | Every field is decoded into a typed value; URLs from them pass the link rule before anything opens them. |
| The network | **No** | TLS with verification on every request. |
| A verdict script | **No** | Sandboxed as above. |
| The program of a `command` token source | Yes, once you confirm it | It runs as you; the confirmation is what makes it a decision. Its output is bounded and only the first line is used. |

## Secure design principles applied

- **Least privilege.** Reads by default: writes need `actions = true` per account, and the GitLab and
  GitHub clients refuse before building a request otherwise (`ClientError::ActionsDisabled`, see
  `crates/bridgewatch-core/src/actions.rs`). GitLab
  sign-in asks for `read_api` unless the account can write. The web views get one Tauri
  capability (`src-tauri/capabilities/`, pinned by `src-tauri/tests/capabilities.rs`). The code is
  `#![forbid(unsafe_code)]` in all three crates.
- **Fail closed.** A token bound to another server, a sign-in from another instance, a redirect, a
  link on an unknown host, a stale config save, or a keychain that is locked all end in a refusal,
  never in a send or a write (`token.rs`, `client/http.rs`, `src-tauri/src/links.rs`,
  `src-tauri/src/config.rs`). A broken verdict script yields `unknown`, not a confident colour.
- **Complete mediation.** Every request to GitLab or GitHub goes through the one `Transport` (the
  opt-in crash reports are the one other outbound traffic, sent by the Sentry SDK over a client
  bridgewatch builds with the same TLS verification); every URL the
  app opens goes through one function in `links.rs`; every config write from a window goes through
  `guard.rs`, whatever route it took (Settings, "Edit as text", the wizard or a raw IPC call).
- **Economy of mechanism.** One core holds every rule and both front ends call it; the IPC layer
  and the TypeScript hold no business logic (`src-tauri/src/commands.rs`). There is no local
  redirect listener and no client secret in the OAuth flow.
- **Safe defaults.** Read only, crash reports off until you say yes (Don't send is the default),
  no telemetry and no update check.
- **Separation of secrets from configuration.** The config file names where a token comes from,
  never the token; a literal token is refused at load time and the wizard refuses to write one.

## Common weaknesses countered

Mapped to the CWE Top 25 and the OWASP Top 10. Each line names the mechanism and the tests that
pin it (paths under `crates/bridgewatch-core/tests/` unless they say otherwise).

| Weakness | How it is countered | Tests |
|---|---|---|
| Insufficiently protected credentials, credentials sent to the wrong host (CWE-522, CWE-201) | Tokens are bound to scheme, host and port; no redirects; a next-page link to another host refused; the log redirect carries no credential. | `client.rs` (`a_redirect_is_an_error_and_the_token_does_not_travel`), `token.rs`, `oauth.rs`, `github.rs` (`a_next_page_on_another_host_is_refused_rather_than_followed`), `job_actions.rs` |
| Cleartext storage of credentials (CWE-312, CWE-256) | Tokens live in the OS credential store; a literal token in the config is refused; the wizard refuses to write anything token-shaped. | `config.rs` (`a_literal_token_is_refused_with_advice`), `wizard.rs` (`a_token_anywhere_in_the_result_is_refused`) |
| Sensitive information in logs and errors (CWE-532, CWE-209, OWASP A09) | `Secret` prints `<redacted>`; the request log has no field for headers; OAuth requests are logged by path. | `client.rs` (`a_secret_redacts_itself`), `oauth_log.rs`, `client_log.rs`, `notify_log.rs`, `job_actions_log.rs`, `logging.rs` |
| URL redirection to an untrusted site, opening hostile links (CWE-601) | Links open only on a configured account's scheme, host and port; look-alike hosts, `file:` and `javascript:` refused. | `src-tauri/src/links.rs`, `src-tauri/src/tray.rs` (`a_hostile_web_url_never_reaches_the_opener`), `oauth.rs` (`a_verification_page_on_another_host_is_refused`) |
| OS command injection (CWE-78) | A command token source runs its argument vector with no shell, stdin closed, a timeout and a bounded read; it is written only after confirmation. | `token.rs` (the `command` source tests), `src-tauri/src/wizard.rs` (`a_command_source_runs_only_after_confirmation_and_then_stays_approved`) |
| Code injection (CWE-94) | The Rhai verdict script has no module resolver and no `eval`, and is capped. | `verdict.rs` (`a_verdict_script_cannot_import_a_file`, `a_verdict_script_cannot_call_eval`, `a_runaway_script_is_stopped_by_the_operation_cap`) |
| Missing authorization for a sensitive action, cross-context requests (CWE-862, CWE-352) | Writes need `actions = true` and a per-click confirmation; sensitive config edits from a web view need a confirmation the Rust side issues. | `job_actions.rs` (`a_write_on_an_account_without_actions_is_refused_before_anything_is_sent`), `src-tauri/src/guard.rs`, `src-tauri/src/commands.rs` |
| Improper input validation (CWE-20) | The config is decoded into typed values with line and column errors; wizard answers are checked step by step, with the shell and the core held to the same rules. | `config.rs`, `wizard.rs` (`bad_answers_are_refused_with_every_step_named`), `wizard_validation_parity.rs` |
| Path traversal and arbitrary file read (CWE-22) | `[ui].theme_css` is read only for a path ending in `.css`, up to 256 KiB; scripts cannot import files. | `src-tauri/src/commands.rs` (`theme_css_reads_a_stylesheet_and_nothing_else`), `verdict.rs` |
| Uncontrolled resource consumption (CWE-400) | Bounded caches, request ring and command output; capped list pages; the Rhai operation cap. | `conditional.rs` (`the_byte_bound_holds_and_an_oversized_body_is_not_stored`), `token.rs` (`a_chatty_token_command_is_bounded_but_still_works`), `github.rs` (`a_listing_stopped_at_the_page_cap_reports_itself_truncated`) |
| Improper neutralization of output (CWE-150) | Job logs are stripped of ANSI escapes and control characters before display. | `job_actions.rs` (`ansi_escapes_and_control_characters_are_stripped_and_a_tab_is_kept`) |
| Race on the config file (CWE-367) | Saves from the GUI are compare-and-swap against the text the window read. | `src-tauri/src/config.rs` (`a_save_based_on_stale_text_is_refused_and_the_file_is_untouched`) |
| Improper certificate validation (CWE-295) | rustls with the platform verifier; there is no option to skip verification. The crash reporter is handed a client bridgewatch builds, so the Sentry SDK's `SSL_VERIFY` switch does not apply. | The core's transport: mechanism only, no code path builds a client without verification. The crash reporter: `src-tauri/tests/crash_report_tls.rs` (`ssl_verify_cannot_turn_off_verification`). |
| Exposure of personal data in committed fixtures (CWE-359) | Recordings pass an allow-list of the fields the engine reads. | `verdict.rs` (`fixtures_contain_no_personal_data`) |
| Vulnerable and outdated components (OWASP A06) | `cargo deny` and dependency review on every pull request, weekly `cargo deny` and `npm audit`, Dependabot, CodeQL. | `.github/workflows/ci.yml`, `.github/workflows/audit.yml` |

## The argument and its evidence

The claim is that bridgewatch meets the requirements in the "Security model" section of
[SECURITY.md](SECURITY.md#security-model), against the actors in the threat model above, for
the scope [SECURITY.md](SECURITY.md#scope) sets. The argument has four parts:

1. **Every actor meets a control.** Each row of the threat model names what stops that actor.
   Each of those controls is either described in the "Security model" section of SECURITY.md
   or, for typed decoding, the stripping of control characters from logs and the capped
   reads, mapped in the weakness table below to its mechanism and tests.
2. **Every crossing of a trust boundary is checked in one place.** Untrusted data enters at
   four points: the GitLab and GitHub responses (decoded into typed values by the client,
   behind the one `Transport`), the web views' IPC calls (gated in Rust by `guard.rs` and the
   command handlers), a verdict script (inside the Rhai sandbox) and, only with crash reports
   allowed, Sentry's answers to a report (handled inside the Sentry SDK, which bridgewatch
   reads nothing from). Complete mediation, above, is what makes "one place" true for the
   first three, so a control cannot be skipped by taking another route.
3. **The design fails closed.** Where a check cannot decide, the answer is a refusal or
   `unknown`, never a send, a write or a confident colour (the principles above).
4. **The common weaknesses are countered in code and pinned by tests.** The table above maps
   each weakness to its mechanism and to the tests that fail if the mechanism is removed. One
   row, certificate validation, is held by construction rather than by a test for the core's
   transport: there is no code path that builds an HTTP client without verification.

What keeps the evidence current:

- The tests named above run in CI's `rust` and `frontend` jobs on every pull request and on
  every push to `main`, and the `main` ruleset requires those jobs to pass before a merge
  ([GOVERNANCE.md](GOVERNANCE.md)).
- CodeQL analyses the Rust, TypeScript and workflow code, and the ruleset blocks a merge on a
  CodeQL error or a security alert of high severity or above. zizmor checks the workflows.
- `cargo deny`, dependency review, a weekly `npm audit` and Dependabot watch the dependencies
  ([Installing bridgewatch](docs/install.md#what-guards-the-code-and-the-release-path)).
- A change to a control changes this document and SECURITY.md in the same pull request, as
  [CONTRIBUTING.md](CONTRIBUTING.md#docs) requires of every document that describes
  behaviour.

What the argument does not cover is the out-of-scope list under the threat model: the OS
keyring's own model, code already running as you, an attacker who can write your config file
or read your keyring, and a fully compromised web view. Those are residual risks, accepted and
stated rather than countered.
