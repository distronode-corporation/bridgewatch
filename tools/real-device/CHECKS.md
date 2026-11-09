# Release checks on real devices

Run before a release tag, on Linux with the harness in [`linux/`](linux/) and on a real
Mac by hand. Record each check as PASS or FAIL with its evidence (a capture, a log line,
a prompt count and the prompt's exact text). A FAIL is reported with that evidence; it is
not fixed inside the check.

Build from the commit that is going to be tagged, and say which builds you used. For the
upgrade checks, take the previous release's app or CLI from its GitHub Release and check
it against GitHub's recorded asset digest:

```
gh api repos/distronode-corporation/bridgewatch/releases/tags/v<previous> \
  --jq '.assets[] | "\(.name) \(.digest)"'
sha256sum <downloaded file>
```

GitLab is a loopback stand-in for every check: `linux/gitlab_standin.py` on Linux, and on
macOS either that file or the stand-in inside `.github/scripts/bound-token-probe.sh`. "A"
and "B" below are two stand-ins on different ports.

## Linux (the harness)

Every step runs inside the harness session (`harness.sh run`, `harness.sh launch`), and
every capture goes through `harness.sh shot`. See [README.md](README.md) for a whole run.

1. **Upgrade with a 1.x own token.** The 1.x app writes a bare entry into the throwaway
   keyring. The new app reads it, binds it on first use (`harness.sh entry` shows
   `bw-bound-v1` and A's origin) and polls A (`harness.sh reqs a`). Count keyring prompts
   (`log/secret-prompts.log`): none with the keyring unlocked. Then lock it
   (`harness.sh keyring lock`) and record what happens.
2. **Repoint the account from A to B in Settings.** The dialog text; the entry is bound
   to A before the move; B gets no request. With the keyring locked during the move
   (dismiss the prompt with `harness.sh cancel-prompts`), the move is refused and
   nothing is written.
3. **The tray "Pause notifications" submenu and Resume.** `tray.py layout` before and
   after each item; the labels and which items are enabled.
4. **The job tools on WebKitGTK.** They appear on hover, and Tab reaches them
   (`xt.py move`, `xt.py key Tab`).
5. **Notification click-through.** A failing pipeline on the stand-in (rewrite its state
   file). The notification is in `log/notifications.jsonl`;
   `harness.sh click-notification <id>` invokes it, and `log/xdg-open.log` or
   `log/gio.log` shows the job page the app asked to open.
6. **The popover grows and shrinks back.** Add many watches, then remove them. Measure
   the window with `xt.py list` before and after.
7. **The popover at its narrowest real width.** Shrink it with `xt.py resize`, then
   capture it.
8. **One-click setup and token health.** Run the setup wizard against the stand-in, and
   set `token_expires_at` in the state file to a date a few days ahead for the
   token-health strip.

## macOS (by hand)

Use a throwaway macOS user, or a new login keychain made for the test and set as the
default for it (`security create-keychain`, `security default-keychain -s`), restored
afterwards. Never touch a real bridgewatch keychain item or config. A person may need to
click the keychain prompts: that is the check, so count them.

1. **Upgrade with a 1.x own token.** Install the previous 1.x release, add an account
   with an own token against A, and let it poll. Quit, then run the new app. Does it
   read the token? How many keychain prompts, with which text ("bridgewatch wants to
   use..."), and does "Always Allow" stop them repeating? The entry is now bound:
   `security find-generic-password -s 'bridgewatch:<account>' -w` prints hex that decodes
   to `bw-bound-v1:`. Polling of A continues.
2. **The same through the new CLI** (`bridgewatch check`, `bridgewatch auth status`): the
   prompt count and the behaviour.
3. **Repoint in Settings, A to B.** The dialog text; the entry is bound to A before the
   move; B gets no request. With a prompt denied during the move: refused, the config
   unchanged, and still polling A.
4. **A downgrade.** The 1.x CLI against the bound entry exits non-zero and sends nothing.
5. **The tray "Pause notifications" submenu** and Resume: the labels and the state.
6. **Notification click-through.** A failing pipeline on the stand-in; the notification
   appears, and clicking it opens the job page.
7. **The popover** grows with many watches and shrinks back when they are removed; find
   its narrowest real width.

Clean up: remove the throwaway keychain or user, both installed apps and the stand-ins.
