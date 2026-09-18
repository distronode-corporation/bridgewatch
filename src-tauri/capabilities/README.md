# Capabilities

`default.json` is the only capability, and it is scoped to the three windows (popover, settings, wizard) by
label rather than to `main`, which does not exist here. It grants
`core:event:default` (listening for `snapshot`, `config-changed` and
`open-settings`) and nothing else.

**The commands in `src/commands.rs` are not listed and do not need to be.**
Tauri 2's ACL governs core and plugin commands. A command the application
registers through `generate_handler!` is reachable from any window that can
reach the IPC at all, which is why `get_snapshot` and friends appear nowhere in
this file, and why the checks that matter live in Rust behind those commands
(`guard.rs`, `links.rs`, `read_stylesheet`).

**No plugin permission is granted to the webview.** Every plugin is driven from
Rust, where the ACL does not apply:

- `opener`: links go through the `open_link` command, which opens a URL only
  when its scheme, host and port are those of a configured account (`links.rs`).
  A static scope could not say that: it was `https://*`, which refused every
  link on a self-managed `http://` instance and allowed every https URL on the
  internet.
- `notification`: a tick's notifications are delivered by the poller.
- `positioner`: the popover is placed by `windows::show_popover`.
- `autostart`: the tray checkbox and the `set_launch_at_login` command.

Granting any of these to JavaScript would hand them to whatever script runs in
the webview for no feature that needs them. Add one only together with the
frontend call that uses it.
