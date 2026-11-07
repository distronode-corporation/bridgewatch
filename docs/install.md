# Installing bridgewatch

## Platform support

| Platform | Status |
| --- | --- |
| macOS 10.15 or newer, Apple Silicon and Intel | Supported. Release builds are signed and notarised, see below. |
| Linux x86_64 and arm64, glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or newer, and equivalents) | Supported. |
| Windows | Not targeted. |

Linux release builds are made on Ubuntu 22.04, on both x86_64 and arm64 (the oldest
build machines GitHub offers). glibc is forward-compatible but not backward-compatible, so
that is the floor: the `.deb`, `.rpm` and `.AppImage` need glibc 2.35 or newer, and do
not start on older systems.

On Linux the tray is a StatusNotifierItem through `libayatana-appindicator`. GNOME has no
built-in host for that protocol, so on GNOME you need the AppIndicator/KStatusNotifierItem
extension enabled or the icon never appears. KDE, Cinnamon and XFCE work as they are.

## Install

**The app on macOS, with Homebrew:**

```
brew install --cask distronode-corporation/tap/bridgewatch
```

The cask installs the `.dmg` the Releases page carries. From 1.3.0 the `.dmg` itself is
notarised and stapled too, not only the `.app` inside it.

**The CLI, with npm**, on macOS and on Linux with glibc 2.35 or newer, x64 and arm64:

```
npm install -g bridgewatch
npx bridgewatch check        # or run it once without installing
```

The package needs Node.js 18 or newer and carries the native binary for each platform; a
small launcher picks the right one. On musl, or a glibc older than 2.35, it refuses to
start and points to the downloads below. See [CLI](cli.md) for what it does.

**Everything else** is on the [Releases page](https://github.com/distronode-corporation/bridgewatch/releases).
A release carries these ten assets:

| File | For |
| --- | --- |
| `bridgewatch_<version>_universal.dmg` | macOS, Apple Silicon and Intel |
| `bridgewatch_<version>_amd64.deb` | Debian and Ubuntu, x86_64 |
| `bridgewatch_<version>_arm64.deb` | Debian and Ubuntu, arm64 |
| `bridgewatch-<version>-1.x86_64.rpm` | Fedora and openSUSE, x86_64 |
| `bridgewatch-<version>-1.aarch64.rpm` | Fedora and openSUSE, arm64 |
| `bridgewatch_<version>_amd64.AppImage` | Other glibc 2.35+ Linux, x86_64 |
| `bridgewatch_<version>_aarch64.AppImage` | Other glibc 2.35+ Linux, arm64 |
| `bridgewatch-cli_<version>_universal-apple-darwin.tar.gz` | The CLI, macOS, Apple Silicon and Intel |
| `bridgewatch-cli_<version>_x86_64-unknown-linux-gnu.tar.gz` | The CLI, glibc 2.35+ Linux, x86_64 |
| `bridgewatch-cli_<version>_aarch64-unknown-linux-gnu.tar.gz` | The CLI, glibc 2.35+ Linux, arm64 |

The `.dmg` and the macOS CLI are universal: one download runs natively on both Apple
Silicon and Intel Macs. The Linux app file names follow each format's own architecture
spelling (`amd64` and `arm64` for Debian, `x86_64` and `aarch64` for RPM); the CLI
tarballs use Rust's target names. Every asset carries a GitHub build provenance
attestation:

```
gh attestation verify bridgewatch_2.0.0_amd64.deb --repo distronode-corporation/bridgewatch
```

Every release also carries the attestation as
`bridgewatch_<version>.intoto.jsonl`, so the same check works offline with
`--bundle bridgewatch_<version>.intoto.jsonl` added. Every bundle carries `LICENSE`,
`NOTICE` and the licence texts of the Rust crates and npm packages built into it
(`THIRD-PARTY-LICENSES-rust.txt`, `THIRD-PARTY-LICENSES-npm.txt`): in
`/usr/share/doc/bridgewatch/` for the `.deb`, `/usr/share/licenses/bridgewatch/` for
the `.rpm`, and among the app's resources in the `.dmg` and the `.AppImage`.

Each CLI tarball holds the `bridgewatch` binary, `LICENSE`, `NOTICE` and
`THIRD-PARTY-LICENSES-cli.txt` (the licence texts of the Rust crates built into the CLI).
To install one by hand:

```
tar -xzf bridgewatch-cli_<version>_x86_64-unknown-linux-gnu.tar.gz
sudo install -m 0755 bridgewatch /usr/local/bin/
```

The app bundles do not carry the CLI. To build it from source instead, see [CLI](cli.md).

### macOS: signed and notarised

The `.app` inside the `.dmg` is signed with the Developer ID **Distronode Corporation
(R935BA6767)** and notarised by Apple, with the ticket stapled, so it opens like any other
downloaded app. The `.dmg` itself is notarised and stapled too. The CLI binary in its
tarball is signed with the same Developer ID and notarised; a bare binary cannot carry a
stapled ticket, so Gatekeeper looks its ticket up online. To check a copy yourself:

```
codesign --verify --deep --strict /Applications/bridgewatch.app
spctl --assess --type execute --verbose /Applications/bridgewatch.app
spctl --assess --type open --context context:primary-signature --verbose bridgewatch_<version>_universal.dmg
```

The second and third commands should end with `source=Notarized Developer ID`.

### Linux: .deb

```
sudo apt install ./bridgewatch_2.0.0_amd64.deb
```

On arm64, use `bridgewatch_2.0.0_arm64.deb`. The package depends on `libsecret-tools`,
because bridgewatch runs `secret-tool` to read a keyring item that has an empty user name,
which is how glab stores its token and how gh stores its active account's (see
[Tokens](tokens.md)).

### Linux: .rpm

```
sudo dnf install ./bridgewatch-2.0.0-1.x86_64.rpm      # Fedora
sudo zypper install ./bridgewatch-2.0.0-1.x86_64.rpm   # openSUSE
```

On arm64, use `bridgewatch-2.0.0-1.aarch64.rpm`. The package requires the file
`/usr/bin/secret-tool` rather than a package name, for the same reason as the `.deb`: the
package that ships it is `libsecret` on Fedora but `secret-tool` on openSUSE, and
a file dependency resolves to the right one on each. The package is not GPG-signed; the
attestation above is how to check where it came from.

### Linux: AppImage

```
chmod +x bridgewatch_2.0.0_amd64.AppImage
./bridgewatch_2.0.0_amd64.AppImage
```

On arm64, use `bridgewatch_2.0.0_aarch64.AppImage`. An AppImage declares no dependencies.
If you reuse glab's or gh's token, install `secret-tool` yourself (`sudo apt install
libsecret-tools` on Debian and Ubuntu, `sudo dnf install libsecret` on Fedora). A token stored in
bridgewatch's own keyring entry goes through the Secret Service over D-Bus and does not
need it; either way a Secret Service (GNOME Keyring, KWallet) has to be running.

## What guards the code and the release path

On every push to `main` unless the item says otherwise:

- CodeQL (Rust, TypeScript, Actions) and [zizmor](https://docs.zizmor.sh) over the
  workflows, both reporting to code scanning.
- `cargo deny` against [deny.toml](../deny.toml): RustSec advisories, a permissive-only
  licence allow-list, crates.io as the only source. Tolerated advisories are listed
  there with their reason, and mirrored in [osv-scanner.toml](../osv-scanner.toml) for
  Scorecard's Vulnerabilities check.
- A weekly re-run of `cargo deny` and of `npm audit --audit-level=high`, because an
  advisory lands against code that did not change and so no push is coming to catch it.
  The npm side audits the whole tree, dev dependencies included: this is a Vite app, so
  they are what gets bundled into the shipped frontend.
- Dependency review on every pull request, Dependabot for Cargo, npm and the pinned
  action SHAs, and a weekly [OpenSSF Scorecard](https://scorecard.dev/viewer/?uri=github.com/distronode-corporation/bridgewatch).
- Dependabot's patch and minor updates merge themselves, but only once the nine checks
  the `main` ruleset requires have passed (all three rust legs, including
  `rust (ubuntu-22.04-arm)`, plus msrv, repo, frontend, cargo-deny, zizmor and
  dependency-review). A major waits for a person.
- Release builds run in a `release` environment that only a `v*` tag can reach and that
  holds the macOS signing identity as its own environment secrets, so no branch and no
  pull request can read it. Release tags are protected by a ruleset against moves and
  deletion (repository administrators can bypass it).
