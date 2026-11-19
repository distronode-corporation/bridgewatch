# Roadmap

The near-term plan for bridgewatch, and what it will not do. The repository has no open issues
today, so the plan is taken from what its own documents say is not finished yet, chiefly
[Limitations](docs/limitations.md). It has no dates: an item is done when the change that does
it is merged and released, and it then leaves this list for [CHANGELOG.md](CHANGELOG.md).
Anything here can change; open an
[issue](https://github.com/distronode-corporation/bridgewatch/issues) or a
[discussion](https://github.com/distronode-corporation/bridgewatch/discussions) to discuss an
item or to propose a new one.

## Near term

- **A global shortcut on Wayland.** `[ui].hotkey` registers through X11 only, so a Wayland
  session gets no global shortcut. The desktop portal's GlobalShortcuts interface, through
  D-Bus, is the route the limitations page names.
- **Merge-request pairing.** A merged-result pipeline is not yet paired with its branch
  pipeline; `merge_request_event` works only as a source like any other.
- **Coverage.** CI holds each part to the floors in `coverage-floors.toml` and
  `vitest.config.ts`, and floors only go up. The Tauri shell (`bridgewatch-app`, 61% of lines
  today) is the furthest behind the core and the CLI (both about 90%).
- **Repeatable builds beyond the CLI.** CI checks that the Linux CLI and the frontend bundle
  build to the same bytes twice. The app bundles are not compared today
  ([Installing bridgewatch](docs/install.md#what-guards-the-code-and-the-release-path)).

## Not planned

- **Windows.** Nothing is built or tested there (docs/limitations.md, docs/install.md).
- **Push instead of polling.** Neither GitLab nor GitHub offers a desktop app a push
  channel, and webhooks need a public inbound URL, so bridgewatch polls
  ([the job view](docs/app.md#the-job-view-and-what-live-means)).
- **Writes by default.** Retry and play stay opt in (`actions = true`), and nothing else
  writes to a provider ([Retry and play](docs/retry-and-play.md)).

A change to any of these would be proposed as an issue first.
