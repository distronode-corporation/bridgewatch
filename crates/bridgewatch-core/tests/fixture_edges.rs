//! The fixture transport, the recorder and the scrubber at their edges: a
//! broken `fixture.json`, a `list.json` holding one pipeline, a recording with
//! its list and a child in the parent's project, and files the scrubber cannot
//! read or has nothing to keep in.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bridgewatch_core::client::fixture::{self, ScrubMode};
use bridgewatch_core::client::{
    CiClient, FixtureTransport, HttpRequest, RequestRing, ScriptTransport, Transport, client_for,
};
use bridgewatch_core::config::{Account, ProjectRef};
use bridgewatch_core::token::Secret;

/// A directory in the temp dir, unique to one call, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, body: &str) {
        std::fs::write(self.0.join(name), body).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const PIPELINE: &str = r#"{"id":7,"iid":7,"project_id":1,"sha":"0123456789abcdef0123456789abcdef01234567",
    "ref":"main","status":"success","source":"push",
    "created_at":"2026-11-01T10:00:00Z","updated_at":"2026-11-01T10:05:00Z"}"#;

fn get(path: &str) -> HttpRequest {
    HttpRequest {
        method: "GET",
        url: format!("https://gitlab.example/api/v4{path}"),
        path: path.to_string(),
        headers: Vec::new(),
        body: None,
        anonymous: false,
        tail_bytes: None,
    }
}

#[test]
fn a_fixture_json_that_is_not_the_metadata_is_invalid_data() {
    let dir = TempDir::new("bw-fixture-bad-meta");
    dir.write("fixture.json", "[]");
    let error = FixtureTransport::load(&dir.0).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

/// Recording one pipeline by id writes `list.json` as that one object, and a
/// pipeline with no file of its own is served from it.
#[tokio::test]
async fn a_list_holding_one_pipeline_object_serves_it_by_id() {
    let dir = TempDir::new("bw-fixture-one");
    dir.write(
        "fixture.json",
        r#"{"primary":7,"project":1,"source":"synthetic"}"#,
    );
    dir.write("list.json", PIPELINE);
    let transport = FixtureTransport::load(&dir.0).unwrap();
    assert_eq!(transport.list().unwrap().len(), 1);

    let answer = transport
        .execute(get("/projects/1/pipelines/7"))
        .await
        .unwrap();
    assert_eq!(answer.status, 200);
    assert!(answer.body.contains("\"id\":7"), "{}", answer.body);
    let missing = transport
        .execute(get("/projects/1/pipelines/8"))
        .await
        .unwrap();
    assert_eq!(missing.status, 404);
    assert_eq!(transport.seen().len(), 2);
    transport.clear_seen();
    assert!(transport.seen().is_empty());

    dir.write("list.json", &format!("[{PIPELINE},{PIPELINE}]"));
    assert_eq!(transport.list().unwrap().len(), 2);

    dir.write("list.json", "{not json");
    assert_eq!(
        transport.list().unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

/// `--list` records the ref's list page too; a pipeline with no web URL is
/// named by its project and id; a child whose bridge names no project lives in
/// the parent's.
#[tokio::test]
async fn a_recording_with_its_list_and_a_child_in_the_parents_project() {
    let t = Arc::new(ScriptTransport::new());
    t.reply("pipelines/7/jobs", 200, "[]");
    t.reply(
        "pipelines/7/bridges",
        200,
        r#"[{"id":70,"name":"trigger:child","stage":"trigger","status":"success",
            "allow_failure":false,"downstream_pipeline":{"id":8,"sha":"0123456789abcdef0123456789abcdef01234567",
            "ref":"main","status":"success"}}]"#,
    );
    t.reply("projects/1/pipelines/8/jobs", 200, "[]");
    t.reply("pipelines?", 200, &format!("[{PIPELINE}]"));
    t.reply("pipelines/7", 200, PIPELINE);
    let client: Arc<dyn CiClient> = client_for(
        &Account::default(),
        &Secret::new("glpat-FIXTURE"),
        t.clone(),
        RequestRing::new(16),
    )
    .unwrap();

    let out = TempDir::new("bw-fixture-record");
    let recorded = fixture::record(client.as_ref(), &ProjectRef::Id(1), 7, &out.0, true)
        .await
        .expect("records");
    assert_eq!(recorded.children["trigger:child"], Some(8));
    assert!(recorded.files.contains(&"child-8.json".to_string()));
    assert_eq!(t.remaining(), 0, "the list and the child were asked for");
    let meta = std::fs::read_to_string(out.0.join("fixture.json")).unwrap();
    assert!(meta.contains("project 1 pipeline 7"), "{meta}");
}

#[test]
fn the_scrubber_refuses_a_file_that_is_not_json_and_keeps_what_has_no_keys() {
    let dir = TempDir::new("bw-fixture-scrub");
    dir.write("jobs.json", "[1, \"two\", null]\n");
    dir.write("expected.json", "not ours");
    let scrubbed = fixture::scrub_dir_with(&dir.0, ScrubMode::Check).unwrap();
    assert_eq!(scrubbed.examined, 1);
    assert_eq!(
        scrubbed.changed.len(),
        1,
        "re-rendered pretty: {scrubbed:?}"
    );

    dir.write("bridges.json", "{not json");
    let error = fixture::scrub_dir_with(&dir.0, ScrubMode::Check).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

/// ⛔ Linux file names are bytes. One that is not UTF-8 is judged by the same
/// prefixes as any other.
#[cfg(target_os = "linux")]
#[test]
fn a_file_name_that_is_not_utf8_is_judged_by_its_prefix() {
    use std::os::unix::ffi::OsStrExt as _;
    let dir = TempDir::new("bw-fixture-bytes");
    let name = std::ffi::OsStr::from_bytes(b"child-\xff.json");
    std::fs::write(dir.0.join(name), "[]\n").unwrap();
    let other = std::ffi::OsStr::from_bytes(b"notes-\xff.json");
    std::fs::write(dir.0.join(other), "[]\n").unwrap();
    let scrubbed = fixture::scrub_dir_with(&dir.0, ScrubMode::Check).unwrap();
    assert_eq!(scrubbed.examined, 1);
    assert!(scrubbed.changed.is_empty());
}

/// A file the recorder cannot write stops the recording with its path,
/// whichever file it is.
#[tokio::test]
async fn a_recording_that_cannot_write_a_file_says_which() {
    for blocked in ["pipeline-7.json", "child-8.json"] {
        let t = Arc::new(ScriptTransport::new());
        t.reply("pipelines/7/jobs", 200, "[]");
        t.reply(
            "pipelines/7/bridges",
            200,
            r#"[{"id":70,"name":"trigger:child","stage":"trigger","status":"success",
                "allow_failure":false,"downstream_pipeline":{"id":8,"project_id":1,
                "sha":"0123456789abcdef0123456789abcdef01234567","ref":"main","status":"success"}}]"#,
        );
        t.reply("pipelines/8/jobs", 200, "[]");
        t.reply("pipelines/7", 200, PIPELINE);
        let client = client_for(
            &Account::default(),
            &Secret::new("glpat-FIXTURE"),
            t,
            RequestRing::new(16),
        )
        .unwrap();
        let out = TempDir::new("bw-fixture-blocked");
        // A directory where the file should go: the write fails.
        std::fs::create_dir_all(out.0.join(blocked)).unwrap();
        let error = fixture::record(client.as_ref(), &ProjectRef::Id(1), 7, &out.0, false)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, fixture::RecordError::Io(path, _) if path.ends_with(blocked)),
            "{error:?}"
        );
    }
}
