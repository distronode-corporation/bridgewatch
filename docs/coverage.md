# Test coverage

How much of the code the automated tests run, measured with the project's own tooling. CI
holds each part to a floor (see "Tests" in [CONTRIBUTING.md](../CONTRIBUTING.md)); this page
records the last full measurement, with the command and the date, so the numbers can be
checked and repeated.

## Last measurement: 2026-11-22

Measured on `main` at `b8e0fd4` (Rust 1.100.0, cargo-llvm-cov 0.9.2, Node 24, vitest 5.1.0).

### Rust, line coverage per crate

Lines under each crate's `src/`, counted as `scripts/check-coverage.py` counts them.

| Crate | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: |
| bridgewatch-core | 10903 | 9805 | 89.92% | 89% |
| bridgewatch-cli | 3047 | 2746 | 90.12% | 90% |
| bridgewatch-app | not measured | | | 61% |
| **core and cli together** | **13950** | **12551** | **89.97%** | |

bridgewatch-app (`src-tauri/`) was not measured in this run: it links WebKitGTK, and the
machine had no WebKitGTK development files. Its floor of 61% is the 61.31% CI's `coverage`
job measured when the floor was set (`coverage-floors.toml`). CI's `coverage` job measures it
on every pull request.

Command for the two crates measured here:

```
cargo llvm-cov --locked -p bridgewatch-core -p bridgewatch-cli \
  --json --summary-only --output-path coverage.json
```

The figures above are read from that report's per-crate totals. `scripts/check-coverage.py`
checks every crate in `coverage-floors.toml`, so on this partial report it reports
bridgewatch-app missing and exits 1. It needs a whole-workspace report, which is what CI's
`coverage` job makes, with WebKitGTK installed:

```
cargo llvm-cov --locked --workspace --json --summary-only --output-path coverage.json
python3 scripts/check-coverage.py coverage.json
```

### Frontend, every file under `src/`

Re-measured on 2026-11-26, after the unused vendored ui components were deleted and the
Svelte compiler's template fallback was left out of the report (see "Tests" in
[CONTRIBUTING.md](../CONTRIBUTING.md)).

| Measure | Covered | Total | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: |
| Lines | 3333 | 3468 | 96.10% | 96% |
| Statements | 4967 | 5257 | 94.48% | 94% |
| Branches | 2300 | 2586 | 88.94% | 88% |
| Functions | 1443 | 1526 | 94.56% | 94% |

Command: `npm ci && npx vitest run --coverage` (42 test files, 672 tests).

### Total

Rust (core and cli) and the frontend together: 15884 of 17418 lines, 91.19%. This leaves out
bridgewatch-app, so it is not the whole workspace.

## CI, the whole workspace: 2026-11-27, pull request #141 (the Tauri shell)

The `coverage` job on pull request #141 at `dbb5b91`
([run 45996213756](https://github.com/distronode-corporation/bridgewatch/actions/runs/45996213756)),
on `main` at `75033f8` plus that pull request, printed, from `scripts/check-coverage.py`:

| Crate | Files | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: | ---: |
| bridgewatch-core | 40 | 11003 | 10449 | 94.96% | 93% |
| bridgewatch-cli | 4 | 3047 | 2746 | 90.12% | 90% |
| bridgewatch-app | 18 | 6001 | 4253 | 70.87% | 61%, now 70% |
| **all three crates** | **62** | **20051** | **17448** | **87.02%** | |

bridgewatch-app rose from 61.07% (3345 of 5477). The line total grew because the new tests
live in the same files as the code (a binary crate has no separate test target to put them
in) and the summary counts test lines too. The branch figure is not on this table: `main`
had no branches job when this ran (it arrives with #138).

### bridgewatch-app: what the tests do not reach, per file

The shell is tested as far as tests and inert seams reach. Tauri's mock runtime (its `test`
feature, in dev-dependencies only) runs the commands that take only the managed state, and
the wizard's instance steps and `links::open`, which are generic over the runtime for this
(the app passes its own handle, so nothing it does changes). A local server stands in for an
instance that refuses the token. What is left needs a running desktop app or an OS service;
[#143](https://github.com/distronode-corporation/bridgewatch/issues/143) tracks it.

A line count is the job's uncovered lines in that file. Throughout, a `#[tauri::command]`
line is the IPC glue the macro writes, which runs only when a webview invokes the command.

| File | Uncovered | Why |
| --- | ---: | --- |
| `main.rs` | 198 | The Tauri builder, the setup hook, plugin registration, single-instance and wake handling: all run only inside `tauri::Builder::run`, which needs a display. |
| `commands.rs` | 425 | Commands that take an `AppHandle<Wry>`: they open, resize and hide windows, emit to the webview, change the OS login item, and reload through the poller and the tray. The state-only commands and the pure helpers are tested. |
| `poller.rs` | 232 | The background poll loop's spawn, tick and reload, and what each tick draws: the tray icon and tooltip, notifications, and the `snapshot` event to the windows. The memo, gates and pure drawing helpers are tested. |
| `oauth.rs` | 232 | The browser sign-in flows: the loopback redirect, opening the browser, the device-code polling that ends in the credential store, and their window events. |
| `tray.rs` | 229 | Building the tray icon and menu, its click and menu events, the paused and launch-at-login check items. The CI runner has no system tray. |
| `wizard.rs` | 236 | `wizard_save` (the write, the credential store and the reload through the running app), `wizard_skip`, `open_wizard`, `show_wizard` and `hide_wizard` (windows), `wizard_detect_cli_token` (asks the real keyring), and the lists and suggestions past a token the instance accepts. `failure` on a blocking task that did not finish is reached only if `client_for` panics. |
| `windows.rs` | 99 | Creating, placing, focusing and styling the popover, Settings and wizard windows. |
| `hotkey.rs` | 61 | Registering and acting on the global shortcut, which needs the OS shortcut service and a key press. |
| `state.rs` | 13 | `needs_approval` once a credential prompt was refused, which takes a person saying no to the OS prompt; `first_run` against the real default path. |
| `config.rs` | 4 | The file watcher's own error callback and a watcher that cannot be created: both are `notify` failures from the OS. And the last arm of `Validation::of`, a `ConfigError::Io`, which `parse_str` never returns (only the core's file read makes one). |
| `jobs.rs` | 4 | The command glue, and the success ends of a job's log and a retry, which need an instance that accepts the token. |
| `links.rs` | 4 | Opening an allowed link in the default browser. The refusal is tested. |
| `logging.rs` | 3 | A rolling file appender that cannot be built in a folder that exists, no home directory at all, and a level reload the subscriber refuses. |
| `crash.rs`, `guard.rs`, `reorder.rs` | 1 each | The summary counts one uncovered line in each, but the job's list of missing lines names none in these files, so the line is not identified. The summary counts each function by one compiled copy, and `crash.rs` is compiled into three test binaries, so the two counts can differ. |

## CI, the whole workspace: 2026-11-27, pull request #136

The `coverage` job on pull request #136 at `90ebebd`
([run 45974911891](https://github.com/distronode-corporation/bridgewatch/actions/runs/45974911891)),
with #137 merged into it, printed, from `scripts/check-coverage.py`:

| Crate | Files | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: | ---: |
| bridgewatch-core | 40 | 11003 | 10448 | 94.95% | 93% |
| bridgewatch-cli | 4 | 3047 | 2746 | 90.12% | 90% |
| bridgewatch-app | 18 | 5477 | 3345 | 61.07% | 61% |
| **all three crates** | **62** | **19527** | **16539** | **84.69%** | |

## CI, the whole workspace: 2026-11-27, pull request #132

The `coverage` job on pull request #132 at `64689ca`
([run 45955220679](https://github.com/distronode-corporation/bridgewatch/actions/runs/45955220679))
printed, from `scripts/check-coverage.py`:

| Crate | Files | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: | ---: |
| bridgewatch-core | 40 | 10874 | 10033 | 92.26% | 92% |
| bridgewatch-cli | 4 | 3047 | 2745 | 90.08% | 90% |
| bridgewatch-app | 18 | 5477 | 3345 | 61.07% | 61% |
| **all three crates** | **62** | **19398** | **16123** | **83.12%** | |

The core figure rose with that pull request's tests. The cli figure is one line below the run
below with no cli change, so one cli line is reached on some runs and not others.

## CI, the whole workspace: 2026-11-22 (before #132)

CI's `coverage` job measures all three crates, bridgewatch-app included, because its runner
has WebKitGTK. Its run on `main` at `69597c0`
([run 44983580934](https://github.com/distronode-corporation/bridgewatch/actions/runs/44983580934))
printed, from `scripts/check-coverage.py`:

| Crate | Files | Lines | Covered | Coverage | Floor |
| --- | ---: | ---: | ---: | ---: | ---: |
| bridgewatch-core | 40 | 10903 | 9811 | 89.98% | 89% |
| bridgewatch-cli | 4 | 3047 | 2746 | 90.12% | 90% |
| bridgewatch-app | 18 | 5477 | 3345 | 61.07% | 61% |
| **all three crates** | **62** | **19427** | **15902** | **81.85%** | |

With the frontend lines from the measurement above (3333 of 3468), the whole project is 19235
of 22895 lines, 84.01%. The job's log is kept for GitHub's log retention period; this table is
the record that lasts.
