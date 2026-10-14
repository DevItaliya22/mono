// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{
    execute::{Expected, Observed, StepResult},
    inspect::{ElementFacts, Handle, Rect, Role},
};
use serde_json::Value;
use std::time::{Duration, Instant};

fn geometry() -> CaptureGeometry {
    CaptureGeometry::physical(
        Rect {
            left: 100,
            top: 50,
            right: 1100,
            bottom: 850,
        },
        1000,
        800,
        32,
    )
    .with_dpi(1250)
}

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

fn facts(locator: &str) -> ElementFacts {
    ElementFacts {
        handle: Handle {
            generation: 1,
            slot: 2,
        },
        locator: Some(locator.into()),
        role: Role::Edit,
        enabled: true,
        offscreen: false,
        bounds: Some(Rect {
            left: 140,
            top: 90,
            right: 340,
            bottom: 122,
        }),
        process_id: Some(7),
    }
}

fn input(step: &'static str, at_ms: u64, locator: &str) -> Event {
    Event {
        step,
        operation: "text",
        kind: EventKind::Input,
        at_ms,
        target: Some(facts(locator)),
        matched: Some(true),
    }
}

#[test]
fn first_encoded_frame_is_the_time_origin() {
    let artifact = pair(
        "document-edit",
        7,
        8,
        geometry(),
        &frames(&[0, 16, 33]),
        &[input(
            "replace-selected-word",
            0,
            "koru:w1/v1/n:00000000000000000000000000000001#text",
        )],
    )
    .expect("artifact");
    assert_eq!(artifact.frames[0].arrived_ms, 0);
    assert_eq!(artifact.events[0].at_ms, 0);
    assert_eq!(artifact.events[0].nearest_frame, Some(0));
    assert_eq!(artifact.events[0].alignment_ms, Some(0));
}

#[test]
fn a_delayed_or_dropped_frame_is_not_assumed_constant_fps() {
    let stamps = frames(&[0, 50, 150]);
    let at_140 = nearest_frame(&stamps, 140).expect("nearest");
    assert_eq!(at_140.0, 2);
    assert_eq!(at_140.1, 10);
    assert_ne!(at_140.0, (140 / 16) as u32);
    let artifact = pair(
        "document-edit",
        7,
        8,
        geometry(),
        &stamps,
        &[input(
            "undo",
            140,
            "koru:w1/v1/n:00000000000000000000000000000001#text",
        )],
    )
    .unwrap();
    let json = artifact.to_json();
    assert_eq!(json["frame_rate_contract"], "not-assumed-constant");
    assert_eq!(json["frames"]["intervals_ms"][0], 50);
    assert_eq!(json["frames"]["intervals_ms"][1], 100);
    assert_eq!(json["origin"], "first-encoded-frame");
}

#[test]
fn frozen_json_has_required_fields_and_redacts_content() {
    let artifact = pair(
        "document-edit",
        7,
        8,
        geometry(),
        &frames(&[0, 20]),
        &[input("replace-selected-word", 18, "secret document title")],
    )
    .unwrap();
    let mut encoded = Vec::new();
    artifact.write_json(&mut encoded).unwrap();
    let text = String::from_utf8(encoded).unwrap();
    assert!(!text.contains("secret"));
    assert!(!text.contains("document title"));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["case"], "document-edit");
    assert_eq!(value["codec"], "h264");
    assert_eq!(value["audio"], false);
    assert_eq!(value["cursor"], false);
    assert_eq!(value["contract"], CONTRACT);
    assert_eq!(value["version"], VERSION);
    assert!(value.get("prototype").is_none());
    assert_eq!(value["events"][0]["at_ms"], 18);
    assert_eq!(value["events"][0]["target"]["locator"], Value::Null);
    assert_eq!(value["capture"]["width"], 1000);
    assert_eq!(
        value["capture"]["coordinate_space"],
        "physical-screen-pixels"
    );
    assert_eq!(value["events"][0]["capture_bounds"]["left"], 40);
    assert_eq!(value["events"][0]["capture_bounds"]["top"], 40);
}

#[test]
fn from_results_includes_assertions_on_the_encoded_frame_origin() {
    let origin = Instant::now();
    let result = StepResult {
        step: "replace-selected-word",
        operation: "text",
        elapsed: Duration::from_millis(500),
        target: None,
        selection: None,
        failure: None,
        expected: Expected {
            kind: "text",
            boolean: None,
            content_redacted: true,
        },
        actual: Observed::default(),
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
        expected: Expected {
            kind: "text-equals",
            boolean: None,
            content_redacted: true,
        },
        actual: Observed::default(),
        delivered_at: None,
        finished_at: Some(origin + Duration::from_millis(55)),
    };
    let events = events_from_results(&[result, assertion], origin);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, EventKind::Input);
    assert_eq!(events[0].at_ms, 40);
    assert_eq!(events[1].kind, EventKind::Assertion);
    assert_eq!(events[1].at_ms, 55);
}

#[test]
fn empty_frames_cannot_invent_an_origin() {
    assert_eq!(
        pair("document-edit", 7, 8, geometry(), &[], &[]).unwrap_err(),
        "capture-not-ready"
    );
}
