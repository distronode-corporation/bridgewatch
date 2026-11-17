# Architecture

How bridgewatch is put together: the parts, how a poll moves through them, and where state
lives. What each key means is in [docs/configuration.md](docs/configuration.md), the rules that
pick an icon are in [docs/verdicts.md](docs/verdicts.md), and what bridgewatch trusts is in
[SECURITY-MODEL.md](SECURITY-MODEL.md).

## The shape

One Rust core and two thin front ends over it. The core holds every decision: what to fetch,
what a pipeline means, what is worth a notification, and whether a config edit is valid. The
CLI prints what the core produced, and the tray app draws it.

```
     config.toml           OS keyring, env, gh/glab items, a token command
         |                                  |
         v                                  v
  +----------------------------------------------------------------------+
  | bridgewatch-core (no Tauri, no GUI)                                   |
  |                                                                      |
  |  config --> poll (planner, policy) --> client (GitLab, GitHub) ------+--> HTTPS to each
  |                 |                          |  Transport seam         |    account's base_url
  |                 v                          v                         |
  |              verdict (pure) <------- recorded responses               |
  |                 |                                                    |
  |                 v                                                    |
  |              Snapshot --> notify, eta, health                         |
  +----------------------------------------------------------------------+
         |                                   |
         v                                   v
  bridgewatch-cli                     src-tauri (bridgewatch-app)
  check, watch, wait, auth,           tray, popover, Settings, wizard,
  config, init, fixture               IPC commands, guard, link opener
                                             |  IPC (core:event:default only)
                                             v
                                      src/ (Svelte 5 web views)
```

## Parts

| Part | Role |
|---|---|
| `crates/bridgewatch-core/src/config/` | The TOML file: typed parsing (a value of the wrong type is an error, an unknown key a warning), the JSON Schema, validation with line and column, and format-preserving edits through `toml_edit`. |
| `crates/bridgewatch-core/src/token.rs`, `token/` | Resolving a credential from the source the config names (a keyring item, bridgewatch's own entry, an environment variable, a command). A token is a `Secret` that prints `<redacted>`. |
| `crates/bridgewatch-core/src/oauth/` | Sign-in with the OAuth 2.0 device authorization grant, and refreshing the stored token set. |
| `crates/bridgewatch-core/src/client/` | The GitLab and GitHub clients behind one `CiClient` trait, over the `Transport` seam (`http.rs`). The real transport is reqwest with rustls and no redirects; the tests use `FixtureTransport` or a scripted one. Conditional requests to GitHub live in `conditional.rs`. |
| `crates/bridgewatch-core/src/poll/` | The poller: one tick asks the planner what is due, fetches it, runs the verdict engine and the notifier, and publishes a `Snapshot`. `policy.rs` holds back-off and refused accounts. |
| `crates/bridgewatch-core/src/verdict/` | Pure functions from API responses to the view model: bridges, deploy markers, the icon state, and the optional Rhai script (`script.rs`). No I/O, no clock, no token. |
| `crates/bridgewatch-core/src/notify.rs`, `eta.rs`, `health.rs` | What is worth a notification (and the ledger that remembers it), the deploy ETA, and token expiry warnings. |
| `crates/bridgewatch-core/src/wizard.rs` | The setup wizard's steps, with no UI in them. |
| `crates/bridgewatch-cli/` | The `bridgewatch` binary: `check`, `watch`, `wait`, `auth`, `config`, `init` and `fixture`, with exit codes taken from constants in `main.rs`. |
| `src-tauri/` | The tray app (crate `bridgewatch-app`): tray and windows, the background poll loop (`poller.rs`), the IPC commands (`commands.rs`), the confirmation gate for sensitive edits (`guard.rs`), config file I/O and hot reload (`config.rs`), the link opener (`links.rs`), logging and opt-in crash reports. |
| `src/` | The Svelte 5 and TypeScript front end: popover, job view, Settings and the wizard. It calls IPC commands and renders what they return; no business logic lives there. |
| `packaging/npm/` | The npm package: a launcher with no dependencies that runs the right prebuilt CLI binary. |

## A poll, start to finish

1. **Load.** The app or the CLI loads `config.toml`. A file that does not parse, holds a value
   of the wrong type or a literal token is refused with the line and column; an unknown key is a
   warning, so a file written for a newer build still loads.
2. **Plan.** A tick runs on the poll interval, or sooner for a manual refresh, a popover open, a
   config change or a wake from sleep. The planner (`poll/planner.rs`) decides what the tick
   fetches before fetching any of it, so an idle tick costs one request per watch; `policy.rs`
   skips accounts that are backing off or were refused.
3. **Resolve.** For each account it needs, the core resolves the token from its source. A token
   bound to another server, or a sign-in issued by another instance, is not sent.
4. **Fetch.** The client lists pipelines (GitLab) or workflow runs (GitHub), walks bridges into
   child pipelines, and reads jobs. Every request goes through the `Transport`, which follows no
   redirect and records method, path, status and timing in a bounded ring for the debug pane.
5. **Decide.** The verdict engine turns the responses into one row per push, each with its
   bridges and a verdict from the deploy markers. The icon follows the newest pipeline of each
   primary watch, and with more than one the worst state wins.
6. **Tell.** The notifier compares the snapshot with its ledger and emits the notifications that
   are new; the app hands them to the OS, the CLI prints them. The ETA learns from deployed rows.
7. **Show.** The app pushes the snapshot to the tray and the web views. The CLI prints it (or JSON)
   and exits with the verdict's code.

A write (retry or play) takes the same path in reverse, only from an account with
`actions = true`, and only after the popover or the CLI has asked.

## Where state lives

- **`config.toml`** in the OS config directory: the only place configuration lives. The app saves
  it by compare-and-swap and hot-reloads edits made in an editor.
- **The OS credential store** (macOS keychain, or the Secret Service on Linux): bridgewatch's own
  tokens under `bridgewatch:<account>`, sign-ins under `bridgewatch:oauth:<account>`. Tokens read
  from gh, glab, an environment variable or a command are read where they are, never copied.
- **The state directory** (`~/Library/Application Support/bridgewatch` on macOS,
  `$XDG_STATE_HOME/bridgewatch` on Linux): the notification ledgers (`notify.json` for the app,
  `notify-cli.json` for the CLI) and `eta.json`.
- **The log directory**: the app's daily log files, seven days kept (see [SECURITY.md](SECURITY.md#the-log-file)).
- **In memory only**: the last snapshot, the request ring, the confirmation ids for pending edits.

## Testing seams

Everything above the `Transport` is tested without a network: GitLab behaviour against fixtures
recorded from real pipelines (`crates/bridgewatch-core/tests/fixtures/`), GitHub against a scripted
transport. Token code is tested through the `TokenProvider` trait with a fake, so no test reads a
real keyring. The front end is tested with vitest in jsdom, and two parity tests hold the config
schema and the Settings registry together (`src-tauri/tests/schema_parity.rs`,
`src/lib/settings/registry.test.ts`).
