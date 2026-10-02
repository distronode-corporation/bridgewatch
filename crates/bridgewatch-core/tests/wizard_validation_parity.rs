//! The wizard validates twice: in the webview, so a mistake is named on the
//! page that has it, and in the core, which is the authority. The two must
//! agree, and nothing held them to it: the webview's watch-id rule drifted
//! stricter than the core's and refused ids (`api.main`) that load fine.
//!
//! `src/components/wizard/__fixtures__/validation-cases.json` is one table of
//! answers and the fields each step must refuse. This file feeds every case to
//! [`wizard::validate_answers`]; `src/components/wizard/core-parity.test.ts`
//! feeds the same cases to the webview's `validateStep`. A rule changed on one
//! side only fails one of them, with the case's name.

use std::collections::BTreeSet;

use bridgewatch_core::wizard::{self, WizardAnswers, WizardStep};
use serde_json::{Map, Value};

/// The shared table, as the frontend test reads it.
const CASES: &str =
    include_str!("../../../src/components/wizard/__fixtures__/validation-cases.json");

/// `base` with the case's answers laid over it, key by key.
fn answers_for(base: &Map<String, Value>, case: &Value) -> WizardAnswers {
    let mut merged = base.clone();
    for (key, value) in case["answers"].as_object().expect("answers is an object") {
        merged.insert(key.clone(), value.clone());
    }
    serde_json::from_value(Value::Object(merged)).expect("the case is a valid WizardAnswers")
}

#[test]
fn every_shared_case_gets_the_core_verdict_the_table_names() {
    let table: Value = serde_json::from_str(CASES).expect("validation-cases.json parses");
    let base = table["base"].as_object().expect("base is an object");
    let cases = table["cases"].as_array().expect("cases is an array");
    // Guards against a table that silently lost its cases.
    assert!(cases.len() >= 10, "only {} cases", cases.len());

    let mut wrong = Vec::new();
    for case in cases {
        let name = case["name"].as_str().expect("a case has a name");
        let step: WizardStep =
            serde_json::from_value(case["step"].clone()).expect("step is a wizard step");
        let expected: BTreeSet<String> = case["fields"]
            .as_array()
            .expect("fields is an array")
            .iter()
            .map(|f| f.as_str().expect("a field is a string").to_string())
            .collect();
        let found: BTreeSet<String> = wizard::validate_answers(&answers_for(base, case))
            .into_iter()
            .filter(|issue| issue.step == step)
            .map(|issue| issue.field)
            .collect();
        if found != expected {
            wrong.push(format!(
                "{name}: expected {expected:?}, core says {found:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_base_answers_pass_every_step() {
    // Each case changes one thing; that only proves anything if the rest is
    // accepted everywhere.
    let table: Value = serde_json::from_str(CASES).unwrap();
    let base = table["base"].as_object().unwrap();
    let empty = serde_json::json!({ "answers": {} });
    assert_eq!(
        wizard::validate_answers(&answers_for(base, &empty)),
        Vec::new()
    );
}
