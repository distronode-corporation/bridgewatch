#!/usr/bin/env bash
# Stage packaging/npm from the three CLI release tarballs.
#
#   bash .github/scripts/npm-stage.sh <version> <tarball-dir>
#
# Used by release.yml's `npm` job (on the tarballs downloaded from the public release)
# and by ci.yml's `npm-rehearsal` (on tarballs built from the pull request's own CLI
# binaries), so a pull request rehearses exactly what a tag publishes.
#
# Each tarball must hold exactly what package-cli.sh put in it, checked BEFORE anything
# is extracted, so nothing else can land in the package. The binary goes to
# packaging/npm/vendor/<target>/bridgewatch at 0755. The licence files go to the package
# root once: LICENSE and NOTICE from this checkout, and THIRD-PARTY-LICENSES-cli.txt from
# the tarballs, which must all carry the same one. packaging/npm/package.json must
# already say <version>.
set -euo pipefail

version="${1:?version}"
tarballs="${2:?tarball dir}"
targets="universal-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
pkg="$repo/packaging/npm"
have="$(node -p 'require(process.argv[1]).version' "$pkg/package.json")"
if [ "$have" != "$version" ]; then
  echo "::error::packaging/npm/package.json is $have, expected $version."
  exit 1
fi

work="$(mktemp -d "${RUNNER_TEMP:?}/npm-stage.XXXXXX")"
trap 'rm -rf "$work"' EXIT
rm -rf "$pkg/vendor"
first=""
for t in $targets; do
  name="bridgewatch-cli_${version}_${t}"
  tarball="$tarballs/$name.tar.gz"
  [ -s "$tarball" ] || { echo "::error::$tarball is missing"; exit 1; }
  expected="$(printf '%s\n' "$name/" "$name/bridgewatch" "$name/LICENSE" "$name/NOTICE" \
    "$name/THIRD-PARTY-LICENSES-cli.txt" | sort)"
  actual="$(tar -tzf "$tarball" | sort)"
  if [ "$expected" != "$actual" ]; then
    echo "::error::$name.tar.gz does not hold exactly the four expected files"
    diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true
    exit 1
  fi
  out="$work/$t"
  mkdir -p "$out" "$pkg/vendor/$t"
  tar -xzf "$tarball" -C "$out" --strip-components=1
  install -m 0755 "$out/bridgewatch" "$pkg/vendor/$t/bridgewatch"
  if [ -z "$first" ]; then
    first="$out/THIRD-PARTY-LICENSES-cli.txt"
  elif ! cmp -s "$first" "$out/THIRD-PARTY-LICENSES-cli.txt"; then
    echo "::error::the tarballs carry different THIRD-PARTY-LICENSES-cli.txt files"
    exit 1
  fi
done
install -m 0644 "$first" "$pkg/THIRD-PARTY-LICENSES-cli.txt"
install -m 0644 "$repo/LICENSE" "$repo/NOTICE" "$pkg/"
(cd "$pkg" && find . -type f -not -path './test/*' | sort)
