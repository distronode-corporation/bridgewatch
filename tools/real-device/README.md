# Real-device checks

CI proves the core, the CLI and the frontend against fixtures and scripted responses, and
`.github/scripts/bound-token-probe.sh` proves own-token binding against a real credential
store. What none of that reaches is the real app on a real desktop: the tray menu, the
popover window, keychain and keyring prompts, notification clicks. Those are checked by
hand before a release tag, and this directory is how to repeat those checks.

- [`CHECKS.md`](CHECKS.md) is the release check list, for Linux and macOS, with what
  counts as a pass.
- [`linux/`](linux/) is a harness that runs the real app on a private X display, under a
  private D-Bus session, with a throwaway HOME and a throwaway gnome-keyring, against
  loopback stand-ins for GitLab.

macOS has no harness here: its checks need a person to see and count keychain prompts,
so `CHECKS.md` gives the setup (a throwaway keychain) and the steps instead.

## The rules every run keeps

- **Nothing on your own screen.** The app runs on an Xvfb display the session started,
  and `harness.sh shot` refuses to capture unless that display is served by the
  session's own Xvfb and its root window is the size Xvfb was given. Look at every
  capture before you trust a check.
- **No real keyring and no real config.** The session's HOME, XDG directories, D-Bus
  bus and keyring live in the run directory. The keyring is created with a random
  password that only the run directory holds.
- **No real token and no real server.** Tokens are made up (`harness.sh token`), and
  the GitLab stand-in listens on 127.0.0.1 only. `harness.sh entry` and `harness.sh
  reqs` print a made-up token by its file name, never its value.
- **Stop by PID.** `harness.sh stop` ends what the session started, by the PIDs it
  recorded, and nothing else.

## Linux: requirements

`harness.sh check` lists what is missing. On Ubuntu the packages are `xvfb`,
`x11-utils` (xwininfo), `ffmpeg`, `dbus-daemon` (`dbus-run-session`) and `dbus-bin`
(`dbus-monitor`), `gnome-keyring` (and `gcr`, whose prompter the locked-keyring checks
need), `libsecret-tools` (secret-tool), `python3-gi`, `libxtst6` and `iproute2` (ss). No
notification daemon, tray host or window manager is needed: `desktopd.py` stands in
for the notification server and the StatusNotifierWatcher on the private bus.

Build the app with its frontend embedded, the way a release does, but in debug:

```
npm ci
npm run tauri build -- --debug --no-bundle    # target/debug/bridgewatch-app
```

## Linux: a whole run

```
H=tools/real-device/linux/harness.sh
eval "$($H start "$HOME/bw-run-1")"           # any new, empty directory; prints BWH_RUN

# A stand-in GitLab project with a failing pipeline (the shape is in gitlab_standin.py).
cat >"$BWH_RUN/srv/a.state.json" <<'EOF'
{"token_expires_at": null,
 "projects": {"101": {"path": "acme/web", "pipeline": 9101,
   "sha": "0123456789abcdef0123456789abcdef01234567",
   "jobs": [["build", "build", "success"], ["test", "test:unit", "failed"]]}}}
EOF
port="$($H standin a)"

# A made-up own token in the throwaway keyring, and a config that uses it.
$H run secret-tool store --label=bridgewatch service bridgewatch:standin username token \
  <"$($H token a)"
mkdir -p "$BWH_RUN/home/.config/bridgewatch"
cat >"$BWH_RUN/home/.config/bridgewatch/config.toml" <<EOF
[accounts.standin]
provider = "gitlab"
base_url = "http://127.0.0.1:$port"
token = { own = true }

[[watches]]
id = "web"
account = "standin"
project = 101
ref = "main"
EOF

$H launch app target/debug/bridgewatch-app
$H reqs a                                     # requests, by made-up token and path
$H entry                                      # the keyring entry, now bw-bound-v1
$H run python3 -I tools/real-device/linux/tray.py layout
$H run python3 -I tools/real-device/linux/tray.py click "Show status"
$H shot popover                               # $BWH_RUN/shots/popover.png
$H stop
```

To change what the stand-in serves, rewrite its state file: it is read on every request.

## Linux: the pieces

| File | What it does |
|---|---|
| `harness.sh` | Every command above; `harness.sh` with no argument prints the list. |
| `session.sh` | Runs inside `dbus-run-session`: starts Xvfb (it picks a free display), the keyring, `desktopd.py` and a `dbus-monitor` log of keyring prompts. |
| `desktopd.py` | Stand-in notification server (records each notification in `log/notifications.jsonl`; `harness.sh click-notification <id>` invokes its action) and StatusNotifierWatcher (records the tray item in `log/sni.jsonl`). |
| `gitlab_standin.py` | Loopback GitLab v4 stand-in; records every request with its headers. |
| `tray.py` | Reads and clicks the app's tray menu over dbusmenu, as a tray host would. |
| `xt.py` | A small X11 driver over ctypes: list and find windows, click, type, send keys, resize. |
| `keyring.py` | Locks and unlocks the throwaway keyring (`harness.sh keyring lock`), for the locked-keyring checks; `harness.sh cancel-prompts <s>` dismisses the unlock prompts it raises. |
| `reqs.py` | The stand-in's request log, summarised by made-up token and path. |
| `bin/xdg-open`, `bin/gio` | First on the session's PATH: record what the app asked to open in `log/xdg-open.log` or `log/gio.log`, and open nothing. |

Logs are in `$BWH_RUN/log` and captures in `$BWH_RUN/shots`; the run directory stays after
`stop` for them. Delete it when you are done: it holds the keyring and its password.
