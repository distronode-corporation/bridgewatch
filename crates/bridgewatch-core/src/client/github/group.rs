//! The commit group: a commit's several workflow runs, folded into one
//! [`Pipeline`] whose bridges are those runs.
//!
//! GitHub has no object above a workflow run. One push starts one run per
//! workflow it matches, and no field on any of them points at the others
//! (`check_suite_id` is per run, measured). What a person means by "did my push
//! work" is therefore synthesised here, from the one list request the poller
//! already makes:
//!
//! | model | from |
//! | --- | --- |
//! | `Pipeline.id` | the largest run id in the group, so the newest group has the largest id and the icon, which follows the largest id, follows it |
//! | `Pipeline.sha` / `ref_name` / `source` | the runs' `head_sha` / `head_branch` / `event`, which the key makes identical |
//! | `Pipeline.status` | [`group_status`] over the runs' mapped statuses |
//! | `Pipeline.created_at` / `started_at` | the earliest run's |
//! | `Pipeline.updated_at` | the latest run's |
//! | `Pipeline.web_url` | `<web host>/<owner>/<repo>/commit/<sha>/checks`, GitHub's own page for the commit's checks |
//! | `PipelineDetail.jobs` | **empty**: the group has no jobs of its own |
//! | `PipelineDetail.bridges` | one per run, named for its WORKFLOW, `downstream_pipeline` = the run |
//! | `PipelineDetail.child_jobs[run]` | that run's `/jobs`, fetched only when `dive` selects it |
//!
//! Everything above the client then works unchanged: bridge verdicts, deploy
//! markers searched across every dived run, sibling and post-deploy failures,
//! the icon rules. A push whose `deploy.yml` succeeded while `lint.yml` failed
//! reads `deployed_with_failure`, exactly as a GitLab parent with a failed
//! sibling bridge does.
//!
//! # The key and the window
//!
//! ⛔ **`head_sha` alone is not a key.** A long-lived branch head collects every
//! scheduled run that ever fired against it; measured, thirteen runs on one sha
//! over ninety minutes, from four different events. So runs are keyed on
//! `(head_sha, event, head_branch)` and, within a key, a group is every run
//! created within the window of the group's NEWEST run. Newest first, so the
//! group that is still arriving is the one the window is measured from.
//!
//! The branch is in the key although the scoping report keyed on sha and event
//! alone: one commit pushed to two refs is two pushes, and a glob watch
//! (`release/*`) would otherwise file one branch's failure under the other's
//! row. With an exact `ref` the list is already one branch, so it changes
//! nothing there.
//!
//! A run whose `created_at` is missing or unreadable is a group of one. Nothing
//! can say how close it is to anything else, and merging on a guess is the one
//! mistake here that hides a failure.
//!
//! # `expect`: the dead bridge, restored from configuration
//!
//! GitLab's most valuable verdict is a trigger job that created no child. GitHub
//! cannot produce one: a workflow whose `on:` did not match leaves NO run
//! object, and neither, per the scoping notes, does every file GitHub refuses
//! before it starts (the ones it reports surface as a `startup_failure` run,
//! which is a failed run and read as one). An absence is not something a list
//! can return. A watch's `expect` names the workflow files a
//! group must hold, and a missing one is synthesised here, so the verdict
//! engine meets exactly the shape it already knows:
//!
//! | the group | a missing expected workflow is |
//! | --- | --- |
//! | settled, and its window has passed | a [`Bridge`] with no downstream pipeline, which [`crate::verdict`] reads `dead` |
//! | still live, or settled inside its window | a `pending` parent [`Job`] of the same name, and a settled group's own status is held `pending` |
//!
//! ⛔ **Timing is the part that matters.** GitHub creates a push's runs within
//! a second or two of each other, but "within the window" is the only promise
//! this module has, so inside it an absence is not yet a fact. While any run is
//! live the group reads live anyway, and saying `dead` there would announce a
//! failure about a group that has not finished. The pending job is what keeps
//! it honest in between: it makes the row read `running` rather than a green
//! that turns red a minute later (which would have notified `finished` first),
//! and holding a settled group's status `pending` is what makes the poller come
//! back for it, because a settled row whose `(status, updated_at)` has not moved
//! is never fetched again and the dead bridge would never be seen.
//!
//! An entry matches a run's `path` by FILE NAME. A run whose `path` is absent
//! matches nothing, which reads dead rather than guessing from its display name.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::client::wire::github as wire;
use crate::config::workflow_file;
use crate::model::{Bridge, DownstreamPipeline, Job, Pipeline};
use crate::status::Status;

/// The largest window [`fold`] honours: a century.
const MAX_WINDOW_SECS: u64 = 100 * 365 * 24 * 60 * 60;

/// The status of a synthetic bridge for an expected workflow that has no run.
///
/// Not one of GitHub's words, and deliberately not one of GitLab's either: the
/// run never existed, so `skipped` or `created` would each claim something that
/// did not happen. The verdict is `dead` whatever this says; it is what the view
/// and a verdict script read as the bridge's own status.
pub(super) const NEVER_STARTED: &str = "never_started";

/// One commit group: the row, its runs as bridges, oldest run first, and what
/// its `expect` adds.
#[derive(Debug, Clone)]
pub(super) struct Group {
    /// The synthetic parent.
    pub pipeline: Pipeline,
    /// The group's own jobs: one `pending` job per expected workflow still
    /// awaited, and otherwise none.
    pub jobs: Vec<Job>,
    /// One per run, then one dead bridge per expected workflow with no run.
    pub bridges: Vec<Bridge>,
}

/// What a watch's `expect` asks of every group, and what answering it needs:
/// the time, and where a missing workflow's own page is.
pub(super) struct Expect<'a> {
    /// `(as written, file name)`, first spelling kept, one per file name.
    workflows: Vec<(String, String)>,
    now: DateTime<Utc>,
    workflow_url: &'a dyn Fn(&str) -> String,
}

impl<'a> Expect<'a> {
    /// The expectations, deduplicated on the file each entry names. Blank
    /// entries are dropped; validation says so.
    pub fn new(
        entries: &[String],
        now: DateTime<Utc>,
        workflow_url: &'a dyn Fn(&str) -> String,
    ) -> Self {
        let mut workflows: Vec<(String, String)> = Vec::new();
        for entry in entries {
            let written = entry.trim();
            let file = workflow_file(written);
            if file.is_empty() || workflows.iter().any(|(_, f)| f == file) {
                continue;
            }
            workflows.push((written.to_string(), file.to_string()));
        }
        Self {
            workflows,
            now,
            workflow_url,
        }
    }
}

/// A run, with the two things grouping reads parsed once.
struct Member {
    created: Option<DateTime<Utc>>,
    run: wire::WorkflowRun,
}

/// Fold one page of runs into commit groups, newest group first.
///
/// `commit_url` turns a sha into the group's link. `page_full` says the list
/// came back with as many runs as were asked for, so older runs may exist that
/// were not returned.
///
/// ⛔ **On a full page the OLDEST groups may be cut in half, and a half group
/// is a wrong verdict**: the missing run could be the failed one. So every
/// group whose window reaches back to the oldest run on the page is dropped,
/// unless that would drop every group, in which case a partial answer is kept
/// rather than none (a primary watch with no rows reads `unknown`). The dropped
/// groups are the oldest, which are the rows a watch shows last, if at all.
pub(super) fn fold(
    runs: Vec<wire::WorkflowRun>,
    window_secs: u64,
    commit_url: &dyn Fn(&str) -> String,
    page_full: bool,
    expect: &Expect<'_>,
) -> Vec<Group> {
    // Clamped, because `Duration::seconds` and the date arithmetic below both
    // panic on overflow and the value is whatever the file says. A century is
    // "every run of this commit and event", which is what an absurd window asks.
    let window = chrono::Duration::seconds(window_secs.min(MAX_WINDOW_SECS) as i64);
    let oldest_on_page = runs
        .iter()
        .filter_map(|r| parse(r.created_at.as_deref()))
        .min();

    // Keyed in a BTreeMap so the output does not depend on hash order.
    let mut by_key: BTreeMap<(String, String, String), Vec<Member>> = BTreeMap::new();
    for run in runs {
        let key = (
            run.head_sha.clone(),
            run.event.clone().unwrap_or_default(),
            run.head_branch.clone().unwrap_or_default(),
        );
        by_key.entry(key).or_default().push(Member {
            created: parse(run.created_at.as_deref()),
            run,
        });
    }

    let mut groups: Vec<(Option<DateTime<Utc>>, Group)> = Vec::new();
    for (_, mut members) in by_key {
        // Newest first. An unreadable time sorts last and, below, never joins
        // or anchors anything.
        members.sort_by(|a, b| b.created.cmp(&a.created).then(b.run.id.cmp(&a.run.id)));
        let mut current: Vec<Member> = Vec::new();
        let mut anchor: Option<DateTime<Utc>> = None;
        for member in members {
            let joins = match (anchor, member.created) {
                (Some(newest), Some(created)) => newest - created <= window,
                _ => false,
            };
            if !joins && !current.is_empty() {
                groups.push((
                    anchor,
                    build(std::mem::take(&mut current), window, commit_url, expect),
                ));
            }
            if !joins {
                anchor = member.created;
            }
            current.push(member);
        }
        if !current.is_empty() {
            groups.push((anchor, build(current, window, commit_url, expect)));
        }
    }

    if page_full && let Some(oldest) = oldest_on_page {
        let complete = |anchor: &Option<DateTime<Utc>>| match anchor {
            Some(newest) => *newest - window > oldest,
            // A group of one with no readable time cannot have lost a member.
            None => true,
        };
        if groups.iter().any(|(anchor, _)| complete(anchor)) {
            groups.retain(|(anchor, _)| complete(anchor));
        } else {
            tracing::debug!(
                groups = groups.len(),
                "every commit group on this page may be missing runs older than the page; \
                 keeping them rather than showing nothing"
            );
        }
    }

    let mut out: Vec<Group> = groups.into_iter().map(|(_, g)| g).collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.pipeline.id));
    out
}

/// One group's row and bridges, from its members (at least one).
fn build(
    mut members: Vec<Member>,
    window: chrono::Duration,
    commit_url: &dyn Fn(&str) -> String,
    expect: &Expect<'_>,
) -> Group {
    // Bridges oldest run first: the order a push started them in, and a stable
    // one, which matters because the planner compares this tick's bridges with
    // the last tick's by name.
    members.sort_by_key(|m| m.run.id);
    let statuses: Vec<Status> = members
        .iter()
        .map(|m| Status::from_github(&m.run.status, m.run.conclusion.as_deref()))
        .collect();

    // Which expected workflows have no run here. Matched on the file, never on
    // the display name; see the module docs.
    let present: HashSet<&str> = members
        .iter()
        .filter_map(|m| m.run.path.as_deref())
        .map(workflow_file)
        .collect();
    let missing: Vec<&(String, String)> = expect
        .workflows
        .iter()
        .filter(|(_, file)| !present.contains(file.as_str()))
        .collect();
    let runs_status = group_status(&statuses);
    // Decided once every run has settled AND the window measured from the
    // newest run has passed, the same window that decides who joins the group.
    // A group with no readable creation time is decided on settling alone:
    // GitHub always sends `created_at`, and waiting on a clock that cannot be
    // read would hide the failure for good.
    let newest_created = members.iter().filter_map(|m| m.created).max();
    let decided =
        !runs_status.is_live() && newest_created.is_none_or(|newest| expect.now - newest > window);
    let status = if !missing.is_empty() && !decided {
        // Every run is settled or the group is live anyway; either way it is
        // not finished until the absence is a fact. See the module docs.
        if runs_status.is_live() {
            runs_status
        } else {
            Status::Pending
        }
    } else {
        runs_status
    };
    let url = |file: &str| Some((expect.workflow_url)(file));
    let (jobs, dead): (Vec<Job>, Vec<Bridge>) = if decided {
        let dead = missing
            .iter()
            .map(|(written, file)| Bridge {
                // No run, so no id; nothing looks a bridge up by it.
                id: 0,
                name: written.clone(),
                status: Status::Unknown(NEVER_STARTED.to_string()),
                stage: None,
                allow_failure: false,
                started_at: None,
                // The workflow's own page: the nearest thing an absence has
                // to a link, and where its history of runs can be read.
                web_url: url(file),
                // ⛔ The one field the verdict engine reads: no downstream
                // pipeline is `dead`, whatever the status says.
                downstream_pipeline: None,
            })
            .collect();
        (Vec::new(), dead)
    } else {
        let waiting = missing
            .iter()
            .enumerate()
            .map(|(i, (written, file))| Job {
                // Unique within the list, which is all it needs: a group has
                // no jobs of its own, so nothing else is in it.
                id: i as u64 + 1,
                name: written.clone(),
                status: Status::Pending,
                stage: None,
                allow_failure: false,
                started_at: None,
                finished_at: None,
                duration: None,
                web_url: url(file),
            })
            .collect();
        (waiting, Vec::new())
    };

    let newest = members
        .iter()
        .max_by_key(|m| m.run.id)
        .expect("a group has at least one run");
    let sha = newest.run.head_sha.clone();
    let pipeline = Pipeline {
        id: newest.run.id,
        // Run numbers are per workflow, so a group of several has no one
        // number to show; the id stands for it, as it does for any row
        // without one.
        iid: None,
        project_id: None,
        web_url: Some(commit_url(&sha)),
        ref_name: newest.run.head_branch.clone().unwrap_or_default(),
        source: newest.run.event.clone(),
        status,
        created_at: earliest(members.iter().map(|m| m.run.created_at.as_deref())),
        updated_at: latest(members.iter().map(|m| m.run.updated_at.as_deref())),
        started_at: earliest(members.iter().map(|m| m.run.run_started_at.as_deref())),
        // Nothing reports when a run finished, so nothing reports it for a
        // group of them either.
        finished_at: None,
        sha,
    };

    let bridges = members
        .into_iter()
        .zip(statuses)
        .map(|(m, status)| bridge_for(m.run, status))
        .chain(dead)
        .collect();
    Group {
        pipeline,
        jobs,
        bridges,
    }
}

/// A run, as the group's bridge.
fn bridge_for(run: wire::WorkflowRun, status: Status) -> Bridge {
    let name = run
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("workflow run {}", run.id));
    Bridge {
        id: run.id,
        name,
        status: status.clone(),
        stage: None,
        // Nothing in the API says a workflow's failure is tolerated, for the
        // same reason no job's is; `[watches.jobs]` names a workflow to say so.
        allow_failure: false,
        started_at: run.run_started_at,
        web_url: run.html_url.clone(),
        downstream_pipeline: Some(DownstreamPipeline {
            id: run.id,
            // The run is in the watch's own repository, which is what `None`
            // means to everything that fetches a child.
            project_id: None,
            sha: Some(run.head_sha),
            ref_name: run.head_branch,
            status,
            web_url: run.html_url,
        }),
    }
}

/// The status of a group, from its runs'.
///
/// "Worst" in the sense of what a person should be told, which is not one
/// fixed order over every status:
///
/// 1. Anything in flight makes the group in flight (`running` when any run is
///    running, else the first live status). A group is not settled until all of
///    its runs are, and a settled status here would stop the fast poll while a
///    workflow is still running.
/// 2. Then `failed`, then an unrecognised status, then `manual` (a run waiting
///    for approval), then `success`.
/// 3. Only then `canceled`, then `skipped`. ⚠️ These are last on purpose: a
///    group is "not built" only when NONE of its runs was built. GitHub skips a
///    whole workflow routinely (every job behind an `if:` that did not hold),
///    and the icon rules read a not-built parent as `canceled`, so ranking
///    `skipped` above `success` would grey out every push that has one such
///    workflow.
pub(super) fn group_status(statuses: &[Status]) -> Status {
    if statuses.contains(&Status::Running) {
        return Status::Running;
    }
    if let Some(live) = statuses.iter().find(|s| s.is_live()) {
        return live.clone();
    }
    if statuses.iter().any(Status::is_failed) {
        return Status::Failed;
    }
    if let Some(unknown) = statuses.iter().find(|s| matches!(s, Status::Unknown(_))) {
        return unknown.clone();
    }
    if statuses.iter().any(Status::is_gate) {
        return Status::Manual;
    }
    if statuses.iter().any(Status::is_success) {
        return Status::Success;
    }
    if statuses.contains(&Status::Canceled) {
        return Status::Canceled;
    }
    statuses
        .first()
        .cloned()
        .unwrap_or_else(|| Status::Unknown("empty".to_string()))
}

fn parse(value: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// The earliest readable timestamp, kept as GitHub wrote it.
fn earliest<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .flatten()
        .filter_map(|v| parse(Some(v)).map(|t| (t, v)))
        .min_by_key(|(t, _)| *t)
        .map(|(_, v)| v.to_string())
}

/// The latest readable timestamp, kept as GitHub wrote it.
fn latest<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .flatten()
        .filter_map(|v| parse(Some(v)).map(|t| (t, v)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, v)| v.to_string())
}

/// The groups the client has most recently presented, so that the row's
/// jobs and bridges and each run's jobs can be answered without listing again.
///
/// Keyed by the LIST REQUEST (its path), the window and the watch's `expect`,
/// so two watches on one repository with different filters, windows or
/// expectations cannot answer for each other. Each list call replaces its own
/// slot whole, which bounds the memo at one slot per distinct watch shape.
///
/// ⚠️ It is a cache of a response, not a source of truth: a row id it does not
/// hold is answered by asking GitHub for that commit's runs again (see
/// `GitHubClient::group_detail`), never by guessing.
#[derive(Debug, Default)]
pub(super) struct Memo {
    slots: HashMap<SlotKey, BTreeMap<u64, GroupDetail>>,
}

/// `(list path, window, expect as written)`.
type SlotKey = (String, u64, Vec<String>);

/// A group's own jobs and its bridges, as `listed_detail` answers them.
type GroupDetail = (Vec<Job>, Vec<Bridge>);

fn slot_key(list_path: &str, window: u64, expect: &[String]) -> SlotKey {
    (list_path.to_string(), window, expect.to_vec())
}

impl Memo {
    /// Replace one list's groups.
    pub fn replace(&mut self, list_path: &str, window: u64, expect: &[String], groups: &[Group]) {
        let slot = groups
            .iter()
            .map(|g| (g.pipeline.id, (g.jobs.clone(), g.bridges.clone())))
            .collect();
        self.slots.insert(slot_key(list_path, window, expect), slot);
    }

    /// Add one group to a list's slot without disturbing the others.
    pub fn insert(&mut self, list_path: &str, window: u64, expect: &[String], group: &Group) {
        self.slots
            .entry(slot_key(list_path, window, expect))
            .or_default()
            .insert(
                group.pipeline.id,
                (group.jobs.clone(), group.bridges.clone()),
            );
    }

    /// The jobs and bridges of the group `id` from that list, if it presented
    /// one.
    pub fn detail(
        &self,
        list_path: &str,
        window: u64,
        expect: &[String],
        id: u64,
    ) -> Option<GroupDetail> {
        self.slots
            .get(&slot_key(list_path, window, expect))?
            .get(&id)
            .cloned()
    }

    /// Whether `run` is a bridge of any group presented for the repository
    /// whose paths start with `repo_prefix`.
    ///
    /// This is what keeps `child_jobs` a no-request answer everywhere except
    /// for the runs a group actually handed out. An expected workflow's dead
    /// bridge has no downstream pipeline, so it is never one of them.
    pub fn presents(&self, repo_prefix: &str, run: u64) -> bool {
        self.slots
            .iter()
            .filter(|((path, _, _), _)| path.starts_with(repo_prefix))
            .flat_map(|(_, groups)| groups.values().flat_map(|(_, bridges)| bridges))
            .any(|b| b.downstream_pipeline.as_ref().is_some_and(|d| d.id == run))
    }
}

/// `scheme://host` of GitHub's WEB UI, from an account's `base_url`.
///
/// ⛔ The API host is not the web host on github.com. One leading `api.` comes
/// off, the rule the tray's "Open pipelines page" and `wizard::gh_service_for`
/// already use: `https://api.github.com` and a data-residency
/// `https://api.acme.ghe.com` lose it, and GitHub Enterprise Server keeps its
/// host, because there the API is a path (`/api/v3`) on the same one.
pub(super) fn web_origin(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    base.split_once("://")
        .map(|(scheme, rest)| format!("{scheme}://{}", rest.strip_prefix("api.").unwrap_or(rest)))
        .unwrap_or_else(|| base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_is_live_until_every_run_has_settled() {
        let s = |v: &[Status]| group_status(v);
        assert_eq!(s(&[Status::Failed, Status::Running]), Status::Running);
        assert_eq!(s(&[Status::Success, Status::Pending]), Status::Pending);
        assert_eq!(s(&[Status::Success, Status::Failed]), Status::Failed);
        assert_eq!(s(&[Status::Success, Status::Manual]), Status::Manual);
        assert_eq!(s(&[Status::Success, Status::Skipped]), Status::Success);
        assert_eq!(s(&[Status::Success, Status::Canceled]), Status::Success);
        assert_eq!(s(&[Status::Skipped, Status::Canceled]), Status::Canceled);
        assert_eq!(s(&[Status::Skipped]), Status::Skipped);
        assert_eq!(
            s(&[Status::Success, Status::Unknown("neutral".into())]),
            Status::Unknown("neutral".into())
        );
    }

    #[test]
    fn the_web_origin_loses_one_leading_api_and_nothing_else() {
        assert_eq!(web_origin("https://api.github.com"), "https://github.com");
        assert_eq!(web_origin("https://api.github.com/"), "https://github.com");
        assert_eq!(
            web_origin("https://api.acme.ghe.com"),
            "https://acme.ghe.com"
        );
        assert_eq!(web_origin("https://ghe.acme.com"), "https://ghe.acme.com");
    }
}
