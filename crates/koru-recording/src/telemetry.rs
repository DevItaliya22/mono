// SPDX-License-Identifier: MPL-2.0

//! `koru-session-recording/1` — the session-telemetry document written beside
//! a clip as `<clip>.actions.json` (the internal session-recorder design).
//!
//! **Observation, never content.** The document records the *kind* of thing a
//! person did and where the mouse was — enough for a Remotion composition to
//! re-animate the cursor and overlay "a menu opened here" — and it structurally
//! cannot carry what they typed, what a document said, or what a tab was
//! called. That is the privacy rule, and it is enforced by there being no
//! field that could hold such a value.
//!
//! All coordinates are capture-local **physical** pixels (the clip's own
//! frame), so a consumer maps them onto the video without knowing the display
//! scale. All times are milliseconds from the encoder's first frame — the
//! recording's origin (ADR-349) — so an action lands on the exact frame it
//! happened in.

use serde::{Deserialize, Serialize};

/// The contract name and version, written as `contract` in every document.
pub const CONTRACT: &str = "koru-session-recording/1";

/// The document. One per clip, written by the same worker that finalizes the
/// video, so the two artifacts can never disagree about existing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTelemetry {
    /// Always [`CONTRACT`]; a reader checks it before reading anything else.
    pub contract: String,
    /// The clip's file name — the document sits beside it, so the name is
    /// the whole reference.
    pub clip: String,
    /// The clip's size in physical pixels, for mapping `x,y` onto the frame.
    pub size: (u32, u32),
    /// What the person did, in time order.
    pub actions: Vec<Action>,
    /// Where the mouse was, every 500 ms, in time order — the stream a
    /// composition re-animates the cursor from.
    pub heartbeats: Vec<Heartbeat>,
}

impl SessionTelemetry {
    /// Start an empty document for `clip` at `size`. Observations are pushed
    /// by the view's recorder; this is the shape `finish` returns when
    /// nothing was observed.
    pub fn new(clip: String, size: (u32, u32)) -> Self {
        Self {
            contract: CONTRACT.to_owned(),
            clip,
            size,
            actions: Vec::new(),
            heartbeats: Vec::new(),
        }
    }

    /// The rendered document, pretty-printed for a person checking a take.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("telemetry always serializes")
    }
}

/// One observed action. `kind` is the whole of *what*; `x,y` is *where*, when
/// the action has a position at all.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Milliseconds since the encoder's first frame.
    pub at_ms: u64,
    pub kind: ActionKind,
    /// Capture-local physical pixels. Absent for actions with no position —
    /// a tab switch and a keystroke happen everywhere at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
}

/// The v1 set (SESSIONRECORD §8): what a composition can overlay, and nothing
/// it could read content out of.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// A press anywhere outside the document's content — always recorded,
    /// always with `x,y`.
    PointerClick,
    /// A press that landed in the active document's content.
    NodeClick,
    /// A menu opened.
    MenuOpen,
    /// The active tab changed.
    TabSwitch,
    /// Text was typed. The kind only — **never** the text.
    TextInput,
}

/// Where the mouse was at one 500 ms tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// Milliseconds since the encoder's first frame.
    pub at_ms: u64,
    /// Capture-local physical pixels.
    pub x: f32,
    pub y: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document's shape is the contract: a composition written against it
    /// must find these keys, these kinds, and no place content could hide.
    #[test]
    fn the_contract_round_trips_and_stays_content_free() {
        let mut doc = SessionTelemetry::new("koru-20260917-101500.mp4".into(), (1920, 1080));
        doc.actions.push(Action {
            at_ms: 1200,
            kind: ActionKind::PointerClick,
            x: Some(512.0),
            y: Some(300.0),
        });
        doc.actions.push(Action {
            at_ms: 1500,
            kind: ActionKind::TabSwitch,
            x: None,
            y: None,
        });
        doc.heartbeats.push(Heartbeat {
            at_ms: 500,
            x: 100.0,
            y: 200.0,
        });
        let json = doc.to_json();
        let back: SessionTelemetry = serde_json::from_str(&json).expect("the document parses back");
        assert_eq!(back, doc);
        assert_eq!(back.contract, CONTRACT);
        // Positionless actions write no x/y keys at all — a consumer must not
        // have to know what `null` means for this document.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tab_switch = &value["actions"][1];
        assert!(tab_switch.get("x").is_none());
        assert!(tab_switch.get("y").is_none());
        // And nothing anywhere in the document could be mistaken for content:
        // every string is the contract, the clip name, or a kind.
        let kinds: Vec<&str> = value["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["pointer_click", "tab_switch"]);
    }
}
