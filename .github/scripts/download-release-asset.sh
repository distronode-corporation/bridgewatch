#!/usr/bin/env bash
# Download one public release asset, riding out the gateway errors GitHub's
# download endpoints answer with now and then.
#
#   bash .github/scripts/download-release-asset.sh <out-file> <url> [<url>...]
#
# Each URL is a source of the same file, tried in turn. On each, a transient
# failure is asked again, with a delay that doubles from one second (capped at
# 30 s), for that source's share of DOWNLOAD_MAX_SECS (120 unless set), so the
# whole download never takes much longer than that. Transient is an HTTP 408,
# 429, 500, 502, 503 or 504, or no usable answer at all: the connection refused,
# reset or timed out (curl exit 6, 7, 28, 35, 52, 55 or 56). Anything else, a 404
# or a 403 above all, is not asked again on that source. Whatever the last source
# answers decides a failure, which ends with one `::error::` line naming the HTTP
# status or curl's exit and the URL; an earlier source's failure is a
# `::warning::` and the next source is tried.
#
# ⛔ The retrying is this loop's, not curl's `--retry`. `--retry 3` gave up
# after about eight seconds of a run of 504s that lasted minutes, and on a macOS
# runner curl reported such a 504 as exit 56 and did not retry it at all, so one
# 504 failed the source (2026-11-09). The loop reads the status itself.
#
# ⛔ And on 2026-11-09 the release page's download URL
# (github.com/.../releases/download/...) answered 504 after ~11 s per request
# for over two minutes while the API's asset URL answered in about a second, so
# a time bound alone was not enough either: the caller names both.
#
# DOWNLOAD_GITHUB_TOKEN, when set, is sent (as a bearer token, through curl's
# config on stdin, never on the command line) to URLs that start with
# DOWNLOAD_TOKEN_PREFIX (`https://api.github.com/` unless set) and to no other.
# Unauthenticated, the API allows 60 requests an hour per address, and a shared
# CI runner's address had used them up: the API's asset URL answered 403. curl
# does not carry the header across the redirect to the download host.
#
# Every request carries `Accept: application/octet-stream`, which the API's
# asset URL needs to send the file rather than its metadata, and which the
# release page's URL ignores.
#
# DOWNLOAD_SHA256, when set, is the digest the file must have, checked per
# source: a source that delivers other bytes is a failure of that source and the
# next is tried. ⛔ This is what makes a pinned API asset id safe to go stale.
# Asset ids are global, so an id left behind by a version bump still answers 200
# with the OLD version's file; without the check here that file ended the
# download and failed the caller's own digest check, and the release page's URL,
# which would have served the right file, was never asked (Seer, #82).
#
# DOWNLOAD_PROTO (default `=https`) is curl's `--proto`; only the tests set it,
# to reach a loopback stand-in over plain http.
set -euo pipefail

out="${1:?out file}"
shift
[ $# -gt 0 ] || { echo "::error::no URL to download $out from" >&2; exit 64; }
max_secs="${DOWNLOAD_MAX_SECS:-120}"
proto="${DOWNLOAD_PROTO:-=https}"
want_sha256="${DOWNLOAD_SHA256:-}"
token="${DOWNLOAD_GITHUB_TOKEN:-}"
token_prefix="${DOWNLOAD_TOKEN_PREFIX:-https://api.github.com/}"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# One request: sets `code` (the HTTP status, 000 for none) and `rc` (curl's exit).
fetch() {
  local url="$1" config=""
  if [ -n "$token" ] && [ "${url#"$token_prefix"}" != "$url" ]; then
    config="header = \"Authorization: Bearer $token\""
  fi
  rc=0
  code="$(printf '%s\n' "$config" | curl -fsSL --config - --proto "$proto" --tlsv1.2 \
    -H 'Accept: application/octet-stream' --connect-timeout 20 --max-time 90 \
    -w '%{http_code}' -o "$out" "$url")" || rc=$?
}

transient() {
  case "$code" in
    408 | 429 | 500 | 502 | 503 | 504) return 0 ;;
    000 | "" | 2??)
      case "$rc" in 6 | 7 | 28 | 35 | 52 | 55 | 56) return 0 ;; esac ;;
  esac
  return 1
}

share=$((max_secs / $#))
[ "$share" -ge 1 ] || share=1

left=$#
for url in "$@"; do
  left=$((left - 1))
  deadline=$((SECONDS + share))
  delay=1
  while :; do
    fetch "$url"
    [ "$rc" -ne 0 ] || break
    transient || break
    [ $((SECONDS + delay)) -lt "$deadline" ] || break
    sleep "$delay"
    delay=$((delay * 2))
    [ "$delay" -le 30 ] || delay=30
  done
  got=""
  if [ "$rc" -eq 0 ]; then
    [ -n "$want_sha256" ] || exit 0
    got="$(sha256_of "$out")"
    [ "$got" != "$want_sha256" ] || exit 0
  fi
  rm -f "$out"
  # ⛔ A 2xx status says nothing on its own: curl reports the status it got even
  # when the body then broke off, so the digest message is for a transfer that
  # finished (rc 0), and a broken one says curl's exit (Seer, #82).
  if [ "$rc" -eq 0 ]; then
    what="$url delivered a file with sha256 $got, expected $want_sha256"
  elif transient; then
    case "$code" in
      000 | "" | 2??) what="no usable answer from $url (curl exit $rc), still after retrying for up to ${share}s" ;;
      *) what="HTTP $code from $url, still after retrying for up to ${share}s" ;;
    esac
  else
    case "$code" in
      000 | "") what="no answer from $url (curl exit $rc)" ;;
      2??) what="the transfer from $url broke off after HTTP $code (curl exit $rc)" ;;
      *) what="HTTP $code from $url (not retried: it is not a transient error)" ;;
    esac
  fi
  if [ "$left" -gt 0 ]; then
    echo "::warning::$what; trying the next source" >&2
  else
    echo "::error::$what" >&2
    exit 1
  fi
done
