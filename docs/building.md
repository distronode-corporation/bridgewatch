# Build from source

Requirements: Rust 1.90 or newer (via [rustup](https://rustup.rs); the repository pins
the stable channel), Node.js 24 (CI's version; `package.json` also accepts 20.19+ and
22.12+), and the platform toolchain.

**macOS:** Xcode Command Line Tools (`xcode-select --install`).

**Ubuntu 22.04 or newer:**

```
sudo apt-get update
sudo apt-get install -y \
  build-essential curl file \
  libwebkit2gtk-4.1-dev \
  libjavascriptcoregtk-4.1-dev \
  libsoup-3.0-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf \
  xdg-utils \
  libsecret-tools
```

`libwebkit2gtk-4.1` (not 4.0) is the Tauri 2 line and `libayatana-appindicator3` puts the
icon in the tray; without the latter the app runs with no icon and no window, which looks
like a crash. `libsecret-tools` is a runtime dependency (`secret-tool`), not a build one.
No OpenSSL is needed: HTTPS is rustls.

Then:

```
git clone https://github.com/distronode-corporation/bridgewatch
cd bridgewatch
npm ci
npm run tauri build
```

Bundles land in `target/release/bundle/` at the repository root. On a Mac with both
`aarch64-apple-darwin` and `x86_64-apple-darwin` installed through rustup,
`npm run tauri build -- --target universal-apple-darwin` makes the universal `.dmg` the
release ships, under `target/universal-apple-darwin/release/bundle/`. For development, see
[CONTRIBUTING.md](../CONTRIBUTING.md).

A plain `npm run tauri build` bundles `LICENSE` and `NOTICE`. To bundle the third-party
licence texts as a release does (which you should, if you distribute the build),
generate them first and pass the overlay config; this needs
[cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9:

```
mkdir -p target/licenses
cargo about generate --locked --fail --manifest-path src-tauri/Cargo.toml -c about.toml \
  -o target/licenses/THIRD-PARTY-LICENSES-rust.txt licenses/about.hbs
node scripts/third-party-licenses-npm.mjs target/licenses/THIRD-PARTY-LICENSES-npm.txt
npm run tauri build -- --config src-tauri/tauri.licenses.conf.json
```
