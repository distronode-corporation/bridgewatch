# Roadmap

What bridgewatch intends to do over the next year, and what it does not intend to do. It is
taken from what this repository's own documents and issues say is not finished yet, chiefly
[Limitations](docs/limitations.md). It has no dates: an item is done when the change that does
it is merged and released, and it then leaves this list for [CHANGELOG.md](CHANGELOG.md).
Anything here can change, and a change of direction is decided as described in
[GOVERNANCE.md](GOVERNANCE.md). Open an
[issue](https://github.com/distronode-corporation/bridgewatch/issues) or a
[discussion](https://github.com/distronode-corporation/bridgewatch/discussions) to discuss an
item or to propose a new one.

## Planned

- **Off chrono.** The `chrono` crate is unmaintained (RUSTSEC-2026-0543, tolerated in
  `deny.toml` with its reason for now). The plan is to move to a maintained date and time
  crate ([issue #106](https://github.com/distronode-corporation/bridgewatch/issues/106)).
- **A global shortcut on Wayland.** `[ui].hotkey` registers through X11 only, so a Wayland
  session gets no global shortcut. The desktop portal's GlobalShortcuts interface, through
  D-Bus, is the route the limitations page names.
- **Merge-request pairing.** A merged-result pipeline is not yet paired with its branch
  pipeline; `merge_request_event` works only as a source like any other.
- **Coverage of the tray app.** CI measures each part against the floors in
  `coverage-floors.toml` (the `coverage` job, which runs on every pull request but is not yet
  a required check) and `vitest.config.ts` (inside the required `frontend` job). The Tauri
  shell (`bridgewatch-app`, 61% of lines) is the furthest behind the core and the CLI (both
  about 90%). The plan is to raise it, and to make `coverage` a required check.
- **Repeatable builds beyond the CLI.** CI's `repeatable` job checks on every push and pull
  request that the Linux x86_64 CLI and the frontend bundle build to the same bytes twice
  ([Repeatable builds](docs/reproducible-builds.md)). The arm64 CLI and the app bundles are not
  compared today ([Installing bridgewatch](docs/install.md#what-guards-the-code-and-the-release-path)).

## Not planned

- **Windows.** Nothing is built or tested there ([Limitations](docs/limitations.md),
  [Installing bridgewatch](docs/install.md)).
- **Push instead of polling.** Neither GitLab nor GitHub offers a desktop app a push
  channel, and webhooks need a public inbound URL, so bridgewatch polls
  ([the job view](docs/app.md#the-job-view-and-what-live-means)).
- **Writes by default.** Retry and play stay opt in (`actions = true`), and they stay the only
  writes to a provider's pipelines that bridgewatch knows ([Retry and play](docs/retry-and-play.md)).

A change to any of these would be proposed as an issue first.
