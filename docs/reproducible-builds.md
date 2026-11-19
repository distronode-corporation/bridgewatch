# Repeatable builds

What bridgewatch claims about building the same bytes twice, how to check it, and what it does
not claim.

## What is repeatable

- **The CLI on Linux x86_64.** Two builds of the `bridgewatch` CLI from the same commit, each
  from a clean `target/` directory, on the same machine with the inputs below unchanged, give
  a byte-identical binary. This is the binary a release ships in the x86_64 Linux CLI tarball
  and the npm package. The arm64 build is expected to behave the same way but is not
  compared.
- **The frontend bundle.** Two `npm run build` runs from the same commit, after the same
  `npm ci`, give a byte-identical `dist/` directory, which is what the app embeds.

`Cargo.lock` and `package-lock.json` pin every dependency, and the release builds use
`--locked`, so both builds compile the same sources. Four things do go into the binary from
the build machine, and they have to match for the bytes to match:

- **The toolchain.** `rust-toolchain.toml` names the stable channel, not a version, so a
  build next month may use a newer `rustc` and give different bytes. Use the same `rustc`
  version for both builds (`rustc --version`).
- **The C toolchain.** The TLS library's cryptography (`aws-lc-sys`, under rustls) is C,
  compiled during the build by the machine's C compiler, and the system linker links the
  binary. A different compiler or linker version can give different bytes.
- **Paths.** The compiler records source paths in panic messages, including the paths of
  dependencies under `CARGO_HOME`, so a different `CARGO_HOME` (or checkout directory) can
  give different bytes.
- **Build settings from the environment.** Cargo reads `CARGO_PROFILE_RELEASE_*` variables as
  overrides of the committed `[profile.release]`, and `RUSTFLAGS` and `CARGO_BUILD_RUSTFLAGS`
  as extra compiler flags, so a different `CARGO_PROFILE_RELEASE_OPT_LEVEL` or `RUSTFLAGS` on
  one build gives different bytes. Leave them unset, apart from the
  `CARGO_PROFILE_RELEASE_STRIP=true` the commands below set, or set them the same for both
  builds.

## What CI checks

The `repeatable` job in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on every
push to `main` and every pull request, on an Ubuntu 22.04 x86_64 runner. It builds the CLI with
the same [build-cli action](../.github/actions/build-cli/action.yml) the release uses, runs
`cargo clean`, builds it again and compares the two binaries byte for byte; then, after one
`npm ci`, it runs `npm run build` twice, removing `dist/` in between, and compares the SHA-256
of every file. Either difference fails the job. Both builds of a pair run on the same runner
with the same toolchain and paths, which is the condition above. The job is not yet a
required check of the `main` ruleset. It compares nothing on arm64 or macOS, and no app
bundle.

## Checking it yourself

On Linux x86_64, with Rust, a C compiler and linker (`build-essential` on Debian and Ubuntu)
and Node.js installed (the CLI does not link GTK or WebKitGTK, and its keyring backend is
pure Rust, so it needs none of the app's system packages):

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
