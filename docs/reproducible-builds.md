# Repeatable builds

What bridgewatch claims about building the same bytes twice, how to check it, and what it does
not claim.

## What is repeatable

- **The CLI on Linux.** Two builds of the `bridgewatch` CLI from the same commit, each from a
  clean `target/` directory, with the same Rust toolchain, in the same checkout directory and
  with the same `CARGO_HOME`, give a byte-identical binary. This is the binary a release ships
  in the Linux CLI tarballs and the npm package.
- **The frontend bundle.** Two `npm run build` runs from the same commit, after the same
  `npm ci`, give a byte-identical `dist/` directory, which is what the app embeds.

`Cargo.lock` and `package-lock.json` pin every dependency, and the release builds use
`--locked`, so both builds compile the same sources. Two things do go into the binary from the
build machine, and they have to match for the bytes to match:

- **The toolchain.** `rust-toolchain.toml` names the stable channel, not a version, so a
  build next month may use a newer `rustc` and give different bytes. Use the same `rustc`
  version for both builds (`rustc --version`).
- **Paths.** The compiler records source paths in panic messages, including the paths of
  dependencies under `CARGO_HOME`, so a different `CARGO_HOME` (or checkout directory) can
  give different bytes.

## Checking it yourself

On Linux, with Rust, a C linker and Node.js installed (the CLI does not link GTK or
WebKitGTK, and its keyring backend is pure Rust, so it needs none of the app's system
packages):

```
git clone https://github.com/distronode-corporation/bridgewatch
cd bridgewatch
CARGO_PROFILE_RELEASE_STRIP=true cargo build --release --locked -p bridgewatch-cli
cp target/release/bridgewatch /tmp/bridgewatch.first
cargo clean
CARGO_PROFILE_RELEASE_STRIP=true cargo build --release --locked -p bridgewatch-cli
cmp /tmp/bridgewatch.first target/release/bridgewatch && echo same

npm ci
npm run build && (cd dist && find . -type f -print0 | sort -z | xargs -0 sha256sum) > /tmp/dist.first
rm -rf dist
npm run build && (cd dist && find . -type f -print0 | sort -z | xargs -0 sha256sum) > /tmp/dist.second
diff /tmp/dist.first /tmp/dist.second && echo same
```

`CARGO_PROFILE_RELEASE_STRIP=true` is how the release builds the CLI (the shared
[build-cli action](../.github/actions/build-cli/action.yml)); the release profile in
`Cargo.toml` already strips, so it changes nothing here unless your environment sets
another value.

## What is not claimed

- **The app bundles.** The `.dmg`, `.deb`, `.rpm` and `.AppImage` are not claimed to be
  bit-for-bit reproducible. They carry build timestamps, the release builds of the app embed
  the build's commit label and crash-report address, and the macOS ones carry a code
  signature and a notarisation ticket that only the release workflow can produce.
- **Rebuilding a published release on your own machine.** Matching a released CLI binary
  needs the same `rustc` version and the same paths as the GitHub-hosted runner that built
  it. The release workflow prints the `rustc` version in its log, but it does not publish it
  with the release. To check where a released file came from, use its build provenance
  attestation instead ([Verifying a release](verifying-releases.md)).
- **The CLI on macOS.** The universal macOS CLI is signed and notarised after it is built,
  so the shipped file is not what a local build produces.
