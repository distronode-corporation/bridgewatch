#!/usr/bin/env bash
# Package one CLI binary as the release asset bridgewatch-cli_<version>_<target>.tar.gz.
#
#   bash .github/scripts/package-cli.sh <version> <target> <binary> <third-party-licenses> <out-dir>
#
# Used by release.yml's build legs ("Package the CLI") and by ci.yml's `npm-rehearsal`,
# so the tarball a pull request rehearses is the one a tag ships.
#
# ⛔ THE TARBALL CARRIES THE MODE. upload-artifact drops the executable bit, so the
# binary is never shipped bare, and this script sets 0755 itself (`install -m`) whatever
# mode it was handed. The tarball holds one top-level directory,
# `bridgewatch-cli_<version>_<target>/`, with the binary, LICENSE, NOTICE and the CLI's
# third-party licence texts, and its exact listing and the binary's mode are asserted.
set -euo pipefail

version="${1:?version}"
target="${2:?target}"
binary="${3:?binary}"
third_party="${4:?THIRD-PARTY-LICENSES-cli.txt}"
out_dir="${5:?out dir}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
name="bridgewatch-cli_${version}_${target}"
for f in "$binary" "$third_party" "$repo/LICENSE" "$repo/NOTICE"; do
  [ -s "$f" ] || { echo "::error::$f is missing or empty"; exit 1; }
done

work="$(mktemp -d "${RUNNER_TEMP:?}/cli-package.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/$name" "$out_dir"
install -m 0755 "$binary" "$work/$name/bridgewatch"
install -m 0644 "$repo/LICENSE" "$repo/NOTICE" "$work/$name/"
install -m 0644 "$third_party" "$work/$name/THIRD-PARTY-LICENSES-cli.txt"

tarball="$out_dir/$name.tar.gz"
if tar --version 2>/dev/null | grep -q 'GNU tar'; then
  tar --owner=0 --group=0 --numeric-owner -czf "$tarball" -C "$work" "$name"
else
  # bsdtar (macOS); COPYFILE_DISABLE keeps AppleDouble ._ files out.
  COPYFILE_DISABLE=1 tar --uid 0 --gid 0 --uname root --gname root \
    -czf "$tarball" -C "$work" "$name"
fi

expected="$(printf '%s\n' "$name/" "$name/bridgewatch" "$name/LICENSE" "$name/NOTICE" \
  "$name/THIRD-PARTY-LICENSES-cli.txt" | sort)"
actual="$(tar -tzf "$tarball" | sort)"
if [ "$expected" != "$actual" ]; then
  echo "::error::$tarball does not hold exactly the four expected files"
  diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true
  exit 1
fi
mode="$(tar -tvzf "$tarball" | awk -v f="$name/bridgewatch" '$NF == f {print $1}')"
[ "$mode" = "-rwxr-xr-x" ] || { echo "::error::the CLI in $tarball is ${mode:-missing}, not -rwxr-xr-x"; exit 1; }
ls -l "$tarball"
