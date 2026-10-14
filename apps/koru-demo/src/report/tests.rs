// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{
    cases::{Operation, Step},
    execute::{Expected, Observed},
    input::{Action, Condition},
    inspect::{Handle, Role},
};
use std::time::Duration;

fn case() -> Case {
    Case {
        name: "document-edit",
        steps: vec![Step {
            id: "check-value",
            target: Target::Named {
                role: Role::Edit,
                name: "private target title".into(),
                language: "en",
            },
            operation: Operation::Expect(Condition::Text("private expected text".into())),
        }],
    }
}
fn result(failure: Option<&'static str>) -> StepResult {
    StepResult {
        step: "check-value",
        operation: "text-equals",
        elapsed: Duration::from_millis(27),
        target: None,
        selection: Some(Selection::NotFound),
        failure,
        expected: Expected {
            kind: "text-equals",
            boolean: None,
            content_redacted: true,
        },
        actual: Observed {
            matched: Some(failure.is_none()),
            boolean: None,
        },
        delivered_at: None,
        finished_at: None,
    }
}
fn encoded(report: &RunReport) -> String {
    let mut out = Vec::new();
    report.write_json(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn failed_report_preserves_step_state_and_elapsed_without_content() {
    let mut report = RunReport::new("document-edit", Path::new("synthetic-root"), 7, 8, 10);
    report.add_case(1, "case", &case(), &[result(Some("state-timeout"))]);
    let text = encoded(&report);
    assert!(!text.contains("private"));
    let value: Value = serde_json::from_str(&text).unwrap();
    let step = &value["runs"][0]["steps"][0];
    assert_eq!(step["elapsed_ms"], 27);
    assert_eq!(step["failure"], "state-timeout");
    assert_eq!(step["actual"]["matched"], false);
    assert_eq!(step["selection"]["state"], "missing");
    assert!(!report.passed());
}

#[test]
fn ten_distinct_complete_cases_are_required_and_failed_reset_stays_failed() {
    let mut report = RunReport::new("document-edit", Path::new("synthetic-root"), 7, 8, 10);
    for iteration in 1..=9 {
        report.add_case(iteration, "case", &case(), &[result(None)]);
    }
    assert!(!report.passed());
    report.add_case(10, "case", &case(), &[result(None)]);
    assert!(report.passed());
    report.add_case(10, "reset", &case(), &[result(Some("state-timeout"))]);
    assert!(!report.passed());
}

#[test]
fn incomplete_and_duplicate_iterations_cannot_become_success() {
    let mut incomplete = RunReport::new("document-edit", Path::new("root"), 7, 8, 1);
    incomplete.add_case(1, "case", &case(), &[]);
    assert!(!incomplete.passed());
    let mut duplicate = RunReport::new("document-edit", Path::new("root"), 7, 8, 2);
    duplicate.add_case(1, "case", &case(), &[result(None)]);
    duplicate.add_case(1, "case", &case(), &[result(None)]);
    duplicate.add_case(2, "case", &case(), &[result(None)]);
    assert!(!duplicate.passed());
}

#[test]
fn setup_failure_is_a_serializable_report_and_error_prose_is_redacted() {
    let mut report = RunReport::new("document-edit", Path::new("root"), 7, 8, 1);
    report.fail("ownership", "instance-owner-mismatch");
    report.fail("provider", "private provider error: customer title");
    let text = encoded(&report);
    assert!(text.contains("instance-owner-mismatch"));
    assert!(!text.contains("customer"));
    assert!(!report.passed());
}

#[test]
fn content_enters_json_only_through_explicit_snapshot_and_never_debug() {
    let mut report = RunReport::new("document-edit", Path::new("root"), 7, 8, 1);
    let mut typed = case();
    typed.steps[0].operation = Operation::Input(Action::Text("private typed text".into()));
    report.add_case(1, "case", &typed, &[result(None)]);
    assert!(!encoded(&report).contains("private"));
    report.add_snapshot(
        1,
        ContentSnapshot::bounded(
            ElementFacts {
                handle: Handle {
                    generation: 1,
                    slot: 2,
                },
                locator: None,
                role: Role::Edit,
                enabled: true,
                offscreen: false,
                bounds: None,
                process_id: Some(7),
            },
            "explicit synthetic snapshot".into(),
            100,
        )
        .unwrap(),
    );
    assert!(encoded(&report).contains("explicit synthetic snapshot"));
    assert!(!format!("{report:?}").contains("synthetic snapshot"));
}

#[test]
fn checkpoints_replace_progress_but_do_not_count_an_unfinalized_case() {
    let mut report = RunReport::new("document-edit", Path::new("root"), 7, 8, 1);
    report.checkpoint_case(1, "case", &case(), &[]);
    report.checkpoint_case(1, "case", &case(), &[result(None)]);
    assert!(!report.passed());
    let progress: Value = serde_json::from_str(&encoded(&report)).unwrap();
    assert_eq!(progress["runs"].as_array().unwrap().len(), 1);
    assert_eq!(progress["runs"][0]["steps"].as_array().unwrap().len(), 1);
    assert_eq!(progress["runs"][0]["finalized"], false);
    report.add_case(1, "case", &case(), &[result(None)]);
    assert!(report.passed());
    let completed: Value = serde_json::from_str(&encoded(&report)).unwrap();
    assert_eq!(completed["runs"].as_array().unwrap().len(), 1);
    assert_eq!(completed["runs"][0]["finalized"], true);
}

#[test]
fn an_unfinished_reset_keeps_the_persisted_report_incomplete() {
    let mut report = RunReport::new("document-edit", Path::new("root"), 7, 8, 1);
    report.add_case(1, "case", &case(), &[result(None)]);
    assert!(report.passed());
    report.checkpoint_case(1, "reset", &case(), &[]);
    assert!(!report.passed());
    report.add_case(1, "reset", &case(), &[result(None)]);
    assert!(report.passed());
}
