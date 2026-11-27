// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

//! Every word a status, a job class or a notification kind is written as.
//!
//! These are wire values: GitLab's status names, the view model's class names
//! and the notification ledger's keys. Each word here is checked against the
//! name serde gives the same value, so the two spellings cannot drift apart.

use bridgewatch_core::notify::NotifyKind;
use bridgewatch_core::status::{JobClass, Status};

#[test]
fn every_status_round_trips_through_its_word() {
    for word in [
        "created",
        "waiting_for_resource",
        "preparing",
        "pending",
        "running",
        "success",
        "failed",
        "canceling",
        "canceled",
        "skipped",
        "manual",
        "scheduled",
        "waiting_for_callback",
    ] {
        let status = Status::from(word.to_string());
        assert!(
            !matches!(status, Status::Unknown(_)),
            "{word} is a known status"
        );
        assert_eq!(status.as_str(), word);
        assert_eq!(String::from(status), word);
    }
    // A status this build does not know keeps the raw word both ways.
    let new = Status::from("brand_new".to_string());
    assert_eq!(new, Status::Unknown("brand_new".into()));
    assert_eq!(String::from(new), "brand_new");
}

#[test]
fn every_job_class_word_is_its_serde_name_and_its_display() {
    for class in [
        JobClass::Passed,
        JobClass::Live,
        JobClass::BlockingFailure,
        JobClass::WarningFailure,
        JobClass::Gate,
        JobClass::NotBuilt,
        JobClass::Unknown,
        JobClass::Ignored,
    ] {
        let serde_name = serde_json::to_value(class).unwrap();
        assert_eq!(serde_name.as_str(), Some(class.as_str()));
        assert_eq!(class.to_string(), class.as_str());
    }
}

#[test]
fn every_notification_kind_word_is_its_serde_name() {
    for kind in [
        NotifyKind::Deployed,
        NotifyKind::BlockingFailure,
        NotifyKind::Finished,
        NotifyKind::Started,
        NotifyKind::GateOpened,
        NotifyKind::Recovered,
        NotifyKind::Batch,
        NotifyKind::Token,
    ] {
        let serde_name = serde_json::to_value(kind).unwrap();
        assert_eq!(serde_name.as_str(), Some(kind.as_str()));
    }
}
