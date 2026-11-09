#!/usr/bin/env bash
# Started by `harness.sh start` inside dbus-run-session, with an environment that holds
# only the run directory. Starts the private desktop, writes $BWH_RUN/session.env, and
# waits for `harness.sh stop`.
set -u
R="$BWH_RUN"; L="$R/log"
pids=()

# Xvfb picks a free display number itself (-displayfd) and listens on no TCP port.
Xvfb -displayfd 3 -screen 0 "${BWH_SCREEN}x24" -nolisten tcp 3>"$R/display" >"$L/xvfb.log" 2>&1 &
xvfb=$!; pids+=("$xvfb")
for _ in $(seq 100); do [ -s "$R/display" ] && break; sleep 0.1; done
display="$(tr -d '[:space:]' <"$R/display")"
[ -n "$display" ] || { echo "Xvfb did not report a display" >&2; kill "$xvfb"; exit 1; }

# Services this bus activates (the keyring's prompter) start on the private display,
# never on one inherited from anywhere else.
gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
  --method org.freedesktop.DBus.UpdateActivationEnvironment \
  "{'DISPLAY': ':$display', 'GDK_BACKEND': 'x11'}" >/dev/null

# A throwaway login keyring, created and unlocked with a random password that only this
# run directory holds (keyring.py unlocks it again after a lock).
umask 077
openssl rand -hex 16 | tr -d '\n' >"$R/kcpass"
gnome-keyring-daemon --foreground --unlock --components=secrets <"$R/kcpass" >"$L/gk.out" 2>"$L/gk.err" &
pids+=($!)
for _ in $(seq 50); do
  gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
    --method org.freedesktop.DBus.NameHasOwner org.freedesktop.secrets 2>/dev/null | grep -q true && break
  sleep 0.1
done

# Stand-in notification server and StatusNotifierWatcher (desktopd.py), and a log of
# every keyring prompt on this bus.
DISPLAY=":$display" python3 -I "$BWH_H/desktopd.py" "$L" >"$L/desktopd.err" 2>&1 & pids+=($!)
dbus-monitor "interface='org.freedesktop.Secret.Prompt'" \
  "interface='org.freedesktop.Secret.Service',member='Unlock'" \
  "interface='org.gnome.keyring.SystemPrompter'" "interface='org.gnome.keyring.internal.Prompter'" \
  >"$L/secret-prompts.log" 2>&1 & pids+=($!)

{
  echo "BWH_DISPLAY=$display"
  echo "BWH_SCREEN=$BWH_SCREEN"
  echo "BWH_XVFB=$xvfb"
  echo "BWH_BUS='$DBUS_SESSION_BUS_ADDRESS'"
  echo "BWH_PIDS=\"${pids[*]}\""
  echo "BWH_INNER=$$"
} >"$R/session.env.part"
mv "$R/session.env.part" "$R/session.env"

while [ ! -e "$R/stop" ]; do sleep 1; done
for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
