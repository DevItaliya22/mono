// SPDX-License-Identifier: MPL-2.0

// The case-table tests are `cfg(windows)` (they compile registry chords
// into virtual-key codes); their helpers stay for them.
#![cfg_attr(not(windows), allow(dead_code))]
use super::*;
#[cfg(windows)]
use crate::recording::{EventKind, events_from_results};
use crate::{
    cases,
    execute::{Expected, Observed, StepResult},
    inspect::Rect,
    recording::{CaptureGeometry, FrameStamp},
};
use std::time::{Duration, Instant};

fn geometry() -> CaptureGeometry {
    CaptureGeometry::physical(
        Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        },
        800,
        600,
        0,
    )
}

fn frames() -> Vec<FrameStamp> {
    vec![FrameStamp {
        index: 0,
        arrived_ms: 0,
        width: Some(800),
        height: Some(600),
        dpi_scale_milli: Some(1000),
    }]
}

fn fixture() -> cases::Fixture {
    cases::Fixture {
        subject: crate::fixtures::model("00000000000000000000000000000001", "text").unwrap(),
        document: crate::fixtures::model("00000000000000000000000000000002", "root").unwrap(),
        entry: Some(crate::fixtures::model("00000000000000000000000000000003", "text").unwrap()),
        reminder_date: Some("04/12/2099".into()),
        reminder_time: Some("3:30 PM".into()),
    }
}

fn result(
    step: &'static str,
    operation: &'static str,
    failure: Option<&'static str>,
    delivered: bool,
    origin: Instant,
) -> StepResult {
    StepResult {
        step,
        operation,
        elapsed: Duration::from_millis(10),
        target: None,
        selection: None,
        failure,
        expected: Expected {
            kind: operation,
            boolean: None,
            content_redacted: true,
        },
        actual: Observed {
            matched: Some(failure.is_none()),
            boolean: None,
        },
        delivered_at: delivered.then_some(origin + Duration::from_millis(5)),
        finished_at: Some(origin + Duration::from_millis(10)),
    }
}

fn passing_results(case: &cases::Case, origin: Instant) -> Vec<StepResult> {
    case.steps
        .iter()
        .map(|step| {
            let operation = match &step.operation {
                crate::cases::Operation::Input(action) => action.kind(),
                crate::cases::Operation::Expect(condition) => condition.kind(),
                crate::cases::Operation::Present => "present",
                crate::cases::Operation::Absent => "absent",
                crate::cases::Operation::InDocumentViewport(_) => "in-document-viewport",
                crate::cases::Operation::ReachByPageDown => "page-down-to-subject",
                crate::cases::Operation::ReachByPageUp => "page-up-to-subject",
            };
            result(
                step.id,
                operation,
                None,
                matches!(step.operation, crate::cases::Operation::Input(_)),
                origin,
            )
        })
        .collect()
}

fn attempt<'a>(
    frames: &'a [FrameStamp],
    results: &'a [StepResult],
    origin: Instant,
    capture_error: Option<&'static str>,
) -> Attempt<'a> {
    Attempt {
        case: "document-edit",
        process_id: 7,
        window_handle: 8,
        geometry: Some(geometry()),
        frames,
        results,
        origin: Some(origin),
        capture_error,
        clip: "clip.mp4",
        actions: "actions.json",
    }
}

#[test]
fn existing_outputs_are_refused_before_capture() {
    refuse_existing_paths(false, false, false).unwrap();
    assert_eq!(
        refuse_existing_paths(true, false, false).unwrap_err(),
        "clip-exists"
    );
    assert_eq!(
        refuse_existing_paths(false, true, false).unwrap_err(),
        "actions-exists"
    );
    assert_eq!(
        refuse_existing_paths(false, false, true).unwrap_err(),
        "result-exists"
    );
}

#[test]
fn a_passing_ui_case_is_not_success_when_the_encoder_was_unavailable() {
    let origin = Instant::now();
    let results = [result(
        "focus-entry",
        "page-up-to-subject",
        None,
        false,
        origin,
    )];
    let conclusion = conclude(Attempt {
        origin: None,
        geometry: None,
        frames: &[],
        capture_error: Some("encoder-unavailable"),
        ..attempt(&[], &results, origin, Some("encoder-unavailable"))
    });
    assert!(!conclusion.passed);
    assert_eq!(conclusion.failure, Some("encoder-unavailable"));
    assert!(conclusion.artifact.is_none());
    assert_eq!(conclusion.manifest["partial"], true);
}

#[test]
fn capture_not_ready_is_not_success_even_when_assertions_passed() {
    let origin = Instant::now();
    let results = [result(
        "focus-entry",
        "page-up-to-subject",
        None,
        false,
        origin,
    )];
    let conclusion = conclude(Attempt {
        origin: None,
        frames: &[],
        capture_error: Some("capture-not-ready"),
        ..attempt(&[], &results, origin, Some("capture-not-ready"))
    });
    assert!(!conclusion.passed);
    assert_eq!(conclusion.failure, Some("capture-not-ready"));
}

#[test]
fn finalize_failure_keeps_paired_events_and_is_not_success() {
    let origin = Instant::now();
    let stamps = frames();
    let results = [result("focus-entry", "click", None, true, origin)];
    let conclusion = conclude(attempt(
        &stamps,
        &results,
        origin,
        Some("encode-finalize-failed"),
    ));
    assert!(!conclusion.passed);
    assert_eq!(conclusion.failure, Some("encode-finalize-failed"));
    assert!(conclusion.artifact.is_some());
    assert_eq!(conclusion.actions_json["events"][0]["step"], "focus-entry");
}

#[test]
fn capture_loss_after_frames_is_not_success() {
    let origin = Instant::now();
    let stamps = frames();
    let results = [result("focus-entry", "click", None, true, origin)];
    let conclusion = conclude(attempt(
        &stamps,
        &results,
        origin,
        Some("capture-stop-failed"),
    ));
    assert!(!conclusion.passed);
    assert_eq!(conclusion.failure, Some("capture-stop-failed"));
}

#[test]
fn target_close_and_cancellation_are_structured_failures() {
    let origin = Instant::now();
    let stamps = frames();
    for code in ["target-destroyed", "cancelled"] {
        let results = [result("focus-entry", "click", Some(code), true, origin)];
        let conclusion = conclude(attempt(&stamps, &results, origin, None));
        assert!(!conclusion.passed);
        assert_eq!(conclusion.failure, Some(code));
        assert!(conclusion.artifact.is_some());
        assert_eq!(conclusion.manifest["passed"], false);
    }
}

#[test]
fn resize_during_the_clip_writes_partial_artifacts() {
    let origin = Instant::now();
    let mut stamps = frames();
    stamps[0].width = Some(1280);
    let results = [result("focus-entry", "click", None, true, origin)];
    let conclusion = conclude(attempt(&stamps, &results, origin, None));
    assert!(!conclusion.passed);
    assert_eq!(conclusion.failure, Some("capture-geometry-changed"));
    assert_eq!(conclusion.actions_json["partial"], true);
}

#[test]
fn a_clean_capture_and_case_is_success() {
    let origin = Instant::now();
    let stamps = frames();
    let results = [result("focus-entry", "click", None, true, origin)];
    let conclusion = conclude(attempt(&stamps, &results, origin, None));
    assert!(conclusion.passed);
    assert_eq!(conclusion.failure, None);
    assert_eq!(conclusion.manifest["contract"], crate::recording::CONTRACT);
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn record_and_test_share_the_compiled_document_and_task_steps() {
    let origin = Instant::now();
    for name in cases::names() {
        let case = cases::compile(name, &fixture()).expect(name);
        let results = passing_results(&case, origin);
        let events = events_from_results(&results, origin);
        let ids: Vec<_> = events.iter().map(|event| event.step).collect();
        let expected: Vec<_> = case.steps.iter().map(|step| step.id).collect();
        assert_eq!(ids, expected, "{name}");
        assert!(events.iter().any(|event| event.kind == EventKind::Input));
        assert!(
            events
                .iter()
                .any(|event| event.kind == EventKind::Assertion)
        );
        let conclusion = conclude(attempt(&frames(), &results, origin, None));
        assert!(conclusion.passed, "{name}");
        assert_eq!(
            conclusion.actions_json["events"].as_array().unwrap().len(),
            case.steps.len(),
            "{name}"
        );
    }
}
