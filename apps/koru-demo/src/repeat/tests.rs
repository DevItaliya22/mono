// SPDX-License-Identifier: MPL-2.0

// The case-table tests are `cfg(windows)` (they compile registry chords
// into virtual-key codes); their helpers stay for them.
#![cfg_attr(not(windows), allow(dead_code))]
use super::*;
use crate::execute::{Expected, Observed};
use std::{path::Path, time::Duration};
fn fixture() -> Fixture {
    Fixture {
        subject: crate::fixtures::model("00000000000000000000000000000001", "text").unwrap(),
        document: crate::fixtures::model("00000000000000000000000000000002", "root").unwrap(),
        entry: Some(crate::fixtures::model("00000000000000000000000000000003", "text").unwrap()),
        reminder_date: Some("04/12/2099".into()),
        reminder_time: Some("3:30 PM".into()),
    }
}
fn results(case: &Case, failure: Option<&'static str>) -> Vec<StepResult> {
    case.steps
        .iter()
        .take(if failure.is_some() {
            1
        } else {
            case.steps.len()
        })
        .map(|step| StepResult {
            step: step.id,
            operation: "fake",
            elapsed: Duration::ZERO,
            target: None,
            selection: None,
            failure,
            expected: Expected {
                kind: "fake",
                boolean: None,
                content_redacted: true,
            },
            actual: Observed::default(),
            delivered_at: None,
            finished_at: None,
        })
        .collect()
}
fn report(name: &str) -> RunReport {
    RunReport::new(name, Path::new("D:/isolated"), 42, 100, 10)
}
// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn ten_successes_use_nine_visible_resets_and_never_reset_after_the_last_case() {
    let mut report = report("document-edit");
    let mut phases = Vec::new();
    run(
        "document-edit",
        &vec![fixture(); 10],
        &mut report,
        |iteration, phase, case, _| {
            phases.push((iteration, phase.to_owned()));
            Ok(results(case, None))
        },
    );
    assert!(report.passed());
    assert_eq!(phases.len(), 19);
    assert_eq!(phases.last(), Some(&(10, "case".into())));
}
// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn missing_ambiguous_save_failure_and_window_close_stop_without_reset() {
    for code in ["not-found", "ambiguous", "save-failed", "target-destroyed"] {
        let mut report = report("document-edit");
        let mut calls = 0;
        run(
            "document-edit",
            &vec![fixture(); 10],
            &mut report,
            |_, phase, case, _| {
                calls += 1;
                assert_eq!(phase, "case");
                Ok(results(case, Some(code)))
            },
        );
        assert_eq!(calls, 1);
        assert!(!report.passed());
        let mut json = Vec::new();
        report.write_json(&mut json).unwrap();
        assert!(String::from_utf8(json).unwrap().contains(code));
    }
}
// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn a_failed_reset_or_report_checkpoint_stops_before_the_next_case() {
    let mut report = report("document-edit");
    let mut calls = 0;
    run(
        "document-edit",
        &vec![fixture(); 10],
        &mut report,
        |_, phase, case, _| {
            calls += 1;
            Ok(results(
                case,
                (phase == "reset").then_some("condition-timeout"),
            ))
        },
    );
    assert_eq!(calls, 2);
    assert!(!report.passed());
    let mut report = RunReport::new("document-edit", Path::new("D:/isolated"), 42, 100, 10);
    let mut calls = 0;
    run(
        "document-edit",
        &vec![fixture(); 10],
        &mut report,
        |_, _, _, _| {
            calls += 1;
            Err("report-write-failed")
        },
    );
    assert_eq!(calls, 1);
    assert!(!report.passed());
}
#[test]
fn reset_reaches_the_reopened_subject_and_gates_every_write_on_exact_content() {
    let reset = reset_document(&fixture()).unwrap();
    let position = |id| reset.steps.iter().position(|step| step.id == id).unwrap();
    assert!(position("reset-reach-subject") < position("reset-exact-after"));
    assert!(position("reset-exact-after") < position("reset-select-line"));
    assert!(position("reset-reach-subject") < position("reset-select-line"));
    assert!(position("reset-selection-exact") < position("reset-replace"));
    assert!(position("reset-saved") < position("reset-reach-entry"));
    assert_eq!(
        reset.steps.last().unwrap().operation,
        Operation::InDocumentViewport(false)
    );
    assert!(
        reset
            .steps
            .iter()
            .any(|step| step.operation == Operation::ReachByPageUp)
    );
}
// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn task_iterations_never_have_a_hidden_cleanup_or_reset_phase() {
    let mut report = report("task-reminder");
    let mut calls = 0;
    run(
        "task-reminder",
        &vec![fixture(); 10],
        &mut report,
        |_, phase, case, _| {
            calls += 1;
            assert_eq!(phase, "case");
            Ok(results(case, None))
        },
    );
    assert_eq!(calls, 10);
    assert!(report.passed());
}
