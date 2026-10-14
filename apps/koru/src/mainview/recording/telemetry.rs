// SPDX-License-Identifier: MPL-2.0

//! The recorder's eyes: what the view notices while a capture is live, and
//! the 500 ms heartbeat — pure mapping, no I/O.
//!
//! The split is the modal's own: everything here is a value, so the suite
//! drives the whole sampler without a window. The *clock* is the encoder's
//! first frame, relayed by the shell as `RecordingEvent::Started`'s origin —
//! same process, same `Instant`, so an observation stamped `now - origin`
//! lands on the frame it happened in with no drift to correct.
//!
//! **What is deliberately not observed:** anything with content in it. The
//! recorder sees that a character was typed, never which one; that a tab
//! changed, never its name; that a menu opened, never its rows. The contract
//! (`koru_recording::telemetry`) has no field such a value could hide in.

use std::time::{Duration, Instant};

use koru_recording::telemetry::{Action, ActionKind, Heartbeat, SessionTelemetry};

/// The mouse-heartbeat cadence (§8). Coarser would make a composition's
/// re-animated cursor step; finer buys nothing a person can see.
pub(crate) const HEARTBEAT: Duration = Duration::from_millis(500);

/// One recording's observations. Created at `Started`, drained by the
/// shell's finalize worker through `View::take_recording_telemetry`.
pub(crate) struct Recorder {
    origin: Instant,
    /// The window's DPI scale at capture start, in thousandths — the second
    /// half of view-logical → capture-physical, the first being the live
    /// app zoom handed to each observation.
    dpi_scale_milli: u32,
    size: (u32, u32),
    clip_name: String,
    actions: Vec<Action>,
    heartbeats: Vec<Heartbeat>,
    last_heartbeat: Option<Instant>,
}

impl Recorder {
    pub(crate) fn new(
        origin: Instant,
        dpi_scale_milli: u32,
        size: (u32, u32),
        clip_name: String,
    ) -> Self {
        Self {
            origin,
            dpi_scale_milli,
            size,
            clip_name,
            actions: Vec::new(),
            heartbeats: Vec::new(),
            last_heartbeat: None,
        }
    }

    /// Milliseconds from the first encoded frame. Saturating: a stale event
    /// from before the origin is a zero, never a panic.
    fn at_ms(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.origin).as_millis() as u64
    }

    /// View-logical → capture-local physical pixels. The zoom is the live
    /// one — a person may zoom mid-recording and the coordinates follow what
    /// is actually on screen; the DPI scale is the start's, because the
    /// lock holds the window on one display.
    fn local(&self, x: f32, y: f32, zoom: f32) -> (f32, f32) {
        let factor = zoom * self.dpi_scale_milli as f32 / 1000.0;
        (x * factor, y * factor)
    }

    /// A press, always recorded and always with its position.
    pub(crate) fn click(&mut self, now: Instant, x: f32, y: f32, zoom: f32) {
        let (x, y) = self.local(x, y, zoom);
        self.actions.push(Action {
            at_ms: self.at_ms(now),
            kind: ActionKind::PointerClick,
            x: Some(x),
            y: Some(y),
        });
    }

    /// The press just recorded landed in the active document's content: it
    /// *is* the node click of the v1 set — one row, reclassified, never a
    /// second row for the same gesture.
    pub(crate) fn last_click_was_a_node(&mut self) {
        if let Some(last) = self.actions.last_mut()
            && last.kind == ActionKind::PointerClick
        {
            last.kind = ActionKind::NodeClick;
        }
    }

    pub(crate) fn menu_open(&mut self, now: Instant) {
        self.push_positionless(now, ActionKind::MenuOpen);
    }

    pub(crate) fn tab_switch(&mut self, now: Instant) {
        self.push_positionless(now, ActionKind::TabSwitch);
    }

    /// A character was typed. The kind arrives here; the character never
    /// does — that is the whole of the privacy rule.
    pub(crate) fn text_input(&mut self, now: Instant) {
        self.push_positionless(now, ActionKind::TextInput);
    }

    fn push_positionless(&mut self, now: Instant, kind: ActionKind) {
        self.actions.push(Action {
            at_ms: self.at_ms(now),
            kind,
            x: None,
            y: None,
        });
    }

    /// When the next heartbeat is due — `None` before the recording starts,
    /// which is what keeps an idle window asleep. The first beat is one
    /// cadence after the origin, not the origin itself.
    pub(crate) fn heartbeat_due(&self) -> Instant {
        self.last_heartbeat
            .map_or(self.origin + HEARTBEAT, |last| last + HEARTBEAT)
    }

    /// Record where the mouse is, if the cadence says so. Answers whether a
    /// beat was taken, so the caller can see the sampler work.
    pub(crate) fn heartbeat(&mut self, now: Instant, x: f32, y: f32, zoom: f32) -> bool {
        if now < self.heartbeat_due() {
            return false;
        }
        let (x, y) = self.local(x, y, zoom);
        self.heartbeats.push(Heartbeat {
            at_ms: self.at_ms(now),
            x,
            y,
        });
        self.last_heartbeat = Some(now);
        true
    }

    /// The document for the finalize worker.
    pub(crate) fn finish(self) -> SessionTelemetry {
        let mut doc = SessionTelemetry::new(self.clip_name, self.size);
        doc.actions = self.actions;
        doc.heartbeats = self.heartbeats;
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder() -> Recorder {
        // A 1080p clip on a 150 % display.
        Recorder::new(
            Instant::now(),
            1500,
            (1920, 1080),
            "koru-20260917-101500.mp4".into(),
        )
    }

    /// Stamps are milliseconds from the first encoded frame — the origin the
    /// shell relayed — never wall time.
    #[test]
    fn observations_are_stamped_from_the_first_frame_origin() {
        let origin = Instant::now();
        let mut recorder = Recorder::new(origin, 1000, (1920, 1080), "clip.mp4".into());
        recorder.menu_open(origin + Duration::from_millis(1200));
        let doc = recorder.finish();
        assert_eq!(doc.actions[0].at_ms, 1200);
    }

    /// View-logical coordinates land in the clip's own pixels through the
    /// live zoom and the start's DPI scale — a click at (100, 50) with zoom
    /// 1.25 on a 150 % display is (187.5, 93.75) in the frame.
    #[test]
    fn coordinates_are_capture_local_physical_pixels() {
        let origin = Instant::now();
        let mut recorder = Recorder::new(origin, 1500, (1920, 1080), "clip.mp4".into());
        recorder.click(origin, 100.0, 50.0, 1.25);
        let doc = recorder.finish();
        let click = &doc.actions[0];
        assert_eq!(click.kind, ActionKind::PointerClick);
        assert_eq!(click.x, Some(187.5));
        assert_eq!(click.y, Some(93.75));
    }

    /// A press that reaches the document is the same row, reclassified — the
    /// v1 set has a node click, not two rows for one gesture.
    #[test]
    fn a_document_click_is_reclassified_not_duplicated() {
        let origin = Instant::now();
        let mut recorder = Recorder::new(origin, 1000, (1920, 1080), "clip.mp4".into());
        recorder.click(origin, 10.0, 10.0, 1.0);
        recorder.last_click_was_a_node();
        let doc = recorder.finish();
        assert_eq!(doc.actions.len(), 1);
        assert_eq!(doc.actions[0].kind, ActionKind::NodeClick);
    }

    /// Beats land on the cadence and not a pass sooner; the first is one
    /// cadence after the origin.
    #[test]
    fn the_heartbeat_keeps_the_cadence() {
        let origin = Instant::now();
        let mut recorder = Recorder::new(origin, 1000, (1920, 1080), "clip.mp4".into());
        assert!(!recorder.heartbeat(origin + Duration::from_millis(200), 0.0, 0.0, 1.0));
        assert!(recorder.heartbeat(origin + Duration::from_millis(500), 10.0, 20.0, 1.0));
        assert!(!recorder.heartbeat(origin + Duration::from_millis(900), 0.0, 0.0, 1.0));
        assert!(recorder.heartbeat(origin + Duration::from_millis(1000), 30.0, 40.0, 1.0));
        let doc = recorder.finish();
        assert_eq!(doc.heartbeats.len(), 2);
        assert_eq!(doc.heartbeats[0].at_ms, 500);
        assert_eq!((doc.heartbeats[0].x, doc.heartbeats[0].y), (10.0, 20.0));
        assert_eq!(doc.heartbeats[1].at_ms, 1000);
    }

    /// The document carries the clip's name and size, so a consumer needs
    /// nothing but the file beside it.
    #[test]
    fn the_document_names_its_clip_and_size() {
        let doc = recorder().finish();
        assert_eq!(doc.clip, "koru-20260917-101500.mp4");
        assert_eq!(doc.size, (1920, 1080));
        assert_eq!(doc.contract, "koru-session-recording/1");
    }
}
