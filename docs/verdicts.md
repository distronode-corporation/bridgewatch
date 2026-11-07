# Icon states and verdict rules

## Icon states

| State | Glyph (builtin) | Means |
| --- | --- | --- |
| `failed` | <img src="../src-tauri/icons/tray/color/failed@1x.png" width="22" height="22" alt=""> red octagon | The pipeline carrying the deploy marker failed before any marker succeeded, a bridge is dead (it never produced its downstream) and no marker exists, a blocking job in the parent failed, (with no marker) a bridge failed, or the pipeline failed before it had any job. |
| `deployed_with_failure` | <img src="../src-tauri/icons/tray/color/deployed_with_failure@1x.png" width="22" height="22" alt=""> amber triangle | A marker succeeded, and a sibling bridge failed or a blocking job failed around it, under the default `downgrade` policies. |
| `deployed` | <img src="../src-tauri/icons/tray/color/deployed@1x.png" width="22" height="22" alt=""> green filled check | A marker succeeded, no other marker can still move, and nothing is holding it back. |
| `running` | <img src="../src-tauri/icons/tray/color/running@1x.png" width="22" height="22" alt=""> blue arrows | A marker is in flight (even after another one succeeded), or (with no marker) something is still running. |
| `canceled` | <img src="../src-tauri/icons/tray/color/canceled@1x.png" width="22" height="22" alt=""> grey slash | The parent was canceled, or every marker was. A skipped pipeline ran nothing and is not listed. |
| `parked_gate` | <img src="../src-tauri/icons/tray/color/parked_gate@1x.png" width="22" height="22" alt=""> amber hourglass | No marker, nothing running, nothing broken, and a manual job or bridge is waiting for a person. |
| `succeeded_no_deploy` | <img src="../src-tauri/icons/tray/color/succeeded_no_deploy@1x.png" width="22" height="22" alt=""> green outline check | Everything settled green and no marker ran. Normal for a lane that does not deploy. |
| `unknown` | <img src="../src-tauri/icons/tray/color/unknown@1x.png" width="22" height="22" alt=""> grey question mark | No data yet, the pipeline's job lists could not be read, a marker is in a status this build does not know, or a verdict script failed. |

The icon reflects the newest pipeline of each primary watch. With more than one primary
watch the worst state wins (in the order `failed`, `deployed_with_failure`, `running`,
`canceled`, `parked_gate`, `unknown`, `deployed`, `succeeded_no_deploy`), and
`config validate` warns about it. On macOS the glyphs are monochrome template images
that follow the menu bar; on Linux they are coloured. The exact rules are under
[Verdict rules](#verdict-rules).

## Verdict rules

For the newest pipeline of each watch that matches its `ref` and `sources` (a `skipped`
pipeline, which ran nothing, is passed over), bridgewatch
reads the parent's jobs and bridges, then the jobs of every child pipeline that `dive`
selects, classifies each job, and looks for the `deploy_markers` across all of them. On
GitHub the "parent" is a workflow run (with no bridges), or with `group = "commit"` the
push, whose bridges are its runs and whose own jobs are empty. The same rules apply to
both. The state is the first rule that matches:

1. **`unknown`** if the pipeline's job lists could not be read and nothing was cached.
2. **`failed`** if no marker succeeded and the markers' own pipeline has a blocking
   failure (a marker that failed counts even when it was allowed to fail), or no marker
   job exists and a bridge is dead (a trigger job that never created its child pipeline,
   or an expected GitHub workflow that never started), or a blocking job in the parent
   failed (unless it started after a successful marker, which rule 4 handles), or there
   is no marker and any bridge failed, or the pipeline itself failed with no jobs and no
   bridges at all (a `.gitlab-ci.yml` that does not parse, a GitHub run that ended
   `startup_failure`). That last one is named `pipeline failed` in `failures`.
3. **`unknown`** if a marker is in a status this build does not recognise.
4. If a marker succeeded and no marker can still move: **`failed`** when a sibling failure or post-deploy failure
   applies with policy `fail`; **`deployed_with_failure`** when one applies with policy
   `downgrade`; otherwise **`deployed`**. A sibling failure is a failed or dead bridge
   other than the one carrying the marker; a post-deploy failure is a blocking job that
   started after the marker, or one in the marker's own pipeline.
5. **`running`** if a marker is in flight, or there is no marker and something is live.
   A job waiting on a manual gate is not live. A marker still queued after another one
   succeeded counts as in flight, and so does a live trigger job that `dive` would walk
   into whose child has not been read yet (it may not exist yet): a marker in there has
   not been seen.
6. **`canceled`** if the parent was canceled (or skipped, when a watch has nothing
   newer to show), or every marker was.
7. **`parked_gate`** if there is no marker, nothing is live, nothing blocking failed,
   and something is waiting on a manual gate. A marker that is itself a manual job counts
   as "no marker" here.
8. **`succeeded_no_deploy`** if there is no marker, nothing is live and nothing blocking
   failed.

`[verdict].script` replaces all of this for a watch's pipelines if your project needs
different rules. `bridgewatch check --json` prints each pipeline's facts (`deploy`,
`failures`, `warnings`, `gates`, `sibling_failures`, `post_deploy_failures`, the bridges
and their jobs), which is the fastest way to see why a pipeline got the state it did.
