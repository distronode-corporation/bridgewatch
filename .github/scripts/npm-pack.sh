#!/usr/bin/env bash
# Pack the staged packaging/npm and check the result.
#
#   bash .github/scripts/npm-pack.sh <version> <out-dir>
#
# Writes <out-dir>/bridgewatch-<version>.tgz. Used by release.yml's `npm` job, which then
# publishes that file, and by ci.yml's `npm-rehearsal`, which dry-runs the publish.
#
# The tarball that is published is the one these checks read: its file list is exactly
# the nine files below, and every executable in it is 0755, because a vendored binary
# that lost its mode fails on every user's machine with EACCES.
set -euo pipefail

version="${1:?version}"
out_dir="${2:?out dir}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
(cd "$repo/packaging/npm" && npm pack --pack-destination "$out_dir" >&2)
tgz="$out_dir/bridgewatch-$version.tgz"
[ -s "$tgz" ] || { echo "::error::npm pack wrote no $tgz"; exit 1; }

expected="$(printf 'package/%s\n' package.json README.md LICENSE NOTICE \
  THIRD-PARTY-LICENSES-cli.txt bin/bridgewatch.js \
  vendor/universal-apple-darwin/bridgewatch \
  vendor/x86_64-unknown-linux-gnu/bridgewatch \
  vendor/aarch64-unknown-linux-gnu/bridgewatch | sort)"
actual="$(tar -tzf "$tgz" | sort)"
if [ "$expected" != "$actual" ]; then
  echo "::error::the npm package does not hold exactly the expected files"
  diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true
  exit 1
fi
for f in bin/bridgewatch.js vendor/universal-apple-darwin/bridgewatch \
         vendor/x86_64-unknown-linux-gnu/bridgewatch vendor/aarch64-unknown-linux-gnu/bridgewatch; do
  mode="$(tar -tvzf "$tgz" | awk -v f="package/$f" '$NF == f {print $1}')"
  [ "$mode" = "-rwxr-xr-x" ] || { echo "::error::package/$f is ${mode:-missing}, not -rwxr-xr-x"; exit 1; }
done
tar -tvzf "$tgz"
