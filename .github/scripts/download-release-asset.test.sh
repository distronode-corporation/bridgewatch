#!/usr/bin/env bash
# Tests for download-release-asset.sh against a loopback stand-in that answers a
# scripted sequence of statuses. Needs bash, curl and python3; no network.
#
#   bash .github/scripts/download-release-asset.test.sh
#
# Run by ci.yml's `repo` job.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/download-release-asset.sh"
work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/download-test.XXXXXX")"
server_pid=""
cleanup() {
  [ -z "$server_pid" ] || { kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; }
  rm -rf "$work"
}
trap cleanup EXIT

# The stand-in: two sources, /a and /b. Each answers its requests with the next
# status in $work/statuses-<a|b> (the last one repeats), the body "payload" on a
# 200, and counts its requests in $work/count-<a|b>. It records whether every
# request asked for application/octet-stream.
cat >"$work/server.py" <<'PY'
import http.server, pathlib, sys
work = pathlib.Path(sys.argv[1])
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        src = self.path.strip("/")[:1]
        statuses = (work / f"statuses-{src}").read_text().split()
        count = work / f"count-{src}"
        n = int(count.read_text()) if count.exists() else 0
        count.write_text(str(n + 1))
        if self.headers.get("Accept") != "application/octet-stream":
            (work / "bad-accept").write_text(self.headers.get("Accept", ""))
        status = int(statuses[min(n, len(statuses) - 1)])
        body = b"payload" if status == 200 else b'{"message":"scripted"}'
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
(work / "port").write_text(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$work/server.py" "$work" &
server_pid=$!
for _ in $(seq 50); do [ -s "$work/port" ] && break; sleep 0.1; done
base="http://127.0.0.1:$(cat "$work/port")"

failures=0
run() { # run <statuses of a> <statuses of b> <max secs> <sources...>: sets rc, err, a, b, secs
  printf '%s\n' "$1" >"$work/statuses-a"
  printf '%s\n' "$2" >"$work/statuses-b"
  local max="$3"
  shift 3
  rm -f "$work/count-a" "$work/count-b" "$work/out"
  local start=$SECONDS
  rc=0
  err="$(DOWNLOAD_PROTO='=http' DOWNLOAD_MAX_SECS="$max" bash "$script" "$work/out" "$@" 2>&1)" || rc=$?
  secs=$((SECONDS - start))
  a="$(cat "$work/count-a" 2>/dev/null || echo 0)"
  b="$(cat "$work/count-b" 2>/dev/null || echo 0)"
}
check() { # check <name> <condition...>
  local name="$1"
  shift
  if "$@"; then echo "ok   $name"; else
    echo "FAIL $name (rc=$rc a=$a b=$b secs=$secs: $err)"
    failures=$((failures + 1))
  fi
}
says() { grep -qF "$1" <<<"$err"; }

run "504 504 200" "200" 120 "$base/a"
check "504, 504, then 200: downloaded on the third request" \
  test "$rc" -eq 0 -a "$a" -eq 3 -a "$(cat "$work/out" 2>/dev/null)" = payload

run "404" "200" 120 "$base/a"
check "a 404 fails on the first request, naming the status and the URL" \
  test "$rc" -ne 0 -a "$a" -eq 1 -a ! -e "$work/out"
check "  ...with one error line" says "::error::HTTP 404 from $base/a (not retried"

run "504" "200" 4 "$base/a"
check "504 every time: retried, then fails within the time bound" \
  test "$rc" -ne 0 -a "$a" -gt 1 -a "$secs" -le 15
check "  ...naming 504" says "::error::HTTP 504 from $base/a, still after retrying for up to 4s"

run "503 200" "200" 120 "$base/a"
check "a 503 is retried too" test "$rc" -eq 0 -a "$a" -eq 2

run "504" "200" 4 "$base/a" "$base/b"
check "a source that keeps answering 504 hands over to the next, within the bound" \
  test "$rc" -eq 0 -a "$a" -gt 1 -a "$b" -eq 1 -a "$secs" -le 15
check "  ...with a warning for the first" says "::warning::HTTP 504 from $base/a"

run "404" "404" 120 "$base/a" "$base/b"
check "a 404 from every source fails with the last one's 404, one request each" \
  test "$rc" -ne 0 -a "$a" -eq 1 -a "$b" -eq 1
check "  ...naming it" says "::error::HTTP 404 from $base/b (not retried"

run "200" "200" 120 "$base/a" "$base/b"
check "the first source that answers is the only one asked" test "$rc" -eq 0 -a "$a" -eq 1 -a "$b" -eq 0
check "every request asked for application/octet-stream" test ! -e "$work/bad-accept"

[ "$failures" -eq 0 ] || { echo "$failures failed"; exit 1; }
echo "all passed"
