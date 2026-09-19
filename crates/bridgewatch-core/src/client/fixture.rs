//! A [`Transport`] backed by recorded JSON on disk.
//!
//! This is what makes the verdict engine testable: a fixture directory holds one
//! real pipeline's API responses, and the whole stack above the transport runs
//! against it with no network, no token and no clock.
//!
//! It is public rather than test-only on purpose. `bridgewatch check --fixture`
//! and the GUI's offline demo both use it, and having one implementation means
//! the thing the tests exercise is the thing that ships.
//!
//! # Directory layout
//!
//! ```text
//! <name>/
//!   fixture.json      { "primary": <pipeline id>, "project": <project id>, "source": "..." }
//!   list.json         the `GET /pipelines` page, as an array (or a single row)
//!   jobs.json         the primary pipeline's jobs
//!   bridges.json      the primary pipeline's bridges
//!   jobs-<id>.json    another parent pipeline's jobs, when the list holds more than one
//!   bridges-<id>.json the same pipeline's bridges
//!   child-<id>.json   jobs of child pipeline <id>
//!   expected.json     what the verdict engine should say. Not served.
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::http::{HttpRequest, HttpResponse, Transport};
use super::wire::gitlab as wire;
use super::{CiClient, ClientError};
use crate::config::ProjectRef;
use crate::model::Pipeline;

/// The `fixture.json` metadata file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureMeta {
    /// The pipeline `jobs.json` and `bridges.json` belong to.
    pub primary: u64,
    /// The project the recording came from.
    pub project: u64,
    /// Where the data came from: a real pipeline URL, or what it was
    /// synthesised from and why.
    #[serde(default)]
    pub source: String,
}

/// A transport that answers from a recorded directory.
#[derive(Debug, Clone)]
pub struct FixtureTransport {
    dir: PathBuf,
    meta: FixtureMeta,
    /// Requests seen, in order. The planner tests assert on exactly this.
    seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl FixtureTransport {
    /// Load a fixture directory.
    pub fn load(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let meta_path = dir.join("fixture.json");
        let meta: FixtureMeta = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Self {
            dir,
            meta,
            seen: Default::default(),
        })
    }

    /// The fixture's metadata.
    pub fn meta(&self) -> &FixtureMeta {
        &self.meta
    }

    /// Every path requested so far, in order.
    pub fn seen(&self) -> Vec<String> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Forget the recorded request paths.
    pub fn clear_seen(&self) {
        if let Ok(mut s) = self.seen.lock() {
            s.clear();
        }
    }

    /// The pipelines in `list.json`, newest first, as the API would return them.
    pub fn list(&self) -> std::io::Result<Vec<Pipeline>> {
        let raw = std::fs::read_to_string(self.dir.join("list.json"))?;
        parse_list(&raw).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn body_for(&self, path: &str) -> Option<String> {
        let (route, _query) = match path.split_once('?') {
            Some((r, q)) => (r, q),
            None => (path, ""),
        };
        let segments: Vec<&str> = route.trim_start_matches('/').split('/').collect();
        // /projects/{project}/pipelines[/{id}[/jobs|/bridges]]
        match segments.as_slice() {
            ["projects", _project, "pipelines"] => self.read("list.json"),
            ["projects", _project, "pipelines", id] => {
                let id: u64 = id.parse().ok()?;
                self.read(&format!("pipeline-{id}.json")).or_else(|| {
                    let list = self.list().ok()?;
                    let found = list.into_iter().find(|p| p.id == id)?;
                    serde_json::to_string(&found).ok()
                })
            }
            ["projects", _project, "pipelines", id, "jobs"] => {
                let id: u64 = id.parse().ok()?;
                if id == self.meta.primary {
                    return self.read("jobs.json");
                }
                self.read(&format!("jobs-{id}.json"))
                    .or_else(|| self.read(&format!("child-{id}.json")))
            }
            ["projects", _project, "pipelines", id, "bridges"] => {
                let id: u64 = id.parse().ok()?;
                if id == self.meta.primary {
                    return self.read("bridges.json");
                }
                self.read(&format!("bridges-{id}.json"))
                    // A recorded child has no bridges of its own unless one was
                    // recorded; an empty array is the honest answer, not a 404.
                    .or_else(|| {
                        self.read(&format!("child-{id}.json"))
                            .map(|_| "[]".to_string())
                    })
            }
            _ => None,
        }
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join(name)).ok()
    }
}

#[async_trait::async_trait]
impl Transport for FixtureTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request.path.clone());
        }
        let (status, body) = match self.body_for(&request.path) {
            Some(body) => (200u16, body),
            None => (404u16, r#"{"message":"404 Not found"}"#.to_string()),
        };
        Ok(HttpResponse {
            status,
            body,
            // Fixtures are recorded fully paginated, so there is never a next
            // page; a fixture that lied here would hide a pagination bug.
            next_page: None,
            ratelimit_remaining: Some(1999),
            ratelimit_reset: None,
            retry_after: None,
            // A file on disk has no validator and no paging header. Answering
            // with one would invite a conditional-request path to believe a
            // fixture round trip proved something about a live 304.
            etag: None,
            link: None,
        })
    }
}

/// Decode a recorded `list.json` through GitLab's own decoder.
///
/// ⚠️ A fixture directory holds GitLab wire bytes, so it is read with the wire
/// types and converted, exactly as the client does with a live response. The
/// normalised model is what comes back, because that is what the callers of
/// [`FixtureTransport::list`] want.
fn parse_list(raw: &str) -> Result<Vec<Pipeline>, serde_json::Error> {
    // `list.json` may hold either the list page or a single pipeline object,
    // because recording one pipeline by id is the common case.
    match serde_json::from_str::<Vec<wire::Pipeline>>(raw) {
        Ok(v) => Ok(v.into_iter().map(Into::into).collect()),
        Err(_) => serde_json::from_str::<wire::Pipeline>(raw).map(|p| vec![p.into()]),
    }
}

// ---------------------------------------------------------------------------
// Scrubbing
// ---------------------------------------------------------------------------

/// Which allow-list applies to an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// A pipeline row or detail object, including a nested `downstream_pipeline`.
    Pipeline,
    /// A job or a bridge. Bridges are jobs that also carry
    /// `downstream_pipeline`, so one list covers both.
    Job,
}

/// Keys kept on a pipeline object. Everything else is dropped.
///
/// `started_at` and `finished_at` are here because [`crate::model::Pipeline`]
/// decodes them; the rest is what a human needs to follow the fixture.
pub const PIPELINE_KEYS: &[&str] = &[
    "id",
    "iid",
    "project_id",
    "sha",
    "ref",
    "status",
    "source",
    "created_at",
    "updated_at",
    "started_at",
    "finished_at",
    "web_url",
    "name",
];

/// Keys kept on a job or bridge object. Everything else is dropped.
///
/// `pipeline` and `downstream_pipeline` are recursed into as pipeline objects.
pub const JOB_KEYS: &[&str] = &[
    "id",
    "name",
    "stage",
    "status",
    "allow_failure",
    "created_at",
    "started_at",
    "finished_at",
    "duration",
    "web_url",
    "failure_reason",
    "pipeline",
    "downstream_pipeline",
];

/// Which allow-list a recorded file's objects take, by file name.
///
/// `None` means the file is bridgewatch's own (`fixture.json`, `expected.json`)
/// and is not a recorded API response. The guard test uses this same mapping, so
/// a file the scrubber skips is a file the test skips, and adding a new recorded
/// file name means changing one function.
pub fn kind_for_file(name: &str) -> Option<RecordKind> {
    if name == "list.json" || name.starts_with("pipeline-") {
        Some(RecordKind::Pipeline)
    } else if name == "jobs.json"
        || name.starts_with("jobs-")
        || name.starts_with("child-")
        || name == "bridges.json"
        || name.starts_with("bridges-")
    {
        Some(RecordKind::Job)
    } else {
        None
    }
}

/// Reduce a recorded API response to the keys bridgewatch needs, in place.
///
/// ⛔ **Fixtures are committed to a public repository and are recorded from a
/// private one.** A raw GitLab job payload carries the committer's real name and
/// email address (twice each), the full commit message and title, the pushing
/// user's profile — `job_title`, `location`, `organization`, `website_url`,
/// `linkedin`, `bio` — and the runner that executed it, including a self-hosted
/// project runner's IP address, system id, tags and free-text description.
/// Measured on the first ten recordings: 284 real email addresses, 284 commit
/// messages, and six runner descriptions naming internal machines.
///
/// ⛔ **This is an allow-list, and that is the whole point.** A deny-list is
/// only correct until GitLab adds a field. Anything not named in
/// [`PIPELINE_KEYS`] or [`JOB_KEYS`] is **dropped**, so a new API field cannot
/// leak by default — it disappears, and the guard test says which key and which
/// file if one ever needs adding back.
///
/// Dropped wholesale, rather than redacted, because nothing reads them: `user`,
/// `commit`, `runner`, `runner_manager`, `tag_list`, `artifacts`,
/// `artifacts_file`, `artifacts_expire_at`, `coverage`,
/// `queued_duration`, `project`, `detailed_status`, `before_sha`, `tag`,
/// `yaml_errors`, `archived`, `committed_at`, `erased_at`.
///
/// Everything the verdict engine reads is kept: job names, stages, statuses,
/// ids, `allow_failure`, timestamps, `downstream_pipeline`, and pipeline and job
/// `web_url`s. The project path is already public through the shipped example
/// config.
///
/// ⚠️ `scripts/record-fixture.sh` applies the same allow-lists with a `jq`
/// filter, and the two must be kept in step.
/// `fixtures_contain_no_personal_data` in `tests/verdict.rs` is what catches
/// them diverging: it asserts the allow-list against every committed fixture.
///
/// ```
/// use bridgewatch_core::client::fixture::{scrub, RecordKind};
/// use serde_json::json;
///
/// let mut value = json!([{
///     "name": "deploy:origins", "status": "success", "allow_failure": false,
///     "commit": { "title": "fix the thing", "author_email": "someone@example.com" },
///     "runner": { "description": "shared runner 1", "ip_address": "10.0.0.1" },
///     "user":   { "name": "A Person", "job_title": "Engineer" },
///     "downstream_pipeline": { "id": 7, "status": "failed", "yaml_errors": null }
/// }]);
/// scrub(&mut value, RecordKind::Job);
///
/// assert_eq!(value[0]["name"], "deploy:origins");        // job names are the point
/// assert_eq!(value[0]["downstream_pipeline"]["id"], 7);  // and so are bridges
/// for gone in ["commit", "runner", "user"] {
///     assert!(value[0].get(gone).is_none(), "{gone} must not survive");
/// }
/// assert!(value[0]["downstream_pipeline"].get("yaml_errors").is_none());
/// ```
pub fn scrub(value: &mut serde_json::Value, kind: RecordKind) {
    match value {
        serde_json::Value::Array(items) => items.iter_mut().for_each(|i| scrub(i, kind)),
        serde_json::Value::Object(_) => scrub_object(value, kind),
        _ => {}
    }
}

fn scrub_object(value: &mut serde_json::Value, kind: RecordKind) {
    let allowed = match kind {
        RecordKind::Pipeline => PIPELINE_KEYS,
        RecordKind::Job => JOB_KEYS,
    };
    let Some(map) = value.as_object_mut() else {
        return;
    };
    map.retain(|key, _| allowed.contains(&key.as_str()));

    // The two nested objects that survive are both pipelines.
    for key in ["pipeline", "downstream_pipeline"] {
        if let Some(child) = map.get_mut(key) {
            scrub(child, RecordKind::Pipeline);
        }
    }
}

/// Whether a scrub rewrites the files it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrubMode {
    /// Rewrite every file that would change.
    Write,
    /// Report what would change and touch nothing.
    Check,
}

/// Scrub every recorded `*.json` in a directory in place, preserving key order
/// and the two-space pretty formatting so the diff stays reviewable.
///
/// Files [`kind_for_file`] does not recognise — `fixture.json`, `expected.json`
/// — are bridgewatch's own and are left alone. Returns the files that changed;
/// running it twice changes nothing the second time.
pub fn scrub_dir(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    scrub_dir_with(dir, ScrubMode::Write)
}

/// [`scrub_dir`], with a say in whether anything is written.
///
/// ⛔ `ScrubMode::Check` exists because `fixture scrub --check` used to scrub
/// the directory for real and then write the old bytes back from a copy held in
/// memory: a gate whose failure mode is "modified the tree it was checking",
/// and which loses the file outright if the process is interrupted between the
/// two writes. A check must read.
pub fn scrub_dir_with(dir: &Path, mode: ScrubMode) -> std::io::Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    entries.sort();

    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(kind) = kind_for_file(name) else {
            continue;
        };
        let raw = std::fs::read_to_string(&path)?;
        let mut value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        scrub(&mut value, kind);
        let rendered = format!(
            "{}\n",
            serde_json::to_string_pretty(&value)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
        );
        if rendered != raw {
            if mode == ScrubMode::Write {
                std::fs::write(&path, rendered)?;
            }
            changed.push(path);
        }
    }
    Ok(changed)
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/// What [`record`] wrote, for the CLI to print.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// The directory written to.
    pub dir: PathBuf,
    /// The files written, in the order they were written.
    pub files: Vec<String>,
    /// The bridges found, and the child pipeline each created.
    pub children: BTreeMap<String, Option<u64>>,
}

/// Walk one parent pipeline through the client and write a fixture directory.
///
/// This is the same walk `scripts/record-fixture.sh` does with `glab`, through
/// the client instead, so it works without `glab` installed and exercises the
/// pagination the tests then rely on.
///
/// ⚠️ The directory layout it writes is GitLab's (`list.json`, `bridges.json`,
/// `child-<id>.json`), so this takes a [`CiClient`] for the calls but is not
/// yet provider-neutral in what it produces.
pub async fn record(
    client: &dyn CiClient,
    project: &ProjectRef,
    pipeline_id: u64,
    out: &Path,
    also_list: bool,
) -> Result<Recorded, RecordError> {
    std::fs::create_dir_all(out).map_err(|e| RecordError::Io(out.to_path_buf(), e))?;
    let mut files = Vec::new();

    let pipeline = client.get_pipeline(project, pipeline_id).await?;
    let project_id = pipeline.project_id.unwrap_or(0);

    if also_list {
        let query = super::ListQuery::exact(pipeline.ref_name.clone(), None, 10);
        let list = client.list_pipelines(project, &query).await?;
        write_json(out, "list.json", &list, &mut files)?;
    } else {
        write_json(out, "list.json", &vec![pipeline.clone()], &mut files)?;
    }
    write_json(
        out,
        &format!("pipeline-{pipeline_id}.json"),
        &pipeline,
        &mut files,
    )?;

    let jobs = client.pipeline_jobs(project, pipeline_id).await?;
    write_json(out, "jobs.json", &jobs, &mut files)?;

    let bridges = client.pipeline_bridges(project, pipeline_id).await?;
    write_json(out, "bridges.json", &bridges, &mut files)?;

    let mut children = BTreeMap::new();
    for bridge in &bridges {
        match &bridge.downstream_pipeline {
            Some(down) => {
                children.insert(bridge.name.clone(), Some(down.id));
                let child_project = down
                    .project_id
                    .map(ProjectRef::Id)
                    .unwrap_or_else(|| project.clone());
                let child_jobs = client.child_jobs(&child_project, down.id).await?;
                write_json(
                    out,
                    &format!("child-{}.json", down.id),
                    &child_jobs,
                    &mut files,
                )?;
            }
            None => {
                children.insert(bridge.name.clone(), None);
            }
        }
    }

    let meta = FixtureMeta {
        primary: pipeline_id,
        project: project_id,
        source: pipeline
            .web_url
            .clone()
            .unwrap_or_else(|| format!("project {project} pipeline {pipeline_id}")),
    };
    write_json(out, "fixture.json", &meta, &mut files)?;

    Ok(Recorded {
        dir: out.to_path_buf(),
        files,
        children,
    })
}

/// Why a recording failed.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// A GitLab call failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// A file could not be written.
    #[error("{0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    /// A response could not be re-serialised.
    #[error("cannot serialise {0}: {1}")]
    Serialize(String, #[source] serde_json::Error),
}

fn write_json<T: Serialize>(
    dir: &Path,
    name: &str,
    value: &T,
    files: &mut Vec<String>,
) -> Result<(), RecordError> {
    // Everything `record` writes goes through the allow-list first. Today the
    // typed model drops the personal fields anyway, by construction — but that
    // is an accident of which fields the engine happens to need, and the day
    // somebody adds `commit` to `Job` it would stop being true silently.
    let mut json =
        serde_json::to_value(value).map_err(|e| RecordError::Serialize(name.to_string(), e))?;
    if let Some(kind) = kind_for_file(name) {
        scrub(&mut json, kind);
    }
    let body = serde_json::to_string_pretty(&json)
        .map_err(|e| RecordError::Serialize(name.to_string(), e))?;
    let path = dir.join(name);
    std::fs::write(&path, format!("{body}\n")).map_err(|e| RecordError::Io(path, e))?;
    files.push(name.to_string());
    Ok(())
}
