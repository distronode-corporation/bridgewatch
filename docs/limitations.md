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
- **No poll on wake from sleep.** The first data after resume can be up to one interval
  stale; opening the popover polls at once.
- **Windows is not targeted.** Nothing is tested there, the keyring code least of all.
- **No merge-request pairing.** `merge_request_event` works as a source like any other,
  but a merged-result pipeline is not paired with its branch pipeline.
- **The wizard's deploy-marker suggestions look one level deep** (parent and direct
  children).
- **The wizard does not write a GitHub commit group.** It writes one row per run; set
  `group = "commit"` and `expect` in Settings or by hand.
- **A GitHub commit group is formed by time.** A run that starts more than
  `fan_out_secs` after the rest of its push (a slow `workflow_dispatch`, a re-push of the
  same commit) is a row of its own. See [`expect`](github-actions.md#expect-a-workflow-that-never-started).
