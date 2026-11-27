// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! The verdict rules on the cases the recorded fixtures in `verdict.rs` do not
//! reach: an override that re-reads a bridge, a grandchild that failed, a
//! trigger job and child that disagree, markers that are ignored, parked or
//! cancelled, and the icon's last two rules.

use bridgewatch_core::config::schema::FailurePolicy;
use bridgewatch_core::config::{WatchRules, parse_str};
use bridgewatch_core::model::{Bridge, DownstreamPipeline, Job};
use bridgewatch_core::status::{JobClass, Status};
use bridgewatch_core::verdict::bridge::{self, BridgeVerdict};
use bridgewatch_core::verdict::deploy::{self, DeployOutcome, JobScope};
use bridgewatch_core::verdict::icon::{self, IconState, PipelineFacts};
use bridgewatch_core::verdict::job;

/// A watch's compiled rules, from the TOML lines that go under `[[watches]]`.
fn rules(extra: &str) -> WatchRules {
    let loaded = parse_str(
        &format!(
            r#"
            [accounts.gl]
            token = {{ env = "T" }}
            [[watches]]
            id = "x"
            account = "gl"
            project = 1
            {extra}
            "#
        ),
        std::path::Path::new("t.toml"),
    )
    .expect("parses");
    WatchRules::compile(&loaded.config.watches[0]).expect("compiles")
}

fn job(id: u64, name: &str, status: &str) -> Job {
    serde_json::from_value(serde_json::json!({ "id": id, "name": name, "status": status }))
        .expect("a job")
}

fn trigger(name: &str, status: &str, child: Option<&str>) -> Bridge {
    Bridge {
        id: 10,
        name: name.into(),
        status: Status::from(status.to_string()),
        stage: None,
        allow_failure: false,
        started_at: None,
        web_url: None,
        downstream_pipeline: child.map(|s| DownstreamPipeline {
            id: 99,
            project_id: Some(1),
            sha: None,
            ref_name: None,
            status: Status::from(s.to_string()),
            web_url: None,
        }),
    }
}

mod bridges {
    use super::*;

    #[test]
    fn an_unknown_verdict_is_worded_unknown() {
        let v = BridgeVerdict::Unknown {
            status: "new".into(),
        };
        assert_eq!(v.word(), "unknown");
    }

    #[test]
    fn an_ignored_trigger_keeps_the_childs_own_verdict() {
        // The caller drops an ignored bridge; asked anyway, nothing is re-read.
        let r = rules(r#"jobs = { "trigger:web" = "ignore" }"#);
        let jobs = [job(1, "lint", "failed")];
        let v = bridge::verdict(
            &trigger("trigger:web", "failed", Some("failed")),
            Some(&jobs),
            &[],
            &r,
        );
        assert_eq!(
            v,
            BridgeVerdict::Failed {
                jobs: vec!["lint".into()]
            }
        );
    }

    #[test]
    fn a_gate_override_reads_a_cancelled_or_skipped_trigger_as_parked() {
        let r = rules(r#"jobs = { "trigger:web" = "gate" }"#);
        for status in ["canceled", "skipped"] {
            let v = bridge::verdict(
                &trigger("trigger:web", status, Some(status)),
                Some(&[]),
                &[],
                &r,
            );
            assert_eq!(
                v,
                BridgeVerdict::AwaitingGate {
                    jobs: vec!["trigger:web".into()]
                },
                "{status}"
            );
        }
    }

    #[test]
    fn a_failed_grandchild_is_the_childs_failure_named_by_its_jobs_or_its_trigger() {
        let r = rules("");
        let nested = [
            ("trigger:ios".to_string(), BridgeVerdict::Dead),
            (
                "trigger:android".to_string(),
                BridgeVerdict::Failed {
                    jobs: vec!["gradle".into()],
                },
            ),
        ];
        let v = bridge::verdict(
            &trigger("trigger:apps", "success", Some("success")),
            Some(&[]),
            &nested,
            &r,
        );
        assert_eq!(
            v,
            BridgeVerdict::Failed {
                jobs: vec!["trigger:ios".into(), "gradle".into()]
            }
        );
    }

    #[test]
    fn a_skipped_trigger_with_a_read_child_is_skipped() {
        let v = bridge::verdict(
            &trigger("trigger:web", "skipped", Some("skipped")),
            Some(&[]),
            &[],
            &rules(""),
        );
        assert_eq!(v, BridgeVerdict::Skipped);
    }

    #[test]
    fn a_live_trigger_whose_child_settled_on_a_failure_is_failed() {
        let jobs = [job(1, "lint", "failed")];
        let v = bridge::verdict(
            &trigger("trigger:web", "running", Some("failed")),
            Some(&jobs),
            &[],
            &rules(""),
        );
        assert_eq!(
            v,
            BridgeVerdict::Failed {
                jobs: vec!["lint".into()]
            }
        );
    }

    #[test]
    fn a_trigger_in_a_status_this_build_does_not_know_falls_back_to_the_child() {
        let v = bridge::verdict(
            &trigger("trigger:web", "brand_new", Some("success")),
            Some(&[]),
            &[],
            &rules(""),
        );
        assert_eq!(
            v,
            BridgeVerdict::Unknown {
                status: "brand_new".into()
            }
        );
    }

    #[test]
    fn an_undived_trigger_reads_the_child_when_the_two_disagree() {
        let r = rules("");
        // Still pending itself, while its child is parked at a manual job.
        let parked = bridge::verdict(
            &trigger("trigger:web", "pending", Some("manual")),
            None,
            &[],
            &r,
        );
        assert_eq!(parked, BridgeVerdict::AwaitingGate { jobs: vec![] });
        // Settled in a status this build does not know.
        let odd = bridge::verdict(
            &trigger("trigger:web", "success", Some("brand_new")),
            None,
            &[],
            &r,
        );
        assert_eq!(
            odd,
            BridgeVerdict::Unknown {
                status: "brand_new".into()
            }
        );
    }
}

mod jobs {
    use super::*;

    #[test]
    fn overrides_re_read_a_tolerated_failure_and_park_an_unbuilt_job() {
        let r = rules(r#"jobs = { "e2e" = "blocking", "deploy:prod" = "gate" }"#);
        assert_eq!(
            job::classify_parts("e2e", &Status::Failed, true, &r),
            JobClass::BlockingFailure
        );
        assert_eq!(
            job::classify_parts("deploy:prod", &Status::Skipped, false, &r),
            JobClass::Gate
        );
        assert_eq!(
            job::base_class(&Status::Unknown("new".into()), false),
            JobClass::Unknown
        );
    }
}

mod deploys {
    use super::*;

    fn scope(jobs: &[Job]) -> JobScope<'_> {
        JobScope {
            pipeline_id: 5,
            pipeline_live: false,
            jobs,
        }
    }

    #[test]
    fn a_cancelled_outcome_is_worded_canceled() {
        assert_eq!(DeployOutcome::Canceled.word(), "canceled");
    }

    #[test]
    fn an_ignored_marker_is_no_marker_at_all() {
        let r = rules(
            r#"deploy_markers = ["deploy:prod", "deploy:eu", "deploy:old"]
            jobs = { "deploy:old" = "ignore" }"#,
        );
        let jobs = [job(1, "deploy:old", "success")];
        assert_eq!(
            deploy::outcome(&[scope(&jobs)], &r, false, None),
            DeployOutcome::Absent
        );
    }

    #[test]
    fn markers_that_all_wait_at_a_gate_are_no_deploy_yet() {
        let r = rules(r#"deploy_markers = ["deploy:prod", "deploy:eu", "deploy:old"]"#);
        let jobs = [
            job(1, "deploy:prod", "manual"),
            job(2, "deploy:eu", "manual"),
        ];
        assert_eq!(
            deploy::outcome(&[scope(&jobs)], &r, false, None),
            DeployOutcome::Absent
        );
    }

    #[test]
    fn markers_that_were_cancelled_or_parked_read_cancelled() {
        let r = rules(r#"deploy_markers = ["deploy:prod", "deploy:eu", "deploy:old"]"#);
        let jobs = [
            job(1, "deploy:prod", "canceled"),
            job(2, "deploy:eu", "manual"),
        ];
        assert_eq!(
            deploy::outcome(&[scope(&jobs)], &r, false, None),
            DeployOutcome::Canceled
        );
    }
}

mod icons {
    use super::*;

    fn facts(deploy: &DeployOutcome) -> PipelineFacts<'_> {
        PipelineFacts {
            deploy,
            parent_blocking: false,
            bridge_failed_or_dead: false,
            sibling_failure: false,
            post_deploy_failure: false,
            anything_live: false,
            awaiting_gate: false,
            parent_not_built: false,
            any_blocking_failure: false,
            marker_scope_failure: false,
            detail_unavailable: false,
            sibling_policy: FailurePolicy::Downgrade,
            post_deploy_policy: FailurePolicy::Downgrade,
        }
    }

    #[test]
    fn a_cancelled_deploy_is_a_cancelled_icon() {
        assert_eq!(
            icon::state(&facts(&DeployOutcome::Canceled)),
            IconState::Canceled
        );
    }

    #[test]
    fn a_blocking_failure_no_rule_explains_is_unknown_rather_than_green() {
        let absent = DeployOutcome::Absent;
        let f = PipelineFacts {
            any_blocking_failure: true,
            ..facts(&absent)
        };
        assert_eq!(icon::state(&f), IconState::Unknown);
    }
}

mod scripts {
    use bridgewatch_core::verdict::{PipelineView, ScriptError, VerdictScript};

    fn view() -> PipelineView {
        serde_json::from_str(
            r#"{"id":7,"iid":1,"sha":"4cfaced9d33af808","sha7":"4cfaced","ref":"main","source":"push",
                "status":"success","web_url":null,"state":"deployed","deploy":"live","deploy_marker":null,
                "deploy_failures":[],"failures":[],"warnings":[],"gates":[],"post_deploy_failures":[],
                "sibling_failures":[],"bridges":[],"parent_jobs":[],"updated_at":null,"created_at":null,
                "live":false}"#,
        )
        .expect("view decodes")
    }

    #[test]
    fn a_script_is_read_from_its_file_and_a_missing_file_says_which() {
        let dir = std::env::temp_dir().join(format!("bw-verdict-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("verdict.rhai");
        std::fs::write(&file, r#""running""#).unwrap();
        let script = VerdictScript::from_path(&file).expect("compiles");
        assert_eq!(script.evaluate(&view()).unwrap().as_str(), "running");
        let missing = dir.join("nope.rhai");
        match VerdictScript::from_path(&missing) {
            Err(ScriptError::Io { path, .. }) => assert!(path.ends_with("nope.rhai"), "{path}"),
            other => panic!("expected an Io error, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_script_that_returns_something_other_than_a_string_is_refused() {
        let script = VerdictScript::from_source("42").expect("compiles");
        match script.evaluate(&view()) {
            Err(ScriptError::BadReturn { returned }) => assert_eq!(returned, "i64"),
            other => panic!("expected BadReturn, got {other:?}"),
        }
    }
}

mod small {
    use super::*;

    /// A manual trigger whose child holds gates is parked on those gates,
    /// named by them rather than by the trigger.
    #[test]
    fn a_manual_trigger_is_parked_on_its_childs_own_gates() {
        let jobs = [job(1, "deploy:prod", "manual")];
        let v = bridge::verdict(
            &trigger("trigger:web", "manual", Some("manual")),
            Some(&jobs),
            &[],
            &rules(""),
        );
        assert_eq!(
            v,
            BridgeVerdict::AwaitingGate {
                jobs: vec!["deploy:prod".into()]
            }
        );
    }

    /// Every state name parses back to its state, `cancelled` included, and
    /// nothing else does.
    #[test]
    fn every_icon_state_name_parses_back() {
        for state in [
            IconState::Unknown,
            IconState::Failed,
            IconState::DeployedWithFailure,
            IconState::Deployed,
            IconState::Running,
            IconState::Canceled,
            IconState::ParkedGate,
            IconState::SucceededNoDeploy,
        ] {
            assert_eq!(IconState::parse(state.as_str()), Some(state));
        }
        assert_eq!(IconState::parse("cancelled"), Some(IconState::Canceled));
        assert_eq!(IconState::parse("green"), None);
    }

    /// A time past what a calendar can hold is shown as its raw seconds.
    #[test]
    fn a_stamp_out_of_range_is_its_seconds() {
        assert_eq!(
            bridgewatch_core::health::stamp(i64::MAX),
            i64::MAX.to_string()
        );
    }
}
