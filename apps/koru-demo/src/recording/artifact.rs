// SPDX-License-Identifier: MPL-2.0

//! Versioned recording JSON. Serialization stays in this runner.

use super::timeline::{
    CHORD_ALIGNMENT_MS, CLICK_ALIGNMENT_MS, Event, EventKind, TIMING_SAMPLE, events_from_results,
    geometry_unchanged, intervals_ms, nearest_frame, origin_ok,
};
use crate::{
    execute::StepResult,
    inspect::{ElementFacts, Rect},
    locator::LocatorPattern,
};
use koru_recording::{CaptureGeometry, FrameStamp, center, to_capture_local};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    time::Instant,
};

pub const CONTRACT: &str = "koru-demo-recording/1";
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct Artifact {
    pub case: String,
    pub process_id: u32,
    pub window_handle: i64,
    pub geometry: CaptureGeometry,
    pub frames: Vec<FrameStamp>,
    pub events: Vec<PairedEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedEvent {
    pub step: String,
    pub operation: String,
    pub kind: EventKind,
    pub at_ms: u64,
    pub nearest_frame: Option<u32>,
    pub alignment_ms: Option<i64>,
    pub target: Option<ElementFacts>,
    pub capture_bounds: Option<Rect>,
    pub matched: Option<bool>,
}

impl PairedEvent {
    pub fn capture_center(&self) -> Option<(i32, i32)> {
        self.capture_bounds.map(center)
    }
}

pub fn pair(
    case: &str,
    process_id: u32,
    window_handle: i64,
    geometry: CaptureGeometry,
    frames: &[FrameStamp],
    events: &[Event],
) -> Result<Artifact, &'static str> {
    if case != "document-edit" && case != "task-reminder" {
        return Err("unknown-case");
    }
    origin_ok(frames)?;
    geometry_unchanged(geometry, frames)?;
    let paired = events
        .iter()
        .map(|event| {
            let (nearest, alignment) = match nearest_frame(frames, event.at_ms) {
                Some(pair) => (Some(pair.0), Some(pair.1)),
                None => (None, None),
            };
            PairedEvent {
                step: event.step.into(),
                operation: event.operation.into(),
                kind: event.kind,
                at_ms: event.at_ms,
                nearest_frame: nearest,
                alignment_ms: alignment,
                target: event.target.clone(),
                capture_bounds: event
                    .target
                    .as_ref()
                    .and_then(|facts| facts.bounds)
                    .map(|bounds| to_capture_local(bounds, geometry.window)),
                matched: event.matched,
            }
        })
        .collect();
    Ok(Artifact {
        case: case.into(),
        process_id,
        window_handle,
        geometry,
        frames: frames.to_vec(),
        events: paired,
    })
}

pub fn from_results(
    case: &str,
    process_id: u32,
    window_handle: i64,
    geometry: CaptureGeometry,
    frames: &[FrameStamp],
    results: &[StepResult],
    origin: Instant,
) -> Result<Artifact, &'static str> {
    pair(
        case,
        process_id,
        window_handle,
        geometry,
        frames,
        &events_from_results(results, origin),
    )
}

fn redacted_target(facts: &ElementFacts) -> Value {
    json!({
        "locator": facts.locator.as_deref().and_then(|value| {
            LocatorPattern::parse(value).ok().map(|id| id.to_string())
        }),
        "role": facts.role.as_str(),
        "control_type": facts.role.control_type(),
        "enabled": facts.enabled,
        "offscreen": facts.offscreen,
        "bounds": rectangle(facts.bounds),
        "process_id": facts.process_id,
    })
}

fn rectangle(value: Option<Rect>) -> Value {
    value
        .map(|r| json!({"left": r.left, "top": r.top, "right": r.right, "bottom": r.bottom}))
        .unwrap_or(Value::Null)
}

fn code(value: &str) -> &str {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        value
    } else {
        "redacted-diagnostic"
    }
}

fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Input => "input",
        EventKind::Assertion => "assertion",
    }
}

impl Artifact {
    pub fn inputs(&self) -> impl Iterator<Item = &PairedEvent> {
        self.events
            .iter()
            .filter(|event| event.kind == EventKind::Input)
    }

    pub fn to_json(&self) -> Value {
        let intervals = intervals_ms(&self.frames);
        json!({
            "version": VERSION,
            "contract": CONTRACT,
            "case": code(&self.case),
            "origin": "first-encoded-frame",
            "time_measures": "native-input-delivery-complete",
            "time_does_not_measure": "application-processing-complete",
            "codec": "h264",
            "audio": false,
            "cursor": false,
            "frame_rate_contract": "not-assumed-constant",
            "timing": {
                "click_alignment_ms": CLICK_ALIGNMENT_MS,
                "chord_alignment_ms": CHORD_ALIGNMENT_MS,
                "source": TIMING_SAMPLE,
            },
            "process_id": self.process_id,
            "window_handle": self.window_handle,
            "capture": {
                "width": self.geometry.width,
                "height": self.geometry.height,
                "window": rectangle(Some(self.geometry.window)),
                "title_bar_height": self.geometry.title_bar_height,
                "dpi_scale_milli": self.geometry.dpi_scale_milli,
                "coordinate_space": "physical-screen-pixels",
                "transform": "uia-screen-minus-window-origin",
                "title_bar_applied": false,
                "dpi_applied": false,
                "shortcut_target": "focused-control",
            },
            "frames": {
                "count": self.frames.len(),
                "first_arrived_ms": self.frames.first().map(|frame| frame.arrived_ms),
                "last_arrived_ms": self.frames.last().map(|frame| frame.arrived_ms),
                "intervals_ms": intervals,
            },
            "events": self.events.iter().map(|event| {
                let (cx, cy) = event.capture_center().map(|(x, y)| (Some(x), Some(y))).unwrap_or((None, None));
                json!({
                    "kind": kind_name(event.kind),
                    "step": code(&event.step),
                    "operation": code(&event.operation),
                    "at_ms": event.at_ms,
                    "nearest_frame": event.nearest_frame,
                    "alignment_ms": event.alignment_ms,
                    "matched": event.matched,
                    "target": event.target.as_ref().map(redacted_target),
                    "capture_bounds": rectangle(event.capture_bounds),
                    "center": {"x": cx, "y": cy},
                })
            }).collect::<Vec<_>>(),
        })
    }

    pub fn write_json(&self, output: &mut impl Write) -> io::Result<()> {
        serde_json::to_writer_pretty(&mut *output, &self.to_json()).map_err(io::Error::other)?;
        output.write_all(b"\n")
    }

    pub fn result_manifest(
        &self,
        clip: &str,
        actions: &str,
        passed: bool,
        failure: Option<&str>,
    ) -> Value {
        json!({
            "version": VERSION,
            "contract": CONTRACT,
            "case": code(&self.case),
            "passed": passed,
            "failure": failure.map(code),
            "clip": clip,
            "actions": actions,
            "origin": "first-encoded-frame",
        })
    }

    pub fn timeline_markdown(&self) -> String {
        let mut out = String::from(
            "# Recording timeline\n\n\
             Origin is the first encoded frame. Times measure native input delivery, not application processing.\n\n\
             | kind | step | operation | at_ms | nearest_frame | alignment_ms |\n\
             | --- | --- | --- | ---: | ---: | ---: |\n",
        );
        for event in &self.events {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                kind_name(event.kind),
                event.step,
                event.operation,
                event.at_ms,
                event
                    .nearest_frame
                    .map(|index| index.to_string())
                    .unwrap_or_else(|| "-".into()),
                event
                    .alignment_ms
                    .map(|delta| delta.to_string())
                    .unwrap_or_else(|| "-".into()),
            ));
        }
        out.push_str(&format!(
            "\nTiming acceptance from {TIMING_SAMPLE}: click {CLICK_ALIGNMENT_MS} ms, chord/text {CHORD_ALIGNMENT_MS} ms.\n"
        ));
        out
    }

    pub fn validation_html(&self, stills: &[(String, String)]) -> String {
        let mut out = String::from(
            "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>Recording sample</title>\n\
             <style>body{font:16px/1.4 sans-serif;margin:24px}figure{margin:24px 0}\
             .frame{position:relative;display:inline-block;background:#111}\
             .frame img{display:block;max-width:100%}\
             .box{position:absolute;border:2px solid #e11;pointer-events:none}</style>\n\
             </head><body>\n\
             <h1>Recording sample</h1>\n\
             <p>Source clip has no captions, zoom, cursor styling or audio. \
             Boxes are UIA screen bounds minus the captured window origin. \
             Shortcut targets are the focused control.</p>\n",
        );
        for event in self.inputs() {
            let still = stills
                .iter()
                .find(|(step, _)| step == &event.step)
                .map(|(_, file)| file.as_str());
            let Some(file) = still else {
                continue;
            };
            out.push_str(&format!(
                "<figure><div class=\"frame\"><img src=\"{}\" alt=\"\">",
                file
            ));
            if let Some(bounds) = event.capture_bounds {
                let width = (bounds.right - bounds.left).max(0);
                let height = (bounds.bottom - bounds.top).max(0);
                out.push_str(&format!(
                    "<div class=\"box\" style=\"left:{}px;top:{}px;width:{}px;height:{}px\"></div>",
                    bounds.left, bounds.top, width, height
                ));
            }
            out.push_str(&format!(
                "</div><figcaption>{} at {} ms, nearest frame {}, alignment {} ms</figcaption></figure>\n",
                event.step,
                event.at_ms,
                event.nearest_frame.map(|n| n.to_string()).unwrap_or_else(|| "none".into()),
                event.alignment_ms.map(|n| n.to_string()).unwrap_or_else(|| "none".into()),
            ));
        }
        out.push_str("</body></html>\n");
        out
    }
}

/// JSON written when pairing cannot complete. The clip, if any, is left as a
/// partial artifact rather than discarded.
pub fn unpaired_manifest(case: &str, clip: &str, actions: &str, failure: &str) -> Value {
    json!({
        "version": VERSION,
        "contract": CONTRACT,
        "case": code(case),
        "passed": false,
        "failure": code(failure),
        "clip": clip,
        "actions": actions,
        "origin": "first-encoded-frame",
        "partial": true,
    })
}

pub fn unpaired_json(
    case: &str,
    process_id: u32,
    window_handle: i64,
    geometry: Option<&CaptureGeometry>,
    frames: &[FrameStamp],
    failure: &str,
) -> Value {
    json!({
        "version": VERSION,
        "contract": CONTRACT,
        "case": code(case),
        "origin": "first-encoded-frame",
        "time_measures": "native-input-delivery-complete",
        "time_does_not_measure": "application-processing-complete",
        "codec": "h264",
        "audio": false,
        "cursor": false,
        "passed": false,
        "failure": code(failure),
        "partial": true,
        "process_id": process_id,
        "window_handle": window_handle,
        "capture": geometry.map(|geometry| json!({
            "width": geometry.width,
            "height": geometry.height,
            "window": rectangle(Some(geometry.window)),
            "title_bar_height": geometry.title_bar_height,
            "dpi_scale_milli": geometry.dpi_scale_milli,
            "coordinate_space": "physical-screen-pixels",
            "transform": "uia-screen-minus-window-origin",
            "title_bar_applied": false,
            "dpi_applied": false,
            "shortcut_target": "focused-control",
        })),
        "frames": {
            "count": frames.len(),
            "first_arrived_ms": frames.first().map(|frame| frame.arrived_ms),
            "last_arrived_ms": frames.last().map(|frame| frame.arrived_ms),
            "intervals_ms": intervals_ms(frames),
        },
        "events": [],
    })
}

#[cfg(test)]
mod tests;
