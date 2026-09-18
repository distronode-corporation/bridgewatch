# bridgewatch-core

The engine behind [bridgewatch](../../README.md): a bridge-aware model of GitLab CI.
No GUI dependencies, no Tauri, no network in the tests.

This file is the API contract for the shell that wraps it. Everything named here
is public and documented; `cargo doc --open -p bridgewatch-core` has the detail.

## Why it is shaped this way

Most GitLab tray monitors define "status" as the newest pipeline on a
branch. On a parent/child estate that is wrong twice over:

- the newest pipeline is frequently an hourly **schedule** that is red by design,
  so the tray is permanently red and stops meaning anything; and
- the parent's own status says nothing about what happened inside the child
  pipelines its `trigger:*` jobs created, so nobody can say *"the website
  deployed while the android bridge failed"*. That is exactly what the recorded
  fixture `tests/fixtures/ca41ab28-deployed-with-failure` did, and what a monitor
  reading the parent's status reports as a flat `failed`.

bridgewatch-core is therefore **bridge-aware** (it walks trigger jobs into child
pipelines), **source-aware** (a watch filters by pipeline source and by role), and
derives a **deploy verdict** from marker jobs the user names.

## Layout

| module | what lives there |
|---|---|
| `config` | the TOML file, its JSON Schema, validation, and format-preserving edits |
| `token` | resolving a credential without ever storing or logging one |
| `client` | the GitLab endpoints (pipelines, jobs, bridges, child jobs, plus user, token and project lookups for the wizard), behind a `Transport` seam, plus the fixture transport |
| `status` | the status vocabulary and job classification |
| `model` | the GitLab wire types |
| `verdict` | pure functions from responses to the view model |
| `poll` | what to fetch, how often, and what to keep |
| `notify` | what is worth interrupting somebody over |
| `wizard` | the setup wizard's steps, with no UI: connection test, project listing, marker suggestions, `build_config` |

## Building a poller and subscribing to snapshots

```rust,no_run
use bridgewatch_core::{config, poll::Poller, token::SystemTokenProvider};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
// --config, then $BRIDGEWATCH_CONFIG, then <config dir>/bridgewatch/config.toml
let path = config::resolve_path(None);
let loaded = config::load(&path)?;          // `loaded.warnings` is worth showing
let mut poller = Poller::from_config(&loaded.config, &SystemTokenProvider)?
    .with_ledger(bridgewatch_core::notify::NotifyLedger::default_path());

// One snapshot per tick. `watch::Receiver` gives you the latest, never a backlog.
let mut snapshots = poller.subscribe();

// Either drive it yourself...
let tick = poller.tick().await;
println!("{}: {} notification(s)", tick.snapshot.icon_state, tick.notifications.len());
tokio::time::sleep(tick.next_interval).await;

// ...or let it loop. Return `false` to stop.
poller.run(|tick| {
    for n in &tick.notifications {
        // deliver n.title / n.body / n.url through the OS
        let _ = n;
    }
    true
}).await;
# let _ = snapshots.borrow_and_update();
# Ok(())
# }
```

`Tick` is `{ snapshot: Snapshot, notifications: Vec<Notification>, next_interval: Duration }`.
The core **produces** notifications; delivering them is the shell's job.

### Offline mode

`Poller::with_clients` takes ready-made `GitLabClient`s, so the same poller runs
against a recorded fixture directory with no token and no network. This is what
`bridgewatch check --fixture <dir>` does:

```rust,no_run
use bridgewatch_core::client::{FixtureTransport, GitLabClient, RequestRing};
use bridgewatch_core::token::Secret;
use std::{collections::BTreeMap, sync::Arc};

# fn example(config: &bridgewatch_core::Config) -> Result<(), Box<dyn std::error::Error>> {
let transport = Arc::new(FixtureTransport::load("tests/fixtures/ca41ab28-deployed-with-failure")?);
let ring = RequestRing::new(config.log.keep_requests);
let mut clients = BTreeMap::new();
for (name, account) in &config.accounts {
    clients.insert(name.clone(),
        GitLabClient::new(account, &Secret::new("fixture"), transport.clone(), ring.clone()));
}
let poller = bridgewatch_core::poll::Poller::with_clients(config, clients, ring)?;
# let _ = poller;
# Ok(())
# }
```

## The `Snapshot` JSON shape

Everything is `Serialize`, so a Tauri command can return a `Snapshot` directly.
This is the whole contract; the GUI renders exactly this and computes nothing.

```jsonc
{
  "icon_state": "deployed_with_failure",   // the tray icon: worst news among primary watches
  "last_poll": "2026-09-17T18:40:12.331Z",
  "errors": ["main-push: rate limited; retry after 60s"],
  "watches": [{
    "id": "main-push",
    "role": "primary",                     // "primary" | "secondary"
    "icon_state": "deployed_with_failure", // null for a secondary watch, always
    "error": null,
    "jobs": "all",                         // show.jobs, else ui.jobs: "all" | "failures".
                                           // Presentation only; rows carry every job either way
    "rows": [{
      "id": 2857464986, "iid": 13072,
      "sha": "ca41ab285c888b276fb4fc798de9fd39f84ace41", "sha7": "ca41ab2",
      "ref": "main", "source": "push",
      "status": "failed",                  // GitLab's own word for the parent
      "web_url": "https://gitlab.com/acme-corp/monorepo/-/pipelines/2857464986",
      "state": "deployed_with_failure",    // bridgewatch's verdict for this pipeline
      "live": false,                       // a gate is NOT live
      "deploy": "live",                    // absent|in_progress|live|failed|dead|canceled|unknown
      "deploy_marker": { "name": "deploy:origins", "id": 16556..., "pipeline_id": 2857465135,
                         "web_url": "...", "started_at": "2026-09-17T14:40:34.699Z" },
      "deploy_failures": [],               // blocking jobs in the marker's pipeline, if it failed
      "failures": ["verify:android"],      // blocking failures + any dead bridge's name
      "warnings": ["dependency-scanning:python-resolution"],
      "gates": ["verify:web_coverage_full 1/4", "ios:smoke"],
      "post_deploy_failures": [],          // blocking failures that started after the marker
      "sibling_failures": ["trigger:android"],  // failed/dead bridges other than the marker's
      "created_at": "...", "updated_at": "...",
      "parent_jobs": [{ "id": 1, "name": "secret_detection", "class": "passed",
                        "status": "success", "allow_failure": true,
                        "stage": "security", "web_url": "...",
                        "started_at": "2026-09-17T09:23:30.129Z",
                        "finished_at": "2026-09-17T09:23:47.021Z",
                        "duration": 16.892 }],       // seconds; GitLab's own, else finished - started;
                                                     // null (never 0) for a job that never ran
      "bridges": [{
        "name": "trigger:android",
        "status": "failed",                // the trigger job's own status
        "verdict": "failed",               // passed|passed_with_warnings|failed|dead|
                                           // running|awaiting_gate|canceled|skipped|unknown
        "verdict_jobs": ["verify:android"],// the failures, gates or warnings it names
        "child_id": 2857465094, "child_project_id": 82468124, "child_url": "...",
        "dived": true,                     // false ⇒ `jobs` is empty because nobody looked
        "jobs": [ /* JobView, as above */ ],
        "bridges": [],                     // the child's own bridges, when dive.depth > 1
        "web_url": "..."
      }]
    }]
  }],
  "request_log": [{ "method": "GET", "path": "/projects/82468124/pipelines?ref=main&...",
                    "status": 200, "ms": 536, "ratelimit_remaining": 1999,
                    "ratelimit_reset": 1789672380, "retry_after": null,
                    "error": null, "at": "2026-09-17T18:40:11.795Z" }]
}
```

### Icon states

`unknown` · `failed` · `deployed_with_failure` · `deployed` · `running` ·
`canceled` · `parked_gate` · `succeeded_no_deploy`

`IconState::severity()` orders them for the tray (lowest wins);
`IconState::as_str()` is also the icon file's stem, so `[icon].theme` pointing at
a directory means `<state>.png` (or the glyph name `[icon].states` maps the state
to). PNG only; the shell draws the tray icon.

### Job classes

`passed` · `live` · `blocking_failure` · `warning_failure` · `gate` ·
`not_built` · `unknown` · `ignored` (an `ignore` override removes the job from
every list, so it never appears in `jobs` at all).

## The request log

`RequestRing` is cheap to clone and shared between the poller and the GUI. It is
what the debug pane shows.

```rust,no_run
# fn example(poller: &bridgewatch_core::poll::Poller) {
let ring = poller.ring();
let recent: Vec<_> = ring.entries();     // oldest first, bounded by log.keep_requests
ring.set_capacity(200);                  // trims immediately if it shrank
ring.clear();
# let _ = recent;
# }
```

⛔ No credential ever reaches it: the auth header is not logged, and `path` is the
API path only. `client.rs`'s tests assert that a serialised ring does not contain
the token.

## Applying a config edit

The file is the source of truth and the GUI writes it back. Every change goes
through `toml_edit`, so comments, key order and `[[watches]]` order survive.
`Edit` is `Serialize + Deserialize`, so a settings pane can send a batch over IPC
and the core applies it in one go.

```rust,no_run
use bridgewatch_core::config::edit::{ConfigEditor, Edit, EditValue};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let raw = std::fs::read_to_string("config.toml")?;
let mut editor = ConfigEditor::new(&raw)?;

editor.apply(&[
    // A numeric segment indexes [[watches]]; quote a segment containing a dot.
    Edit::Set { path: "watches.1.poll.live_secs".into(), value: EditValue::Integer(30) },
    // Or address a watch by id and never think about indices.
    Edit::SetWatch { id: "main-push".into(), path: "sibling_failure".into(),
                     value: EditValue::String("fail".into()) },
    Edit::Unset { path: r#"watches.0.jobs."kics-iac-sast""#.into() },
])?;

std::fs::write("config.toml", editor.to_toml())?;
# Ok(())
# }
```

Also available: `Edit::AddWatch`, `Edit::RemoveWatch`, and on the editor itself
`watch_ids()`, `watch_index(id)`, `document()`.

⚠️ Validate before writing. `config::parse_str(&editor.to_toml(), path)` returns
`ConfigError::Invalid { diagnostics }` holding **every** problem, each with a
dotted `path`, a `message`, a `severity` and, where it could be located, a byte
`span`. `Diagnostic::line_col(raw)` turns a span into 1-based line and column for
an editor gutter. Warnings come back on a successful load, in `Loaded::warnings`.

## Tokens

```rust,no_run
use bridgewatch_core::token::{self, Secret, SystemTokenProvider};
use bridgewatch_core::config::TokenSource;

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let source = TokenSource::Keyring {
    service: "glab:gitlab.com:token".into(),
    user: String::new(),                       // ⚠️ empty is legal and is what glab writes
};
let secret: Secret = token::resolve(&source, "gitlab", &SystemTokenProvider)?;

// The GUI's "paste a token" field writes bridgewatch's own entry, which
// `token = { own = true }` reads back.
token::set_own_token("gitlab", &Secret::new("glpat-…"), &SystemTokenProvider)?;
token::clear_own_token("gitlab", &SystemTokenProvider)?;
# let _ = secret;
# Ok(())
# }
```

`Secret`'s `Debug` and `Display` both print `<redacted>`; `expose()` is the only
way to see it, and should be called as late as possible.

Three things worth knowing before wiring a settings pane:

- ⛔ **An empty keyring user does not work through the `keyring` crate.** Measured
  against 4.2.0 on macOS, `Entry::new(service, "")` returns
  `Err(Invalid("user", "cannot be empty"))` *before* it touches the credential
  store, so there is no store-level call to fall back to. `SystemTokenProvider`
  shells out instead (`security find-generic-password -s <service> -w` on macOS,
  `secret-tool lookup service <service> username ""` on Linux), and only when the
  user is empty. The macOS path is verified not to prompt for a service that does
  not exist; the Linux path could not be verified from macOS and is best-effort.
- ⛔ **What `glab` stores is not the token.** `glab` uses `zalando/go-keyring`,
  whose macOS backend base64-encodes **every** password before
  `security add-generic-password`. The stored item is
  `go-keyring-base64:` + base64 of the real value (or, historically,
  `go-keyring-encoded:` + hex); for a 62-character `glpat-` token that is 102
  characters. Sending it as a `PRIVATE-TOKEN` header is a **401**, which reads
  exactly like a scope problem and is not. `decode_keyring_envelope` unwraps it
  after every keyring read and again in `token::resolve`, so a
  `command` source pointing straight at `security` works too. Decoding is
  idempotent, and a value that carries a prefix but will not decode is an error
  naming the envelope rather than a pass-through.
- ⛔ **Reading a live keychain item can raise a GUI prompt.** Resolve a token in
  response to a user action, not on a timer, and never in a test. The unit tests
  drive the `TokenProvider` trait with a fake.
- ⚠️ **A token that cannot be an HTTP header value is refused at resolve time**,
  with advice, because reqwest reports it as an unactionable `builder error` on
  every request forever. The usual cause is a `command` source whose program
  prints more than the token.

## Notifications

```rust,no_run
use bridgewatch_core::notify::{NotifyLedger, NotifyKind};

# fn example() {
let mut ledger = NotifyLedger::load(&NotifyLedger::default_path());
// `Poller::with_ledger(path)` does the load and the save for you.
let _ = (&mut ledger, NotifyKind::Deployed);
# }
```

A `Notification` is `{ watch, kind, pipeline_id, title, body, url, key }`.
`title` and `body` are already rendered through the watch's MiniJinja templates;
`url` is the resolved `click` target. Hand them to the OS and do nothing else.

Rules the shell can rely on:

- **A secondary watch never notifies**, and is never even baselined.
- **The first successful tick per watch baselines silently**, recording every key
  it *would* have fired so the next tick reports only genuine changes.
- Dedupe key is `pipeline|kind|sorted job names`: job **names**, so a retry of
  the same failure is not news.
- The ledger is the only thing bridgewatch persists across launches. A corrupt
  one costs one repeated notification, never a crash.

⚠️ MiniJinja follows Jinja2: `default(x)` substitutes only for an *undefined*
value, and `failures | join(', ')` on an empty list is the defined empty string.
The shipped default body passes `default('all green', true)`; without the second
argument a green pipeline notifies with a blank body.

## The verdict script hook

`[verdict].script` (or `script_source`) hands the icon-state rules to a Rhai
script, for every pipeline of every watch. It receives the `PipelineView` as
`pipeline` (also `p`) and returns an icon-state name. The engine is sandboxed:
the module resolver is replaced so `import` cannot read a file, `eval` is
disabled, and operations, call depth and container sizes are capped. A
script that errors or returns something unrecognised yields `unknown` plus an
entry in `Snapshot::errors`, never a confidently wrong colour.

## Testing

Everything above the transport seam is tested against recorded fixtures of real
pipelines (their URLs rewritten to the placeholder `acme-corp/monorepo`) and
fixtures synthesised from them, each saying what it was made from.
`cargo test -p bridgewatch-core` touches no network and no keyring.

- `client::FixtureTransport` replays a recorded directory. It is public because
  `bridgewatch check --fixture` uses it too, so the thing the tests exercise is
  the thing that ships.
- `client::fixture::record` walks one parent pipeline into a new fixture
  directory; `bridgewatch fixture record <id> [--out <dir>]` is its front end, and
  `scripts/record-fixture.sh <id> <name>` does the same walk with `glab api`.
- Each fixture's `expected.json` records its **source** (a real pipeline id, or
  what it was synthesised from and why), and the test asserts that field is
  present.

⛔ **Recordings are put through an allow-list before they are written**, because
fixtures are committed to a public repository and are recorded from a private
one. A raw GitLab payload carries the committer's real name and email address,
the commit message and title, the pushing user's profile (job title, location,
employer, social handles) and the runner that ran it, including a self-hosted
project runner's IP address, system id, tags and free-text description.

`client::fixture::scrub` keeps only `PIPELINE_KEYS` and `JOB_KEYS` and **drops
everything else**, so a field GitLab adds next month cannot leak by default. The
`jq` filter in `scripts/record-fixture.sh` applies the same two lists; nothing
makes them agree automatically, so `fixtures_contain_no_personal_data` asserts
the Rust allow-list against every committed fixture and
`the_jq_filter_names_every_allow_listed_key` checks the script still names every
key. `bridgewatch fixture scrub <dir>` re-applies the allow-list in place, and
`--check` makes it a gate.

⚠️ If the engine ever needs a field that is currently dropped, the guard's
failure names the file, the JSON pointer and the key, and adding it means
editing **both** lists.
