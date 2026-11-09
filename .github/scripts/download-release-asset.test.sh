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

# The stand-in: answers each request with the next status in $work/statuses (the
# last one repeats), the body "payload" on a 200, and counts requests in
# $work/count.
cat >"$work/server.py" <<'PY'
import http.server, pathlib, sys
work = pathlib.Path(sys.argv[1])
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        statuses = (work / "statuses").read_text().split()
        count = work / "count"
        n = int(count.read_text()) if count.exists() else 0
        count.write_text(str(n + 1))
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
url="http://127.0.0.1:$(cat "$work/port")/asset.tar.gz"

failures=0
run() { # run <statuses> <max secs>: sets rc, err, count, secs
  printf '%s\n' "$1" >"$work/statuses"
  rm -f "$work/count" "$work/out"
  local start=$SECONDS
  rc=0
  err="$(DOWNLOAD_PROTO='=http' DOWNLOAD_MAX_SECS="$2" bash "$script" "$url" "$work/out" 2>&1)" || rc=$?
  secs=$((SECONDS - start))
  count="$(cat "$work/count" 2>/dev/null || echo 0)"
}
check() { # check <name> <condition...>
  local name="$1"; shift
  if "$@"; then echo "ok   $name"; else echo "FAIL $name (rc=$rc count=$count secs=$secs: $err)"; failures=$((failures + 1)); fi
}

run "504 504 200" 120
check "504, 504, then 200: downloaded on the third request" \
  test "$rc" -eq 0 -a "$count" -eq 3 -a "$(cat "$work/out" 2>/dev/null)" = payload

run "404" 120
check "a 404 fails on the first request, naming the status and the URL" \
  bash -c "[ $rc -ne 0 ] && [ $count -eq 1 ] && [ ! -e '$work/out' ] && grep -q 'HTTP 404 from $url (not retried' <<<\"\$0\"" "$err"

run "504" 4
check "504 every time: retried, then fails within the time bound, naming 504" \
  bash -c "[ $rc -ne 0 ] && [ $count -gt 1 ] && [ $secs -le 15 ] && grep -q 'HTTP 504 from $url, still after retrying for up to 4s' <<<\"\$0\"" "$err"

run "503 200" 120
check "a 503 is retried too" test "$rc" -eq 0 -a "$count" -eq 2

[ "$failures" -eq 0 ] || { echo "$failures failed"; exit 1; }
echo "all passed"
