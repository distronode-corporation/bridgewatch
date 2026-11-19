# Verifying a release

What a bridgewatch release is signed with, how to check each kind of asset, and what is not
signed. Everything here is done by
[`.github/workflows/release.yml`](../.github/workflows/release.yml) when a `vX.Y.Z` tag is
pushed; no person signs anything by hand.

## What is signed, and by what

| What | Signed with | How to check |
| --- | --- | --- |
| Every release asset: the `.dmg`, both `.deb`, both `.rpm`, both `.AppImage` and the three CLI tarballs | A GitHub build provenance attestation, signed through Sigstore under the release workflow's own identity | `gh attestation verify`, below |
| The macOS app (`bridgewatch.app` inside the `.dmg`) | Apple Developer ID **Distronode Corporation (R935BA6767)**, hardened runtime, notarised by Apple with the ticket stapled | `codesign` and `spctl`, below |
| The `.dmg` itself | Notarised by Apple, with the ticket stapled | `spctl`, below |
| The macOS CLI binary (in its tarball and in the npm package) | The same Developer ID, hardened runtime, notarised (a bare binary cannot carry a stapled ticket, so Gatekeeper looks it up online) | `codesign`, below |
| The `bridgewatch` npm package | npm provenance (SLSA provenance v1), published through npm trusted publishing from the same workflow | `npm audit signatures`, below |

The release workflow checks its own signatures before anything is public. The macOS build
fails unless the app inside the `.dmg` and the CLI carry the Developer ID signature and the
`.dmg` is assessed as notarised. After the attestation is made, the workflow runs
`gh attestation verify` on every asset, both against GitHub's attestation store and against
the bundle file it is about to attach, and the release stays a draft until that passes. The
`npm` job checks each CLI tarball against the digest GitHub recorded and verifies its
attestation, pinned to `release.yml` at the release's tag and commit, before it publishes the
package.

## The build provenance attestation

Each release attaches one Sigstore bundle, `bridgewatch_<version>.intoto.jsonl`, whose
subjects are all ten assets. It records that the file was built by
`.github/workflows/release.yml` in this repository, from the release's tag, on a
GitHub-hosted runner.

There is no long-lived signing key to download. Sigstore signs with a short-lived
certificate issued to the workflow run's OIDC identity, and what you check is that identity:
this repository, this workflow file and this tag. With the [GitHub CLI](https://cli.github.com):

```
gh attestation verify bridgewatch_2.1.0_amd64.deb \
  --repo distronode-corporation/bridgewatch \
  --signer-workflow distronode-corporation/bridgewatch/.github/workflows/release.yml \
  --source-ref refs/tags/v2.1.0 \
  --deny-self-hosted-runners
```

Exit status 0 means the file's SHA-256 digest is a subject of an attestation that was signed
by that workflow, from that tag, on a GitHub-hosted runner. A file built anywhere else, by any
other workflow or from any other ref, fails.

To check against the bundle attached to the release, instead of asking GitHub's attestation
API, download `bridgewatch_<version>.intoto.jsonl` too and add
`--bundle bridgewatch_<version>.intoto.jsonl`. For a machine with no network at all, first
save Sigstore's trusted root on a machine that has one, with
`gh attestation trusted-root > trusted-root.jsonl`, and add
`--custom-trusted-root trusted-root.jsonl` as well.

GitHub also records each asset's SHA-256 digest when it is uploaded, so a download can be
compared with what the workflow uploaded:

```
gh api repos/distronode-corporation/bridgewatch/releases/tags/v2.1.0 \
  --jq '.assets[] | "\(.name) \(.digest)"'
sha256sum bridgewatch_2.1.0_amd64.deb
```

## macOS code signing and notarisation

```
codesign --verify --deep --strict /Applications/bridgewatch.app
spctl --assess --type execute --verbose /Applications/bridgewatch.app
spctl --assess --type open --context context:primary-signature --verbose bridgewatch_2.1.0_universal.dmg
codesign -dv --verbose=2 /Applications/bridgewatch.app 2>&1 | grep -E 'Authority|TeamIdentifier'
codesign --verify --strict --verbose=2 bridgewatch            # the CLI from its tarball
codesign -dv --verbose=2 bridgewatch 2>&1 | grep -E 'Authority|TeamIdentifier'
```

The two `spctl` commands should end with `source=Notarized Developer ID`. A copy signed and
notarised by any other Apple developer would pass those too, so the two `codesign -dv` lines
are the check that it is this project's: each should show
`Authority=Developer ID Application: Distronode Corporation (R935BA6767)` and
`TeamIdentifier=R935BA6767`. The signing certificate is issued by Apple, and
macOS checks it against Apple's own roots, so there is no key to obtain from this project.

## The npm package

```
mkdir bridgewatch-audit && cd bridgewatch-audit
npm init -y
npm install --ignore-scripts bridgewatch@2.1.0     # the version you want to check
npm audit signatures
```

`npm audit signatures` audits the packages of the project it runs in, not global installs,
hence the throwaway project. It checks the registry signature of every package there and the
provenance attestation of those that have one; for `bridgewatch` it should report one verified
registry signature and one verified attestation. The provenance names
this repository and `release.yml` as the source and the build. The package's three binaries are
the same bytes as the CLI tarballs on the GitHub Release: the `npm` job downloads them from the
public release and verifies their attestation before it packs them.

## What is not signed

- **Git tags.** Release tags are annotated but not cryptographically signed. A ruleset
  refuses moving or deleting a `v*` tag (repository administrators can bypass it), and the
  attestation above names the tag a build came from.
- **The Linux packages carry no signature of their own.** The `.deb` and `.rpm` are not
  GPG-signed, and the `.AppImage` has no embedded signature. The attestation is how to check
  where they came from.
- **The Linux CLI binaries** are not signed beyond the attestation over their tarballs.

## Where the signing keys are

The Sigstore certificate exists only for the length of the workflow run that signs, and the
npm publish token is minted by npm for one publish. The Apple Developer ID certificate and the
notarisation key are held as secrets of this repository's `release` environment, which only a
`v*` tag can reach: no branch, pull request or person's machine holds them, and they are not on
the release download pages. See [GOVERNANCE.md](../GOVERNANCE.md#continuity).
