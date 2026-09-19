# Contributing to bridgewatch

## Layout

```
crates/bridgewatch-core/   Rust library: config parsing and editing, token resolution, the
                           GitLab and GitHub clients, the pipeline walk (and GitHub's
                           commit group), the verdict rules, the poller,
                           notifications and the setup wizard's logic. No Tauri dependency,
                           so it compiles and tests without a GUI toolchain.
crates/bridgewatch-cli/    The `bridgewatch` command-line binary, over the core.
src-tauri/                 The Tauri 2 shell (crate `bridgewatch-app`): tray, windows,
                           the IPC commands and the checks behind them, config file I/O.
src/                       Svelte 5 + TypeScript frontend: popover, job view, Settings,
                           setup wizard. UI components are vendored shadcn-svelte under
                           src/lib/components/ui/, themed in src/lib/theme/.
examples/                  The shipped example configs: distronode.toml (GitLab) and
                           github.toml (GitHub).
scripts/                   record-fixture.sh and check-version.py.
```

## Setup

You need Rust 1.88 or newer (via [rustup](https://rustup.rs); `rust-toolchain.toml` pins
the stable channel), Node.js 24, and the platform packages listed under "Build from
source" in [README.md](README.md): the Xcode Command Line Tools on macOS, the WebKitGTK
4.1 and libayatana-appindicator3 development packages (and the rest of that list) on
Ubuntu.

```
npm ci
npm run tauri dev                                   # the GUI, with frontend hot reload
cargo run -p bridgewatch-cli -- check --json         # the CLI against your own config
```

To try the verdict engine with no token and no network, answer from a recorded fixture:

```
cargo run -p bridgewatch-cli -- -c examples/distronode.toml check \
  --fixture crates/bridgewatch-core/tests/fixtures/ca41ab28-deployed-with-failure
```

## The inner loop

```
cargo test -p bridgewatch-core
```

It has no network access, needs no token and runs against recorded fixtures and
scripted responses, which is what makes it usable as a save-and-run loop. If a change
makes this suite need a live GitLab or GitHub, the change is in the wrong place: HTTP is behind the `Transport` trait and
the tests supply their own. Tests never read a real keyring either; token code is
tested through the `TokenProvider` trait with a fake.

For the frontend, `npx vitest run <file>` runs one test file.

## The whole local gate

This is what CI's `rust`, `repo` and `frontend` jobs run:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo check -p bridgewatch-app --locked
python3 scripts/check-version.py            # Cargo.toml, package.json, tauri.conf.json agree
python3 src-tauri/icons/generate.py --check # tray icons match the generator
npm run typecheck                           # tsc
npm run check                               # svelte-check
npx vitest run                              # frontend tests, including the settings parity test
npm run build                               # vite build (compiles Tailwind)
```

CI additionally runs `cargo check` at the declared minimum Rust version (the `msrv`
job) and, on macOS, for `x86_64-apple-darwin`. Clippy is `-D warnings`, so a warning is a
failure. There is no formatting bot; run `cargo fmt --all` yourself. Installers are
built by CI only on request (`workflow_dispatch` with `build_installers`) and by the
release workflow.

## Adding a config key

Every config key has a Settings control and every control a key. Two tests hold that:
`src-tauri/tests/schema_parity.rs` fails when the committed
`src/lib/config.schema.json` no longer matches the core's schema, and
`src/lib/settings/registry.test.ts` fails when a schema path has no control or a control
names no path.

1. Add the field to `crates/bridgewatch-core/src/config/schema.rs`, with a serde default
   and a doc comment (it becomes the schema description).
2. Regenerate the schema:
   `cargo run -p bridgewatch-cli -- config schema > src/lib/config.schema.json`.
3. Add the control to `src/lib/settings/registry.ts`.
4. Add it to `examples/distronode.toml` with a comment saying what it is for, or to
   `examples/github.toml` if it is GitHub-only. Some core tests edit those files and
   check that their comments survive verbatim, so change an existing comment there only
   together with those tests.
5. Document it in the README's configuration reference.

## Fixtures

The verdict rules are tested against real GitLab pipelines, not hand-written ones. A
hand-written fixture encodes what you believe GitLab returns, which is the belief under
test. Two ways to record one:

```
bridgewatch fixture record <pipeline-id> --out crates/bridgewatch-core/tests/fixtures/<name> \
  [--project <id|path>] [--list]
scripts/record-fixture.sh <pipeline-id> <name> [project] [--list]
```

The CLI walks the pipeline through bridgewatch's own client and your config's account and
token. The script does the same walk with `glab api`, so an authenticated glab is all it
needs. Both write the parent pipeline, its jobs, its bridges and every child pipeline's
jobs.

**Recordings are filtered through an allow-list before they are written.** A raw GitLab
payload carries committers' names and email addresses, commit messages, the pushing
user's profile and runner details; fixtures are committed to a public repository, so
only the fields the engine reads are kept (`PIPELINE_KEYS` and `JOB_KEYS` in
`crates/bridgewatch-core/src/client/fixture.rs`, mirrored by the `jq` filter in the
script). The `fixtures_contain_no_personal_data` test checks every committed fixture
against the Rust list, and another test checks the script names every key.
`bridgewatch fixture scrub <dir>...` re-applies the list in place; `--check` reports
without writing.

**GitHub has no recorder yet.** `bridgewatch fixture record` refuses a github account
(exit 64), because a run payload carries author and committer names and email addresses,
the commit message, the actor and runner names, and no allow-list covers those keys yet.
The GitHub client's tests (`crates/bridgewatch-core/tests/github.rs` and
`github_group.rs`) run over a scripted transport with hand-built bodies that hold only the
keys the client decodes, with invented values, and say so where they stand in for a case
no live sample exists for.

What the allow-list keeps is still revealing: job names, stage names, branch names,
commit SHAs, pipeline ids and timestamps. Job names can name components, regions and
vendors. If those are not something you would publish, do not commit the fixture.

Each fixture has a `fixture.json` whose `source` says where it came from (a pipeline, or
what it was synthesised from and why) and an `expected.json` with the verdict. Add the
test that reads a fixture in the same commit, and say in the commit message what the
pipeline actually did. A fixture with no stated expected verdict is a snapshot, not a
test.

## Commits and pull requests

Conventional Commits are not required. What is required is that the message says **why**,
not just what. The diff already says what. A good message names the thing that was
wrong, the evidence, and what would have caught it.

Pull requests run `.github/workflows/ci.yml` and all of it must be green.

## Docs

Every claim in the README must be checkable in the code. If you change behaviour the
README describes (a default, a rule, a CLI flag, an exit code), change the README in the
same pull request. The exit-code table is copied from `bridgewatch --help`, which is
generated from the constants in `crates/bridgewatch-cli/src/main.rs`.

## Reporting bugs

Open an issue with the output of `bridgewatch check --json` (it contains no token) and,
for a verdict disagreement, the pipeline id. A pipeline id is usually enough to record a
fixture and turn the report into a test; say if the project is private.

For anything security-relevant, do not open an issue. See [SECURITY.md](SECURITY.md).

## Tray and app icons

The tray PNGs and the app icon are generated, and CI checks that the committed files
match the generator (stdlib Python only). Regeneration is two steps:

```
python3 src-tauri/icons/generate.py
npx tauri icon src-tauri/icons/icon-source.png
```

The 1024px master is `icon-source.png`, not `icon.png`: `tauri icon` overwrites
`icons/icon.png` with a 512px copy of whatever it was given, so a master under that name
would be destroyed by the command that consumes it.
