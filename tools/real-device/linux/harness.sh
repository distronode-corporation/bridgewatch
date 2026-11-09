#!/usr/bin/env bash
# The Linux real-device harness: run the real bridgewatch app on a private X display,
# under a private D-Bus session, with a throwaway HOME and a throwaway gnome-keyring.
# Nothing reaches the caller's desktop, session bus, keyring or config.
#
#   harness.sh check                     the tools this needs, and which are missing
#   harness.sh start <run-dir>           start the private session; prints `export BWH_RUN=...`
#   harness.sh run <cmd> [args...]       run a command inside it (its bus, HOME and display)
#   harness.sh launch <name> <cmd> [args...]  the same in the background, logged to
#                                        $BWH_RUN/log/<name>.log; `stop` ends it by PID
#   harness.sh standin <name> [port]     start a loopback GitLab stand-in; prints its port
#   harness.sh token <name>              make up a token, kept in $BWH_RUN/token-<name>
#   harness.sh entry [account]           bridgewatch's keyring entry, made-up tokens redacted
#   harness.sh reqs <name>               the requests a stand-in got, by token and path
#   harness.sh shot <name> [x y w h]     check the display tree, then capture it to a PNG
#   harness.sh click-notification <id> [action]  invoke a notification's action (default
#                                        "default"), as a click on it would
#   harness.sh keyring status|lock|unlock
#   harness.sh cancel-prompts <seconds>  dismiss every keyring unlock prompt while it runs
#   harness.sh stop                      stop everything the session started, by PID
#
# Every command but `check` and `start` reads the run directory from $BWH_RUN. See
# tools/real-device/README.md for a whole run.
set -euo pipefail

H="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCREEN="${BWH_SCREEN:-1600x1000}"

die() { echo "harness: $*" >&2; exit 1; }
need_run() {
  [ -n "${BWH_RUN:-}" ] || die "BWH_RUN is not set (run: eval \"\$(harness.sh start <dir>)\")"
  [ -f "$BWH_RUN/session.env" ] || die "$BWH_RUN has no running session"
  # shellcheck disable=SC1091
  . "$BWH_RUN/session.env"
}

# Run inside the private session. env -i first, so nothing of the caller's environment
# (WAYLAND_DISPLAY, XDG_SESSION_TYPE, WAYLAND_SOCKET, its DISPLAY, its bus, its HOME)
# reaches the command. GDK_BACKEND=x11 keeps GTK and WebKitGTK on the private display;
# a Chromium-based tool run here would also need --ozone-platform=x11.
inside() {
  need_run
  exec env -i PATH="$H/bin:/usr/bin:/bin" HOME="$BWH_RUN/home" XDG_RUNTIME_DIR="$BWH_RUN/run" \
    XDG_CONFIG_HOME="$BWH_RUN/home/.config" XDG_DATA_HOME="$BWH_RUN/home/.local/share" \
    XDG_CACHE_HOME="$BWH_RUN/home/.cache" TMPDIR="$BWH_RUN/tmp" \
    DISPLAY=":$BWH_DISPLAY" GDK_BACKEND=x11 DBUS_SESSION_BUS_ADDRESS="$BWH_BUS" \
    BWH_RUN="$BWH_RUN" BWH_LOG="$BWH_RUN/log" LANG=C.UTF-8 WEBKIT_DISABLE_COMPOSITING_MODE=1 "$@"
}

# The capture guard: the display must be the one this session started, its root window
# the size Xvfb was given, and its server Xvfb's. A shot is never taken otherwise.
check_display() {
  need_run
  local tree="$1" w h
  # Who serves the display comes first, before anything talks to it: the listener on its
  # socket must be this session's Xvfb, never a desktop that happens to answer on that
  # number.
  [ -n "$BWH_XVFB" ] && [ "$(cat "/proc/$BWH_XVFB/comm" 2>/dev/null)" = Xvfb ] ||
    die "the session's Xvfb (pid $BWH_XVFB) is not running"
  ss -xlpH 2>/dev/null | awk -v p="/tmp/.X11-unix/X$BWH_DISPLAY" '$5 == p' |
    grep -q "pid=$BWH_XVFB," || die "display :$BWH_DISPLAY is not served by this session's Xvfb"
  xwininfo -root -tree -display ":$BWH_DISPLAY" >"$tree" || die "display :$BWH_DISPLAY is not reachable"
  read -r w h < <(xwininfo -root -display ":$BWH_DISPLAY" | awk '/Width:/{w=$2} /Height:/{h=$2} END{print w, h}')
  [ "${w}x${h}" = "$BWH_SCREEN" ] || die "display :$BWH_DISPLAY is ${w}x${h}, not this session's $BWH_SCREEN"
}

cmd="${1:-}"; shift || true
case "$cmd" in
check)
  missing=0
  for t in Xvfb xwininfo ffmpeg ss gdbus dbus-run-session dbus-monitor gnome-keyring-daemon secret-tool openssl python3; do
    if command -v "$t" >/dev/null; then echo "ok       $t"; else echo "MISSING  $t"; missing=1; fi
  done
  if python3 -c 'import gi; gi.require_version("Gio", "2.0"); from gi.repository import Gio' 2>/dev/null; then
    echo "ok       python3 gi (Gio)"
  else echo "MISSING  python3 gi (Gio)"; missing=1; fi
  for lib in libX11 libXtst; do
    if python3 -c "import ctypes.util,sys; sys.exit(0 if ctypes.util.find_library('${lib#lib}') else 1)"; then
      echo "ok       $lib"; else echo "MISSING  $lib"; missing=1; fi
  done
  exit "$missing"
  ;;
start)
  run="${1:?usage: harness.sh start <run-dir>}"
  mkdir -p "$run"
  [ -z "$(ls -A "$run")" ] || die "$run is not empty; use a new directory per run"
  run="$(cd "$run" && pwd)"
  mkdir -m 0700 "$run/home" "$run/run" "$run/tmp" "$run/log" "$run/srv" "$run/shots"
  # A private bus: listens in the run directory, and allows eavesdropping so the session
  # can log keyring prompts with dbus-monitor.
  cat >"$run/session.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$run/run</listen>
  <standard_session_servicedirs />
  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>
EOF
  # env -i BEFORE dbus-run-session: the bus, and everything it activates (the keyring's
  # prompter included), inherits only this environment, never the caller's display.
  env -i PATH="/usr/bin:/bin" HOME="$run/home" XDG_RUNTIME_DIR="$run/run" TMPDIR="$run/tmp" \
    XDG_DATA_HOME="$run/home/.local/share" XDG_CONFIG_HOME="$run/home/.config" \
    BWH_RUN="$run" BWH_H="$H" BWH_SCREEN="$SCREEN" LANG=C.UTF-8 \
    setsid dbus-run-session --config-file="$run/session.conf" -- bash "$H/session.sh" \
    >"$run/log/session.out" 2>&1 </dev/null &
  for _ in $(seq 100); do [ -f "$run/session.env" ] && break; sleep 0.1; done
  [ -f "$run/session.env" ] || die "the session did not start; see $run/log/session.out"
  echo "export BWH_RUN=$run"
  ;;
run) inside "$@" ;;
launch)
  need_run
  name="${1:?usage: harness.sh launch <name> <cmd> [args...]}"; shift
  "$0" run "$@" >"$BWH_RUN/log/$name.log" 2>&1 &
  echo "$!" >>"$BWH_RUN/pids"
  echo "$name: pid $!, log $BWH_RUN/log/$name.log"
  ;;
standin)
  need_run
  name="${1:?usage: harness.sh standin <name> [port]}"
  [ -f "$BWH_RUN/srv/$name.state.json" ] || die "write $BWH_RUN/srv/$name.state.json first (see gitlab_standin.py)"
  rm -f "$BWH_RUN/srv/$name.port"
  python3 -I "$H/gitlab_standin.py" "$BWH_RUN/srv" "$name" "${2:-0}" >"$BWH_RUN/log/standin-$name.err" 2>&1 &
  echo "$!" >>"$BWH_RUN/pids"
  for _ in $(seq 50); do [ -s "$BWH_RUN/srv/$name.port" ] && break; sleep 0.1; done
  cat "$BWH_RUN/srv/$name.port"
  ;;
token)
  need_run
  name="${1:?usage: harness.sh token <name>}"
  [ -f "$BWH_RUN/token-$name" ] || { umask 077; printf 'glpat-harness%s' "$(openssl rand -hex 10)" >"$BWH_RUN/token-$name"; }
  echo "$BWH_RUN/token-$name"
  ;;
entry)
  need_run
  rc=0
  v="$("$0" run secret-tool lookup service "bridgewatch:${1:-standin}" username token 2>&1)" || rc=$?
  for t in "$BWH_RUN"/token-*; do [ -f "$t" ] && v="${v//$(cat "$t")/<$(basename "$t")>}"; done
  printf 'rc=%s value=%q\n' "$rc" "$v"
  ;;
reqs)
  need_run
  python3 -I "$H/reqs.py" "$BWH_RUN" "${1:?usage: harness.sh reqs <name>}"
  ;;
shot)
  need_run
  name="${1:?usage: harness.sh shot <name> [x y w h]}"
  check_display "$BWH_RUN/log/tree-$name.txt"
  out="$BWH_RUN/shots/$name.png"
  if [ $# -ge 5 ]; then
    ffmpeg -loglevel error -y -f x11grab -video_size "$4x$5" -i ":$BWH_DISPLAY+$2,$3" -frames:v 1 "$out"
  else
    ffmpeg -loglevel error -y -f x11grab -video_size "$BWH_SCREEN" -i ":$BWH_DISPLAY" -frames:v 1 "$out"
  fi
  echo "$out"
  ;;
click-notification)
  inside gdbus call --session --dest org.freedesktop.Notifications \
    --object-path /org/freedesktop/Notifications --method org.bwtest.Control.Invoke \
    "${1:?usage: harness.sh click-notification <id> [action]}" "${2:-default}"
  ;;
keyring) inside python3 -I "$H/keyring.py" "$@" ;;
cancel-prompts)
  need_run
  end=$((SECONDS + ${1:?usage: harness.sh cancel-prompts <seconds>}))
  while [ "$SECONDS" -lt "$end" ]; do
    w="$("$0" run python3 -I "$H/xt.py" find 'Unlock' | head -1)"
    if [ -n "$w" ]; then
      echo "$(date -u +%T) unlock prompt $w -> Escape"
      "$0" run python3 -I "$H/xt.py" focus "$w"
      sleep 0.3
      "$0" run python3 -I "$H/xt.py" key Escape
      sleep 1
    fi
    sleep 0.5
  done
  ;;
stop)
  need_run
  touch "$BWH_RUN/stop"
  if [ -f "$BWH_RUN/pids" ]; then
    while read -r p; do kill "$p" 2>/dev/null || true; done <"$BWH_RUN/pids"
  fi
  for _ in $(seq 50); do kill -0 "$BWH_INNER" 2>/dev/null || break; sleep 0.1; done
  for p in $BWH_PIDS $BWH_INNER; do kill "$p" 2>/dev/null || true; done
  rm -f "$BWH_RUN/session.env"
  echo "stopped; the run directory $BWH_RUN is left for its logs and shots"
  ;;
*) sed -n '2,20p' "$0"; exit 64 ;;
esac
