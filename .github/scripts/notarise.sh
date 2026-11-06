#!/usr/bin/env bash
# Submit one file to Apple's notary service, wait for the verdict, and fail unless it
# is `Accepted`. Used by release.yml's macOS leg for the CLI (a zip of the signed
# Mach-O) and for the .dmg.
#
#   bash .github/scripts/notarise.sh <file>
#
# Reads the App Store Connect API key from the environment of the step that calls it:
# APPLE_API_KEY_PATH (the .p8 file the workflow wrote), APPLE_API_KEY (its id) and
# APPLE_API_ISSUER. Nothing here prints any of them.
#
# ⛔ The status is read from notarytool's JSON, never inferred from its exit code: the
# verdict is the thing being asserted, and an `Invalid` or `Rejected` submission must
# fail the leg with Apple's log printed, whatever notarytool exits with.
set -euo pipefail

file="${1:?usage: notarise.sh <file>}"
[ -s "$file" ] || { echo "::error::$file does not exist or is empty"; exit 1; }
for v in APPLE_API_KEY_PATH APPLE_API_KEY APPLE_API_ISSUER; do
  if [ -z "${!v:-}" ]; then
    echo "::error::$v is not set, so $(basename "$file") cannot be notarised"
    exit 1
  fi
done
[ -s "$APPLE_API_KEY_PATH" ] || { echo "::error::the notary key file is missing or empty"; exit 1; }

auth=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
out="${RUNNER_TEMP:?}/notary-$(basename "$file").json"

rc=0
xcrun notarytool submit "$file" "${auth[@]}" --wait --timeout 30m --output-format json >"$out" || rc=$?

field() {
  python3 -I -c 'import json, sys
try:
    print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))
except Exception:
    print("")' "$out" "$1"
}
status="$(field status)"
id="$(field id)"
echo "notarytool: $(basename "$file") submission ${id:-<none>} status ${status:-<none>} (exit $rc)"

if [ "$status" != "Accepted" ]; then
  echo "::error::Apple's notary service did not accept $(basename "$file") (status: ${status:-none})"
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" "${auth[@]}" || true
  fi
  exit 1
fi
