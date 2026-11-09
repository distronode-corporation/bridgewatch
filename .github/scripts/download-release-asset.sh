#!/usr/bin/env bash
# Download one public release asset, riding out the gateway errors GitHub's
# download endpoints answer with now and then.
#
#   bash .github/scripts/download-release-asset.sh <out-file> <url> [<url>...]
#
# Each URL is a source of the same file, tried in turn. On each, curl retries a
# transient answer (408, 429, 500, 502, 503, 504, a timeout, a reset connection)
# with a delay that doubles from one second, for that source's share of
# DOWNLOAD_MAX_SECS (120 unless set); the whole download never takes much
# longer than that. Any other answer (a 404, a 403) is not retried on that
# source. Whatever the last source answers decides a failure, which ends with
# one `::error::` line naming the HTTP status and the URL; an earlier source's
# failure is a `::warning::` and the next source is tried.
#
# ⛔ `curl --retry 3` alone was the cause of red `cli` jobs: three retries with
# its default delays (1, 2 and 4 s) spent about eight seconds on a run of 504s
# that lasted longer. And on 2026-11-09 the release page's download URL
# (github.com/.../releases/download/...) answered 504 after ~11 s per request
# for over two minutes while the API's asset URL answered in about a second, so
# a time bound alone was not enough either: the caller names both.
#
# Every request carries `Accept: application/octet-stream`, which the API's
# asset URL needs to send the file rather than its metadata, and which the
# release page's URL ignores.
#
# DOWNLOAD_PROTO (default `=https`) is curl's `--proto`; only the tests set it,
# to reach a loopback stand-in over plain http.
set -euo pipefail

out="${1:?out file}"
shift
[ $# -gt 0 ] || { echo "::error::no URL to download $out from" >&2; exit 64; }
max_secs="${DOWNLOAD_MAX_SECS:-120}"
proto="${DOWNLOAD_PROTO:-=https}"
share=$((max_secs / $#))
[ "$share" -ge 1 ] || share=1

left=$#
for url in "$@"; do
  left=$((left - 1))
  code=""
  rc=0
  code="$(curl -fsSL --proto "$proto" --tlsv1.2 \
    -H 'Accept: application/octet-stream' \
    --retry 30 --retry-max-time "$share" --connect-timeout 20 \
    -w '%{http_code}' -o "$out" "$url")" || rc=$?
  [ "$rc" -ne 0 ] || exit 0
  rm -f "$out"
  case "$code" in
    408 | 429 | 500 | 502 | 503 | 504) what="HTTP $code from $url, still after retrying for up to ${share}s" ;;
    000 | "") what="no answer from $url (curl exit $rc), after retrying for up to ${share}s" ;;
    *) what="HTTP $code from $url (not retried: it is not a transient error)" ;;
  esac
  if [ "$left" -gt 0 ]; then
    echo "::warning::$what; trying the next source" >&2
  else
    echo "::error::$what" >&2
    exit 1
  fi
done
