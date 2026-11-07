# Notifications and noise

Notifications are for news, so bridgewatch says each thing once:

- **The first tick after a launch is silent.** It learns what is on screen; only changes
  after it notify.
- **A pipeline is reported failing once.** More jobs failing in the same pipeline, and
  that pipeline settling red, say nothing more; the popover lists every failure. A new
  push that fails is news again.
- **"Fixed again"** (`notify.recovered`, on by default): when a watch whose newest
  settled pipeline failed gets a newer one that passes, that push says "deployed, fixed
  again" (or "succeeded_no_deploy, fixed again") instead of its usual deploy or finish
  notification. On a failing watch, a deploy is announced when its pipeline finishes: as
  "fixed again" if it passed, or as "deployed" with a "blocking failure" if it didn't.
  Cancelled and parked pipelines neither break nor fix a watch. While a newer pipeline is
  still running (a new push, or a failed one being retried), an older green pipeline
  never counts as the fix, but a failure that settles meanwhile still counts.
- **A burst becomes one.** More than 3 notifications from one poll (a laptop waking up, a
  push fanning out across watches) are shown as one, "bridgewatch: 5 updates", quoting
  the first three. The CLI's `watch` still prints every one.
- **Pause notifications** in the tray menu silences them for 1 hour or until you resume,
  and the popover says so, with a Resume button. A pause lasts until the app quits.
  What arrives while paused is not saved up for later.
- **Secondary watches never notify**, and each kind can be turned off per watch
  (`[watches.notify]`).

## Failure alerts

A failure notification says which jobs failed and, on a second line, the **first failing
line**, and clicking it opens the failed job's page (`notify.click`, default
`first_failure_or_pipeline`).

- **GitLab:** the job's failure reason, in words ("script failure", "stuck or timeout
  failure"), from the jobs list bridgewatch already fetches. It costs nothing.
- **GitHub:** the first `failure` annotation of the job's check run, as `file:line:
  message` (a compiler error, a failing assertion), else GitHub's own "Process completed
  with exit code N". Only a failed run is asked, only for its first blocking failure
  (after `[watches.jobs]`), and only once per job: one request per failed run, none for a
  passing one, none on later polls. A rate limit or a server error is asked again on the
  next poll.
  A fine-grained token needs the **Checks: read** permission for it; without it the line
  is left out and everything else works.

The same line is shown under the failed job in the popover, and a failed job's
**copy** button puts the failure on the clipboard as Markdown (job, where it ran, the
first failing line and the link), ready for an issue or a chat.

Which desktops report a click on a notification (`tauri-plugin-notification` 2.8):

| OS | Click opens the job |
| --- | --- |
| macOS | Yes. |
| Linux, BSDs | Yes, on a notification server that offers actions (GNOME Shell and KDE Plasma do); one that does not reports nothing. |
| Windows | Only while the toast is on screen; not from the Action Center. |
