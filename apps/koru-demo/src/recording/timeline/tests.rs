// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{execute::StepResult, recording::CaptureGeometry};
use std::time::{Duration, Instant};

fn frames(times: &[u64]) -> Vec<FrameStamp> {
    times
        .iter()
        .enumerate()
        .map(|(index, arrived_ms)| FrameStamp {
            index: index as u32,
            arrived_ms: *arrived_ms,
            width: None,
            height: None,
            dpi_scale_milli: None,
        })
        .collect()
}

#[test]
fn first_encoded_frame_is_the_time_origin() {
    origin_ok(&frames(&[0, 16, 33])).unwrap();
    let at_0 = nearest_frame(&frames(&[0, 16, 33]), 0).unwrap();
    assert_eq!(at_0, (0, 0));
}

#[test]
fn a_nonzero_first_frame_index_is_not_an_origin() {
    let mut stamps = frames(&[0, 16]);
    stamps[0].index = 3;
    assert_eq!(
        origin_ok(&stamps).unwrap_err(),
        "origin-not-first-encoded-frame"
    );
}

#[test]
fn a_delayed_or_dropped_frame_is_not_assumed_constant_fps() {
    let stamps = frames(&[0, 50, 150]);
    let at_140 = nearest_frame(&stamps, 140).expect("nearest");
    assert_eq!(at_140.0, 2);
    assert_eq!(at_140.1, 10);
    assert_ne!(at_140.0, (140 / 16) as u32);
    assert_eq!(intervals_ms(&stamps), vec![50, 100]);
}

#[test]
fn accepted_sample_bounds_cover_clicks_and_chords() {
    assert!(within_acceptance("click", 0));
    assert!(within_acceptance("click", -1));
    assert!(within_acceptance("click", 5));
    assert!(!within_acceptance("click", 40));
    assert!(within_acceptance("chord", 22));
    assert!(within_acceptance("text", 16));
    assert!(!within_acceptance("chord", 40));
}

#[test]
fn empty_frames_cannot_invent_an_origin() {
    assert_eq!(origin_ok(&[]).unwrap_err(), "capture-not-ready");
}

#[test]
fn a_resized_or_dpi_changed_frame_fails_the_contract() {
    let geometry = CaptureGeometry::physical(
        crate::inspect::Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        },
        800,
        600,
        0,
    );
    let ok = frames(&[0, 16]);
    geometry_unchanged(geometry, &ok).unwrap();
    let mut odd_height = ok.clone();
    odd_height[1].width = Some(800);
    odd_height[1].height = Some(601);
    geometry_unchanged(geometry, &odd_height)
        .expect("odd WGC height vs even encoder size is not a resize");
    let mut resized = ok.clone();
    resized[1].width = Some(1024);
    resized[1].height = Some(768);
    assert_eq!(
        geometry_unchanged(geometry, &resized).unwrap_err(),
        "capture-geometry-changed"
    );
    let mut scaled = frames(&[0, 16]);
    scaled[1].dpi_scale_milli = Some(1250);
    assert_eq!(
        geometry_unchanged(geometry, &scaled).unwrap_err(),
        "capture-geometry-changed"
    );
}

#[test]
fn events_from_results_stamp_delivery_and_assertion_finish() {
    let origin = Instant::now();
    let input = StepResult {
        step: "replace-selected-word",
        operation: "text",
        elapsed: Duration::from_millis(40),
        target: None,
        selection: None,
        failure: None,
        expected: crate::execute::Expected {
            kind: "text",
            boolean: None,
            content_redacted: true,
        },
        actual: crate::execute::Observed::default(),
        delivered_at: Some(origin + Duration::from_millis(40)),
        finished_at: Some(origin + Duration::from_millis(41)),
    };
    let assertion = StepResult {
        step: "replacement-visible",
        operation: "text-equals",
        elapsed: Duration::from_millis(12),
        target: None,
        selection: None,
        failure: None,
        expected: crate::execute::Expected {
            kind: "text-equals",
            boolean: None,
            content_redacted: true,
        },
        actual: crate::execute::Observed {
            matched: Some(true),
            boolean: None,
        },
        delivered_at: None,
        finished_at: Some(origin + Duration::from_millis(55)),
    };
    let events = events_from_results(&[input, assertion], origin);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, EventKind::Input);
    assert_eq!(events[0].at_ms, 40);
    assert_eq!(events[1].kind, EventKind::Assertion);
    assert_eq!(events[1].at_ms, 55);
    assert_eq!(events[1].matched, Some(true));
}
