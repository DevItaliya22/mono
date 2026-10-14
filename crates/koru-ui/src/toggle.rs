// SPDX-License-Identifier: MPL-2.0

//! A pill switch: one boolean, shown as a track with a knob at one end.
//!
//! **A switch, not a checkbox.** It is used where the change takes effect the
//! moment it is made and there is no form to submit — per-Profile Cloud sync is
//! the first — so the control has to read as a state rather than as a choice
//! waiting to be confirmed. The knob's *position* carries that, which is why the
//! track is wide enough for the travel to be visible at a glance.
//!
//! **Colour is never the only difference.** On and off differ by knob position
//! as well as by track fill, and the optional leading glyph differs too (the
//! caller passes `CLOUD`/`CLOUD_SLASH`), so the state survives greyscale and a
//! colour-blind reader.
//!
//! Geometry is a pure function of the rect it is given, and the caller hit-tests
//! that same rect — there is no second copy of the layout to drift.
use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Toggle {
    on: bool,
    enabled: bool,
    /// Drawn while a change is in flight: the switch still shows the state the
    /// caller believes in, dimmed, so nothing appears to have happened twice.
    pending: bool,
    icon: Option<char>,
}

impl Toggle {
    /// Height of the whole control, and of the track.
    pub const H: f32 = 18.0;
    /// Width of the track alone.
    pub const TRACK_W: f32 = 32.0;
    /// Gap between the leading glyph and the track.
    pub const ICON_GAP: f32 = 6.0;
    /// Width of the leading glyph's slot. Square, so the control's own width is
    /// the same whichever glyph a caller passes.
    pub const ICON_W: f32 = 16.0;
    const KNOB_INSET: f32 = 2.0;

    pub const fn new(on: bool) -> Self {
        Self {
            on,
            enabled: true,
            pending: false,
            icon: None,
        }
    }
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub const fn pending(mut self, pending: bool) -> Self {
        self.pending = pending;
        self
    }
    pub const fn icon(mut self, glyph: char) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// The control's full width, glyph slot included when there is one.
    pub const fn width(self) -> f32 {
        match self.icon {
            Some(_) => Self::ICON_W + Self::ICON_GAP + Self::TRACK_W,
            None => Self::TRACK_W,
        }
    }

    /// The track, inside `rect`. In an RTL row the glyph leads from the right,
    /// so the track sits at the *left* end of the control.
    pub fn track(self, rect: Rect, rtl: bool) -> Rect {
        let y = rect.y + (rect.h - Self::H) * 0.5;
        let x = match (self.icon.is_some(), rtl) {
            (true, false) => rect.x + Self::ICON_W + Self::ICON_GAP,
            _ => rect.x,
        };
        Rect::new(x, y, Self::TRACK_W, Self::H)
    }

    /// The leading glyph's square slot, or `None` when there is no glyph.
    pub fn icon_slot(self, rect: Rect, rtl: bool) -> Option<Rect> {
        self.icon?;
        let y = rect.y + (rect.h - Self::ICON_W) * 0.5;
        let x = if rtl {
            rect.x + rect.w - Self::ICON_W
        } else {
            rect.x
        };
        Some(Rect::new(x, y, Self::ICON_W, Self::ICON_W))
    }

    /// Centre and radius of the knob. "On" travels toward the reading end of
    /// the line: the right in LTR, the left in RTL.
    pub fn knob(self, rect: Rect, rtl: bool) -> (f32, f32, f32) {
        let track = self.track(rect, rtl);
        let radius = Self::H * 0.5 - Self::KNOB_INSET;
        let near = track.x + Self::KNOB_INSET + radius;
        let far = track.x + track.w - Self::KNOB_INSET - radius;
        let cx = if self.on == !rtl { far } else { near };
        (cx, track.y + track.h * 0.5, radius)
    }

    #[allow(clippy::too_many_arguments)]
    #[track_caller]
    pub fn draw(
        self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        rect: Rect,
        rtl: bool,
        hovered: bool,
        focused: bool,
    ) {
        let _component = koru_draw::inspect::scope(
            "KoruToggle",
            "component",
            root,
            scene::rect(rect.x, rect.y, rect.w, rect.h),
            |r| {
                r.property(
                    "implementation",
                    "koru_ui::toggle::Toggle",
                    "shared implementation",
                );
                r.property(
                    "on / enabled / pending",
                    format!("{} / {} / {}", self.on, self.enabled, self.pending),
                    "instance state",
                );
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        let p = theme::p();
        let track = self.track(rect, rtl);
        let shape = scene::rrect(track.x, track.y, track.w, track.h, track.h * 0.5);
        let live = self.enabled && !self.pending;
        scene::fill(
            canvas,
            root,
            if self.on && live {
                p.accent
            } else if self.on {
                p.row_selected
            } else {
                p.chip_bg
            },
            &shape,
        );
        if hovered && live {
            scene::fill(canvas, root, p.hover_tint, &shape);
        }
        scene::stroke(
            canvas,
            root,
            if focused {
                p.focus_ring
            } else {
                p.panel_border
            },
            if focused {
                theme::BORDER_W * 2.0
            } else {
                theme::BORDER_W
            },
            &shape,
        );
        let (cx, cy, radius) = self.knob(rect, rtl);
        scene::fill(
            canvas,
            root,
            if self.on && live {
                p.on_accent
            } else if live {
                p.text
            } else {
                p.text_dim
            },
            &scene::circle(cx, cy, radius),
        );
        if let (Some(glyph), Some(slot)) = (self.icon, self.icon_slot(rect, rtl)) {
            let layout = engine.icon_layout(glyph, Self::ICON_W);
            scene::draw_layout_first_line(
                canvas,
                root,
                &layout,
                slot.x + (slot.w - layout.width()) * 0.5,
                slot.y,
                Some(if live { p.text_dim } else { p.placeholder }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::new(100.0, 40.0, Toggle::new(false).icon('x').width(), 58.0)
    }

    #[test]
    fn the_glyph_leads_and_the_track_follows_in_a_left_to_right_row() {
        let toggle = Toggle::new(true).icon('x');
        let r = rect();
        let slot = toggle.icon_slot(r, false).unwrap();
        let track = toggle.track(r, false);
        assert_eq!(slot.x, r.x);
        assert!(slot.x + slot.w <= track.x, "the glyph must not overlap");
        assert_eq!(track.x + track.w, r.x + r.w);
    }

    #[test]
    fn a_right_to_left_row_mirrors_both_the_glyph_and_the_knobs_travel() {
        let on = Toggle::new(true).icon('x');
        let off = Toggle::new(false).icon('x');
        let r = rect();
        let slot = on.icon_slot(r, true).unwrap();
        let track = on.track(r, true);
        assert_eq!(slot.x + slot.w, r.x + r.w, "the glyph leads from the right");
        assert_eq!(track.x, r.x);
        // "On" travels toward the reading end, which is the left in RTL.
        assert!(on.knob(r, true).0 < off.knob(r, true).0);
        assert!(on.knob(r, false).0 > off.knob(r, false).0);
    }

    #[test]
    fn the_knob_stays_inside_the_track_at_both_ends() {
        for rtl in [false, true] {
            for on in [false, true] {
                let toggle = Toggle::new(on).icon('x');
                let r = rect();
                let track = toggle.track(r, rtl);
                let (cx, cy, radius) = toggle.knob(r, rtl);
                assert!(
                    cx - radius >= track.x - 0.01,
                    "knob left edge {cx} {rtl} {on}"
                );
                assert!(cx + radius <= track.x + track.w + 0.01);
                assert!((cy - (track.y + track.h * 0.5)).abs() < 0.01);
                assert!(radius > 0.0);
            }
        }
    }

    #[test]
    fn the_width_a_caller_reserves_is_the_width_the_parts_occupy() {
        assert_eq!(Toggle::new(false).width(), Toggle::TRACK_W);
        let with_icon = Toggle::new(false).icon('x');
        assert_eq!(
            with_icon.width(),
            Toggle::ICON_W + Toggle::ICON_GAP + Toggle::TRACK_W
        );
        assert!(Toggle::new(false).icon_slot(rect(), false).is_none());
    }

    #[test]
    fn state_is_carried_by_position_as_well_as_by_colour() {
        // The regression this guards: a switch whose only difference between on
        // and off is the track fill is invisible in greyscale.
        let r = rect();
        let on = Toggle::new(true).icon('x').knob(r, false).0;
        let off = Toggle::new(false).icon('x').knob(r, false).0;
        assert!((on - off).abs() > Toggle::H * 0.5);
        // Disabled and pending change how it is painted, never where the knob is.
        let base = Toggle::new(true).icon('x');
        assert_eq!(base.enabled(false).knob(r, false).0, on);
        assert_eq!(base.pending(true).knob(r, false).0, on);
    }
}
