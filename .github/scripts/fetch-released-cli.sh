#!/usr/bin/env bash
# Download a RELEASED bridgewatch CLI and check it against a pinned digest.
#
#   bash .github/scripts/fetch-released-cli.sh <target> <out-dir>
#
# Prints the path of the extracted binary. Used by the bound-token probe
# (.github/scripts/bound-token-probe.sh), which needs the CLI a 1.x user has
# installed, not one built from this tree: the claim under test is what that
# shipped binary does with an entry 2.0 wrote.
#
# ⛔ The digests are pinned here, not fetched. 1.3.0 publishes no SHA256SUMS
# file; these are the subject digests of its provenance attestation
# (bridgewatch_1.3.0.intoto.jsonl, SLSA v1) and equal GitHub's own asset
# digests. A digest fetched next to the file it checks proves only that the
# two downloads agree. The asset ids are pinned for the API's asset URL, which
# answered when the release page's URL did not (download-release-asset.sh); an
# id that ever stopped matching only costs that source, and the digest still
# decides what is accepted.
set -euo pipefail

target="${1:?target}"
out_dir="${2:?out dir}"
version="1.3.0"

case "$target" in
  universal-apple-darwin) sha256="936112ed8912a0f62f2b4d87f324a400aafdf9da586c498e2f04208a3d732d15"; asset=718830385 ;;
  x86_64-unknown-linux-gnu) sha256="9193d7d780b0d5cbce7aa70e91426ab3f4de3e5390d542f3a54717b7f40e474a"; asset=718830384 ;;
  aarch64-unknown-linux-gnu) sha256="37640bdcfec6c4c4698c36cdd7b21f31ba0948de1d7f0f4d719d77e4ea9a806b"; asset=718830386 ;;
  *) echo "::error::no released $version CLI for target $target" >&2; exit 1 ;;
esac

name="bridgewatch-cli_${version}_${target}"
url="https://github.com/distronode-corporation/bridgewatch/releases/download/v${version}/${name}.tar.gz"
api_url="https://api.github.com/repos/distronode-corporation/bridgewatch/releases/assets/${asset}"
mkdir -p "$out_dir"
tarball="$out_dir/$name.tar.gz"
# No token: the release is public, and the download needs none. The API's asset
# URL first, the release page's second; transient errors are retried within
# about two minutes in all, and a 404 from both fails with that status.
bash "$(dirname "${BASH_SOURCE[0]}")/download-release-asset.sh" "$tarball" "$api_url" "$url"

if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$tarball" | cut -d' ' -f1)"
else
  got="$(shasum -a 256 "$tarball" | cut -d' ' -f1)"
fi
if [ "$got" != "$sha256" ]; then
  echo "::error::$name.tar.gz has sha256 $got, expected $sha256" >&2
  exit 1
fi
echo "$name.tar.gz matches its pinned sha256 $sha256" >&2

tar -xzf "$tarball" -C "$out_dir"
bin="$out_dir/$name/bridgewatch"
[ -x "$bin" ] || { echo "::error::$name.tar.gz holds no executable bridgewatch" >&2; exit 1; }
reported="$("$bin" --version)"
[ "$reported" = "bridgewatch $version" ] \
  || { echo "::error::the released binary reports \"$reported\", expected \"bridgewatch $version\"" >&2; exit 1; }
echo "$reported, from $url" >&2
printf '%s\n' "$bin"
