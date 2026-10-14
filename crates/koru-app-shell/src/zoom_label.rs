// SPDX-License-Identifier: MPL-2.0

//! **The transient `120%` indicator**, as the shell's own.
//!
//! Koru has flashed the percentage centred on the window since ADR-271 —
//! `apps/koru/src/mainview/zoomlabel.rs` and `draw_zoom_label` in its
//! `draw.rs`. The owner asked for the same thing in the terminal (2026-09-02),
//! and the honest way to give it to one more host was to move the behaviour to
//! the shell that already owns display zoom rather than to copy 40 lines into a
//! second app. Every host the shell seats gets it, for nothing.
//!
//! **What is mirrored exactly**: the copy (`{n}%`, integer, no decimals), the
//! type role, the 9 px pad, the 6 px popover pill with its border and its
//! shadow, the centring on the whole window, the fixed physical size at every
//! zoom, and the timing — 0.7 s at full opacity, then a 0.5 s linear ramp to
//! nothing, 1.2 s in all. A person who zooms Koru and then zooms the terminal
//! should not be able to tell which window they are in.
//!
//! **What is deliberately not mirrored**: Koru arms this by edge-detecting the
//! `theme::zoom()` process global, because zoom is a shell-level transform its
//! view never sees. The shell *is* that level — [`ShellState::set_zoom`] already
//! answers whether anything moved — so the arming is a call at the one place the
//! value changes, and the terminal's zoom (which never touches `theme::set_zoom`
//! at all) is armed by the same line as Koru's would be.
//!
//! **And the frame rule is the toast's, not Koru's.** `ZoomLabel::animating`
//! there is `shown.is_some()`, so it asks for 8 ms frames through the hold as
//! well as the fade — about 87 frames to hold a pill still, which
//! [techdebt 9](../../../docs/contributing/techdebt/README.md) already names as the thing the
//! toast's rule is worth generalising over. Here, [`Self::animating`] is true
//! **only while the pill is actually fading**, and the end of the hold is a
//! [`maintenance_deadline`](Self::maintenance_deadline) — a wake, not a spin.
//! Holding a message still is not an animation; `koru_ui::toast` said it first.

use std::time::{Duration, Instant};

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;

/// How long the indicator stays at full opacity before it begins to fade.
/// Koru's `HOLD_SECS`.
const HOLD: Duration = Duration::from_millis(700);
/// How long the ramp to nothing takes. Koru's `TOTAL_SECS - HOLD_SECS`.
const FADE: Duration = Duration::from_millis(500);

/// The pill's inset on all four sides. Koru's `pad`.
const PAD: f32 = 9.0;
/// Its corner radius. Koru's `Panel::popover(6.0)`.
const RADIUS: f32 = 6.0;

/// The flash, as a value.
///
/// One `Option<Instant>` and nothing else: the deadline is derived from it at
/// every read rather than stored beside it, so the two cannot disagree, and a
/// re-arm is an assignment rather than a reset of three fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoomLabel {
    /// When the current flash began. `None` at rest — which is what
    /// [`animating`](Self::animating) and [`showing`](Self::showing) report.
    shown: Option<Instant>,
}

impl ZoomLabel {
    pub const fn new() -> Self {
        Self { shown: None }
    }

    /// (Re)arm. **Each change resets the deadline**, so holding Ctrl+= keeps the
    /// indicator up rather than letting an earlier flash expire underneath the
    /// steps that followed it.
    pub const fn show(&mut self, now: Instant) {
        self.shown = Some(now);
    }

    /// Whether there is a pill to draw at all. Cheap, and `alpha` is the one
    /// that says how faded it is.
    pub const fn showing(&self) -> bool {
        self.shown.is_some()
    }

    /// The alpha to draw at, or `None` once the flash is over.
    ///
    /// **Elapsed time against a fixed duration, never a per-frame decrement**,
    /// so a dropped frame cannot leave the pill stuck half-faded on screen
    /// (ADR-26). The ramp is linear, exactly as Koru's is.
    pub fn alpha(&self, now: Instant) -> Option<f32> {
        let lived = now.saturating_duration_since(self.shown?);
        if lived < HOLD {
            return Some(1.0);
        }
        let fading = (lived - HOLD).as_secs_f32() / FADE.as_secs_f32();
        (fading < 1.0).then(|| (1.0 - fading).clamp(0.0, 1.0))
    }

    /// **Only while it is fading.** See the module note: the hold is a wake, not
    /// 87 frames of drawing the same pill in the same place.
    pub fn animating(&self, now: Instant) -> bool {
        self.shown.is_some_and(|shown| {
            now.saturating_duration_since(shown) >= HOLD && self.alpha(now).is_some()
        })
    }

    /// When the shell next has to look at this window: the moment the hold ends
    /// and the fade wants frames. `None` at rest, and `None` once the fade has
    /// started — [`animating`](Self::animating) is what asks for frames then.
    pub fn maintenance_deadline(&self) -> Option<Instant> {
        self.shown.map(|shown| shown + HOLD)
    }

    /// Drop an expired flash, so [`showing`](Self::showing) reaches `false` and
    /// the window stops being redrawn. Answers whether anything changed, which
    /// is what the shell's `maintain` accumulates.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.shown.is_none() || self.alpha(now).is_some() {
            return false;
        }
        self.shown = None;
        true
    }

    /// The pill's rect in **window** space — the space `draw` paints in, which
    /// is the unzoomed one.
    ///
    /// Split out from [`draw`](Self::draw) so a test can say where the pill
    /// lands without a font: everything here but the label's own measurement is
    /// arithmetic, and `Menu::at_width` takes the same position for the same
    /// reason.
    pub fn pill(label: (f32, f32), viewport: Rect, zoom: f32) -> Rect {
        let w = label.0 + PAD * 2.0;
        let h = label.1 + PAD * 2.0;
        // `viewport` is the window's zoomed-*down* layout space; multiplying by
        // the zoom recovers true window pixels, which is the space the undone
        // transform below draws in.
        Rect::new(
            (viewport.w * zoom - w) * 0.5,
            (viewport.h * zoom - h) * 0.5,
            w,
            h,
        )
    }

    /// The percentage a zoom factor reads as — [`theme::zoom_percent`], which
    /// is also what Koru's own flash and the window menu's readout show.
    pub fn text(zoom: f32) -> String {
        theme::zoom_percent(zoom)
    }

    /// Paint it, centred on the window and **at a fixed physical size**.
    ///
    /// The transform undoes the shell's zoom, or the indicator would swell at
    /// 200% and all but vanish at 50% — it is a readout *about* the zoom, so it
    /// is the one piece of chrome the zoom must not touch.
    pub fn draw(
        &self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        viewport: Rect,
        zoom: f32,
        now: Instant,
    ) {
        let Some(alpha) = self.alpha(now) else {
            return;
        };
        // Guarded rather than a trusting divide, for `ShellWindow::divisor`'s
        // reason: a zero would put the pill at infinity.
        let divisor = if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        };
        let unzoomed = root * Affine::scale(f64::from(1.0 / divisor));
        let label = engine.simple_layout(&Self::text(zoom), theme::role::row());
        let pill = Self::pill((label.width(), label.height()), viewport, divisor);
        // `faded` carries the shadow with it. Fading the panel while its shadow
        // stayed put left the shadow as the last thing on screen, cast by
        // nothing — Koru's note, and the reason `Panel::alpha` exists.
        scene::floating_panel(
            scene,
            unzoomed,
            pill.x,
            pill.y,
            pill.w,
            pill.h,
            &scene::Panel::popover(RADIUS).faded(alpha),
        );
        scene::draw_layout(
            scene,
            unzoomed,
            &label,
            pill.x + PAD,
            pill.y + PAD,
            Some(fade(theme::p().text, alpha)),
        );
    }
}

/// Scale a colour's alpha, so the text fades with the pill under it.
fn fade(mut color: Color, alpha: f32) -> Color {
    color.components[3] *= alpha;
    color
}

#[cfg(test)]
mod tests;
