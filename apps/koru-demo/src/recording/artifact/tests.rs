// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{
    inspect::{ElementFacts, Handle, Rect, Role},
    recording::{
        CHORD_ALIGNMENT_MS, CLICK_ALIGNMENT_MS, CaptureGeometry, Event, EventKind, FrameStamp,
        TIMING_SAMPLE,
    },
};
use serde_json::{Value, json};

fn geometry() -> CaptureGeometry {
    CaptureGeometry {
        window: Rect {
            left: 100,
            top: 50,
            right: 1100,
            bottom: 850,
        },
        width: 1000,
        height: 800,
        title_bar_height: 32,
        dpi_scale_milli: 1250,
    }
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
fn frozen_json_names_the_contract_and_what_times_measure() {
    let artifact = pair(
        "document-edit",
        7,
        8,
        geometry(),
        &frames(&[0, 16]),
        &[input(
            "replace-selected-word",
            5,
            "koru:w1/v1/n:00000000000000000000000000000001#text",
        )],
    )
    .unwrap();
    let json = artifact.to_json();
    assert_eq!(json["version"], VERSION);
    assert_eq!(json["contract"], CONTRACT);
    assert_eq!(json["origin"], "first-encoded-frame");
    assert_eq!(json["time_measures"], "native-input-delivery-complete");
    assert_eq!(
        json["time_does_not_measure"],
        "application-processing-complete"
    );
    assert_eq!(json["codec"], "h264");
    assert_eq!(json["audio"], false);
    assert_eq!(json["cursor"], false);
    assert_eq!(json["timing"]["click_alignment_ms"], CLICK_ALIGNMENT_MS);
    assert_eq!(json["timing"]["chord_alignment_ms"], CHORD_ALIGNMENT_MS);
    assert_eq!(json["timing"]["source"], TIMING_SAMPLE);
    assert_eq!(json["capture"]["dpi_scale_milli"], 1250);
    assert_eq!(json["capture"]["dpi_applied"], false);
    assert_eq!(json["capture"]["title_bar_applied"], false);
    assert_eq!(json["capture"]["shortcut_target"], "focused-control");
    assert_eq!(json["events"][0]["kind"], "input");
    assert_eq!(json["events"][0]["center"]["x"], 140);
    assert_eq!(json["events"][0]["center"]["y"], 56);
    assert!(json.get("prototype").is_none());
}

#[test]
fn assertions_are_events_and_content_is_redacted() {
    let events = [
        input("replace-selected-word", 18, "secret document title"),
        Event {
            step: "replacement-visible",
            operation: "text-equals",
            kind: EventKind::Assertion,
            at_ms: 40,
            target: None,
            matched: Some(true),
        },
    ];
    let artifact = pair(
        "document-edit",
        7,
        8,
        geometry(),
        &frames(&[0, 20]),
        &events,
    )
    .unwrap();
    let mut encoded = Vec::new();
    artifact.write_json(&mut encoded).unwrap();
    let text = String::from_utf8(encoded).unwrap();
    assert!(!text.contains("secret"));
    assert!(!text.contains("document title"));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["events"].as_array().unwrap().len(), 2);
    assert_eq!(value["events"][1]["kind"], "assertion");
    assert_eq!(value["events"][1]["at_ms"], 40);
    assert_eq!(value["events"][0]["target"]["locator"], Value::Null);
}

#[test]
fn resize_refuses_pairing_and_writes_a_partial_manifest() {
    let mut stamps = frames(&[0, 16]);
    stamps[1].width = Some(1280);
    stamps[1].height = Some(720);
    assert_eq!(
        pair("document-edit", 7, 8, geometry(), &stamps, &[]).unwrap_err(),
        "capture-geometry-changed"
    );
    let json = unpaired_json(
        "document-edit",
        7,
        8,
        Some(&geometry()),
        &stamps,
        "capture-geometry-changed",
    );
    assert_eq!(json["partial"], true);
    assert_eq!(json["failure"], "capture-geometry-changed");
    assert_eq!(json["events"], json!([]));
    let manifest = unpaired_manifest(
        "document-edit",
        "clip.mp4",
        "actions.json",
        "capture-geometry-changed",
    );
    assert_eq!(manifest["passed"], false);
    assert_eq!(manifest["partial"], true);
}

#[test]
fn result_manifest_is_not_success_when_the_case_failed() {
    let artifact = pair(
        "task-reminder",
        7,
        8,
        geometry(),
        &frames(&[0]),
        &[Event {
            step: "open-tasks",
            operation: "click",
            kind: EventKind::Input,
            at_ms: 0,
            target: None,
            matched: Some(false),
        }],
    )
    .unwrap();
    let manifest = artifact.result_manifest("clip.mp4", "actions.json", false, Some("case-failed"));
    assert_eq!(manifest["passed"], false);
    assert_eq!(manifest["failure"], "case-failed");
    assert_eq!(manifest["case"], "task-reminder");
}

#[test]
fn validation_view_maps_input_boxes_onto_named_stills() {
    let artifact = pair(
        "document-edit",
        7,
        8,
        geometry(),
        &frames(&[0, 16]),
        &[input(
            "replace-selected-word",
            16,
            "koru:w1/v1/n:00000000000000000000000000000001#text",
        )],
    )
    .unwrap();
    let html = artifact.validation_html(&[(
        "replace-selected-word".into(),
        "replace-selected-word.png".into(),
    )]);
    assert!(html.contains("replace-selected-word.png"));
    assert!(html.contains("left:40px"));
    assert!(html.contains("top:40px"));
    let timeline = artifact.timeline_markdown();
    assert!(timeline.contains("replace-selected-word"));
    assert!(timeline.contains("native input delivery"));
}
