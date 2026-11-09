# Limitations

- **Polling, not push.** See [the job view](app.md#the-job-view-and-what-live-means).
- **Glob and regex refs, and more than one source, are filtered client-side over one
  page.** GitLab's pipelines API filters `ref` by exact name and `source` by one value,
  and GitHub's runs API filters `branch` and `event` the same way, so those watches read
  the newest page for the project (4 × `show.max_rows` pipelines or runs, between 30 and
  100) and filter it. A matching pipeline older than that page is not seen. On GitHub a
  page without `workflow` holds every workflow's runs, so it covers less time.
- **A notification click is reported only where the desktop reports it.** Windows reports
  it only while the toast is on screen, not from the Action Center, and a Linux
  notification server without actions reports nothing. The popover row carries the same
  link.
- **A poll on wake can come before the network.** A wake is noticed within 5 seconds of
  resume and polls at once; if the network is not back yet, that poll fails and the
  watch backs off as after any failed poll. Opening the popover or Refresh polls again.
- **The global shortcut does not work on Wayland.** `[ui].hotkey` registers through
  X11, the only Linux backend the hotkey library has, and a Wayland session gives apps
  no global shortcut of their own: under XWayland it fires only while an X11 window has
  the focus. It is off unless set, and the log says so when it is set in a Wayland
  session. There is no Wayland route yet (the desktop portal's GlobalShortcuts
  interface would be one, through D-Bus).
- **Windows is not targeted.** Nothing is tested there, the keyring code least of all.
- **No merge-request pairing.** `merge_request_event` works as a source like any other,
  but a merged-result pipeline is not paired with its branch pipeline.
- **The wizard's deploy-marker suggestions look one level deep** (parent and direct
  children).
- **A GitHub commit group is formed by time.** A run that starts more than
  `fan_out_secs` after the rest of its push (a slow `workflow_dispatch`, a re-push of the
  same commit) is a row of its own. See [`expect`](github-actions.md#expect-a-workflow-that-never-started).
