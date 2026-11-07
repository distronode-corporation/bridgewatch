#!/usr/bin/env bash
# Prove, end to end against the platform's real credential store, that an own
# token (`token = { own = true }`) stays with its server.
#
#   bash .github/scripts/bound-token-probe.sh <cli> <released-1.3.0-cli>
#
# Run by the `cli` job in ci.yml: on macOS from the cli-keychain-probe action, in
# a throwaway keychain it has made the user's default (the keyring crate reads
# only the default keychain), and on Linux by itself, under a private D-Bus
# session with a throwaway gnome-keyring, a temporary HOME and a temporary
# XDG_RUNTIME_DIR, so the real login keyring is never reached. No secret is used:
# every value is made up here.
#
# Two loopback stand-ins for GitLab, A and B, record the headers of every request
# and answer 404. The steps:
#   1. seed a bare 1.x entry (on macOS with -A, any application may read it);
#      as a control, the released 1.3.0 CLI reads it and sends it to A;
#   2. `check` against A sends the token to A and rewrites the entry, which then
#      starts with bw-bound-v1: and names A's origin;
#   3. repoint the account to B: `check` exits 70 naming A, and B gets nothing;
#   4. the released 1.3.0 CLI, pointed at A and then at B, exits non-zero on the
#      bound entry with its control-character refusal, and neither gets a request.
set -euo pipefail

new_cli="${1:?cli}"
old_cli="${2:?released cli}"
new_cli="$(cd "$(dirname "$new_cli")" && pwd)/$(basename "$new_cli")"
old_cli="$(cd "$(dirname "$old_cli")" && pwd)/$(basename "$old_cli")"
os="$(uname -s)"

# ---- Linux: re-run inside a private session bus with a throwaway keyring. ----
if [ "$os" = Linux ] && [ "${BW_PROBE_SESSION:-}" != 1 ]; then
  for tool in dbus-run-session gnome-keyring-daemon secret-tool; do
    command -v "$tool" >/dev/null || { echo "::error::$tool is not installed"; exit 1; }
  done
  scratch="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:?set RUNNER_TEMP or TMPDIR}}/bound-probe.XXXXXX")"
  trap 'rm -rf "$scratch"' EXIT
  mkdir -m 0700 "$scratch/home" "$scratch/run"
  rc=0
  # env -i: nothing of the caller's session (its bus, its keyring control socket,
  # its HOME) reaches the probe.
  env -i PATH="$PATH" HOME="$scratch/home" XDG_RUNTIME_DIR="$scratch/run" \
    RUNNER_TEMP="$scratch" BW_PROBE_SESSION=1 \
    dbus-run-session -- bash "${BASH_SOURCE[0]}" "$new_cli" "$old_cli" || rc=$?
  exit "$rc"
fi

work="$(mktemp -d "${RUNNER_TEMP:?}/bound-probe-work.XXXXXX")"
servers=()
cleanup() {
  # The +-expansion: bash 3.2 calls an empty array unbound under set -u.
  for pid in ${servers[@]+"${servers[@]}"}; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "$work"
}
trap cleanup EXIT

if [ "$os" = Linux ]; then
  # A fresh login keyring in the temporary HOME, created and unlocked with a
  # made-up password, then the secrets component on this bus.
  kcpass="$(openssl rand -hex 16)"
  # Its chatter (an unterminated "discover_other_daemon: 1") is shown only on failure.
  printf '%s' "$kcpass" | gnome-keyring-daemon --unlock --components=secrets >/dev/null 2>"$work/gk.err" \
    || { cat "$work/gk.err"; echo "::error::gnome-keyring-daemon --unlock failed"; exit 1; }
  # It exits with the session bus when dbus-run-session ends.
  gnome-keyring-daemon --start --components=secrets >/dev/null 2>"$work/gk.err" \
    || { cat "$work/gk.err"; echo "::error::gnome-keyring-daemon --start failed"; exit 1; }
  case "$HOME" in "$RUNNER_TEMP"/*) ;; *) echo "::error::HOME is not the throwaway one"; exit 1 ;; esac
fi

account="boundprobe$(openssl rand -hex 4)"
service="bridgewatch:$account"
marker="probe-$(openssl rand -hex 16)"

# Seed, read back and delete the own entry, the way a 1.x user's store holds it.
seed() {
  if [ "$os" = Darwin ]; then
    # -A stands in for the ACL a 1.x entry has when 1.x itself wrote it; written
    # by `security` without it, the first read would raise a prompt nobody can
    # answer here.
    security add-generic-password -A -s "$service" -a token -w "$marker" "${KEYCHAIN:?}"
  else
    printf '%s' "$marker" | secret-tool store --label="bridgewatch probe" service "$service" username token
  fi
}
stored() {
  if [ "$os" = Darwin ]; then
    local raw
    raw="$(security find-generic-password -s "$service" -a token -w "${KEYCHAIN:?}")"
    # `security -w` prints a value holding a non-printable byte (the tab after
    # the marker) as hex.
    # Either case: which one `security` uses is not documented.
    if [[ "$raw" =~ ^([0-9a-fA-F]{2})+$ ]] && [ "${#raw}" -gt "${#marker}" ]; then
      echo "security -w printed the entry as hex" >&2
      printf '%s' "$raw" | xxd -r -p
    else
      echo "security -w printed the entry as text" >&2
      printf '%s' "$raw"
    fi
  else
    secret-tool lookup service "$service" username token
  fi
}

# The two stand-ins. Each writes every request's line and headers to
# <name>.headers and answers 404.
#
# ⛔ From a file, not a heredoc on stdin: macOS's /bin/bash (3.2) gives an
# asynchronous command inside a function /dev/null as its stdin, heredoc or
# not, so python read an empty script and exited 0 without a word.
cat > "$work/standin.py" <<'PY'
import http.server
import pathlib
import sys

work = pathlib.Path(sys.argv[1])
name = sys.argv[2]


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(work / f"{name}.headers", "a", encoding="utf-8") as out:
            out.write(f"{self.command} {self.path}\n")
            for key, value in self.headers.items():
                out.write(f"{key}: {value}\n")
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
(work / f"{name}.port.tmp").write_text(str(server.server_address[1]), encoding="utf-8")
(work / f"{name}.port.tmp").rename(work / f"{name}.port")
server.serve_forever()
PY
start_standin() {
  local pid
  python3 -I "$work/standin.py" "$work" "$1" </dev/null 2>"$work/$1.err" &
  pid=$!
  servers+=("$pid")
  for _ in $(seq 1 150); do
    [ -s "$work/$1.port" ] && break
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
  done
  if [ ! -s "$work/$1.port" ]; then
    if kill -0 "$pid" 2>/dev/null; then
      echo "::error::stand-in $1 is running but wrote no port in 30s"
    else
      rc=0
      wait "$pid" || rc=$?
      echo "::error::stand-in $1 exited $rc before writing its port"
    fi
    cat "$work/$1.err"
    ls -la "$work"
    command -v python3
    python3 --version
    exit 1
  fi
  : > "$work/$1.headers"
}
start_standin a
start_standin b
origin_a="http://127.0.0.1:$(cat "$work/a.port")"
origin_b="http://127.0.0.1:$(cat "$work/b.port")"

config() {
  cat > "$work/config.toml" <<EOF
[accounts.$account]
base_url = "$1"
token = { own = true }

[[watches]]
id = "probe"
account = "$account"
project = 12345678
ref = "main"
EOF
}

# Every CLI run is bounded: a store that prompts would otherwise hang the job.
# 142 is SIGALRM, and is reported as a hang, never as a refusal.
last_rc=0
run() {
  local label="$1"
  shift
  last_rc=0
  perl -e 'alarm shift @ARGV; exec @ARGV or die "exec: $!"' 120 "$@" --config "$work/config.toml" check \
    >"$work/out" 2>"$work/err" || last_rc=$?
  echo "$label: check exited $last_rc"
  if [ "$last_rc" -eq 142 ]; then
    echo "::error::$label: the CLI did not finish in 120s (a credential prompt?)"
    exit 1
  fi
}
requests() { grep -c '^GET ' "$work/$1.headers" || true; }
redacted_err() { sed -E "s/$marker/<marker>/g" "$work/err"; }

echo "probe: bash $BASH_VERSION, account $account, stand-in A $origin_a, stand-in B $origin_b"

# ---- 1. A bare 1.x entry, and the control: 1.3.0 reads it. ----
seed
[ "$(stored 2>/dev/null)" = "$marker" ] \
  || { echo "::error::step 1: the seeded entry does not read back from the shell"; exit 1; }
echo "step 1: seeded a bare entry for service $service, user token"
config "$origin_a"
run "step 1 (control, released 1.3.0 against A)" "$old_cli"
grep -qF "$marker" "$work/a.headers" \
  || { echo "::error::step 1: the released 1.3.0 CLI did not send the bare entry to A, so step 4 would prove nothing"; redacted_err; exit 1; }
[ "$(stored 2>/dev/null)" = "$marker" ] \
  || { echo "::error::step 1: the released 1.3.0 CLI changed the entry"; exit 1; }
echo "step 1: the released 1.3.0 CLI read the bare entry and sent it to A ($(requests a) requests); the entry is unchanged"
: > "$work/a.headers"

# ---- 2. This CLI against A: sent to A, and the entry is bound to A. ----
run "step 2 (this CLI against A)" "$new_cli"
grep -qF "$marker" "$work/a.headers" \
  || { echo "::error::step 2: this CLI did not send the token to A"; redacted_err; exit 1; }
[ "$(requests b)" -eq 0 ] || { echo "::error::step 2: B got a request"; exit 1; }
value="$(stored)"
case "$value" in
  "bw-bound-v1:"$'\t'*) ;;
  *) echo "::error::step 2: the entry does not start with bw-bound-v1: and a tab"; exit 1 ;;
esac
doc_origin="$(printf '%s' "${value#*$'\t'}" | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(d["origin"] if d.get("token") else "")')"
[ "$doc_origin" = "$origin_a" ] \
  || { echo "::error::step 2: the entry is bound to \"$doc_origin\", expected $origin_a"; exit 1; }
echo "step 2: this CLI sent the token to A ($(requests a) requests), and the entry now starts with bw-bound-v1: and is bound to $doc_origin"
a_before="$(requests a)"

# ---- 3. Repointed to B: refused with 70, nothing sent. ----
config "$origin_b"
run "step 3 (this CLI, account repointed to B)" "$new_cli"
redacted_err
[ "$last_rc" -eq 70 ] || { echo "::error::step 3: check exited $last_rc, expected 70"; exit 1; }
[ "$(requests b)" -eq 0 ] || { echo "::error::step 3: B got $(requests b) requests"; exit 1; }
[ "$(requests a)" -eq "$a_before" ] || { echo "::error::step 3: A got a request"; exit 1; }
grep -qF "$origin_a" "$work/err" \
  || { echo "::error::step 3: the refusal does not name the server the token belongs to ($origin_a)"; exit 1; }
echo "step 3: this CLI exited 70 naming $origin_a, and B got no request"

# ---- 4. The released 1.3.0 CLI on the bound entry: refused locally. ----
for target in a b; do
  if [ "$target" = a ]; then config "$origin_a"; else config "$origin_b"; fi
  run "step 4 (released 1.3.0 against $target)" "$old_cli"
  redacted_err
  [ "$last_rc" -ne 0 ] || { echo "::error::step 4: the released 1.3.0 CLI exited 0 against $target"; exit 1; }
  grep -qF "cannot go in an HTTP header" "$work/err" \
    || { echo "::error::step 4: the released 1.3.0 CLI failed against $target, but not with its control-character refusal"; exit 1; }
done
[ "$(requests b)" -eq 0 ] || { echo "::error::step 4: B got $(requests b) requests"; exit 1; }
[ "$(requests a)" -eq "$a_before" ] || { echo "::error::step 4: A got a request"; exit 1; }
echo "step 4: the released 1.3.0 CLI refused the bound entry against A and B without sending, and no stand-in got a request"

if [ "$os" = Linux ]; then
  secret-tool clear service "$service" username token || true
fi
echo "the bound-token probe passed on $os"
