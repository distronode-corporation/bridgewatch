#!/usr/bin/env bash
# Download one public release asset, riding out the gateway errors GitHub's
# download CDN answers with now and then.
#
#   bash .github/scripts/download-release-asset.sh <url> <out-file>
#
# curl retries a transient answer (408, 429, 500, 502, 503, 504, a timeout, a
# reset connection) with a delay that doubles from one second, for at most
# DOWNLOAD_MAX_SECS seconds in all (120 unless set). Anything else, a 404 or a
# 403 above all, fails on the first answer: waiting does not make a missing
# asset appear. Either way a failure ends with one `::error::` line naming the
# HTTP status and the URL.
#
# ⛔ `curl --retry 3` alone was the cause of red `cli` jobs: three retries with
# its default delays (1, 2 and 4 s) spent about eight seconds on a run of 504s
# that lasted longer. The bound is now time, not a count.
#
# DOWNLOAD_PROTO (default `=https`) is curl's `--proto`; only the tests set it,
# to reach a loopback stand-in over plain http.
set -euo pipefail

url="${1:?url}"
out="${2:?out file}"
max_secs="${DOWNLOAD_MAX_SECS:-120}"
proto="${DOWNLOAD_PROTO:-=https}"

code=""
rc=0
code="$(curl -fsSL --proto "$proto" --tlsv1.2 \
  --retry 30 --retry-max-time "$max_secs" --connect-timeout 20 \
  -w '%{http_code}' -o "$out" "$url")" || rc=$?

if [ "$rc" -ne 0 ]; then
  rm -f "$out"
  case "$code" in
    408 | 429 | 500 | 502 | 503 | 504)
      echo "::error::HTTP $code from $url, still after retrying for up to ${max_secs}s" >&2 ;;
    000 | "")
      echo "::error::no answer from $url (curl exit $rc), after retrying for up to ${max_secs}s" >&2 ;;
    *)
      echo "::error::HTTP $code from $url (not retried: it is not a transient error)" >&2 ;;
  esac
  exit 1
fi
