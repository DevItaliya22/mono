// SPDX-License-Identifier: MPL-2.0

//! **Where the person is looking.**
//!
//! A camera is a canvas point at the middle of the view and a zoom. Every
//! mapping between the canvas and the view is here and nowhere else, so
//! drawing, hit testing and every gesture agree about where a thing is:
//!
//! ```text
//! view   = (canvas - center) * zoom + viewport / 2
//! canvas = (view - viewport / 2) / zoom + center
//! ```
//!
//! **The view is in logical pixels, local to the pane.** The host has already
//! divided by the DPI and the app's display zoom and subtracted the pane's
//! origin, and it multiplies them back in through the `root` transform it
//! draws with — so the canvas's own zoom composes with the app's without
//! either knowing about the other.
//!
//! Centred rather than anchored at the top-left, so a window that is resized
//! keeps what was in the middle of it in the middle.

use vello::kurbo::{Affine, Rect};

#[cfg(test)]
mod tests;

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Camera {
    /// The canvas point at the middle of the view.
    pub center_x: f64,
    pub center_y: f64,
    /// 1.0 is 100 %: one canvas pixel is one logical pixel.
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_y: 0.0,
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// 10 %: a canvas can be seen whole.
    pub const MIN_ZOOM: f64 = 0.1;
    /// 400 %: a canvas pixel is four screen pixels.
    pub const MAX_ZOOM: f64 = 4.0;
    /// How far the centre may go. Objects are clamped to ±1,000,000 when they
    /// are written; the camera may look a little past the farthest of them.
    pub const LIMIT: f64 = 2_000_000.0;

    pub fn new(center_x: f64, center_y: f64, zoom: f64) -> Self {
        Self {
            center_x,
            center_y,
            zoom,
        }
        .clamped()
    }

    /// The same camera inside its limits. A value that is not a number — a
    /// corrupted setting, a zero-sized fit — falls back to the default for
    /// that field rather than poisoning every mapping after it.
    pub fn clamped(self) -> Self {
        let finite = |v: f64, fallback: f64| if v.is_finite() { v } else { fallback };
        Self {
            center_x: finite(self.center_x, 0.0).clamp(-Self::LIMIT, Self::LIMIT),
            center_y: finite(self.center_y, 0.0).clamp(-Self::LIMIT, Self::LIMIT),
            zoom: finite(self.zoom, 1.0).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM),
        }
    }

    /// A canvas point, in view coordinates.
    pub fn to_view(&self, viewport: (f32, f32), canvas: (f64, f64)) -> (f64, f64) {
        (
            (canvas.0 - self.center_x) * self.zoom + f64::from(viewport.0) * 0.5,
            (canvas.1 - self.center_y) * self.zoom + f64::from(viewport.1) * 0.5,
        )
    }

    /// A view point, in canvas coordinates.
    pub fn to_canvas(&self, viewport: (f32, f32), view: (f64, f64)) -> (f64, f64) {
        (
            (view.0 - f64::from(viewport.0) * 0.5) / self.zoom + self.center_x,
            (view.1 - f64::from(viewport.1) * 0.5) / self.zoom + self.center_y,
        )
    }

    /// A canvas rectangle, in view coordinates.
    pub fn rect_to_view(&self, viewport: (f32, f32), rect: Rect) -> Rect {
        let (x0, y0) = self.to_view(viewport, (rect.x0, rect.y0));
        let (x1, y1) = self.to_view(viewport, (rect.x1, rect.y1));
        Rect::new(x0, y0, x1, y1)
    }

    /// The part of the canvas the view shows.
    pub fn visible(&self, viewport: (f32, f32)) -> Rect {
        let (x0, y0) = self.to_canvas(viewport, (0.0, 0.0));
        let (x1, y1) = self.to_canvas(viewport, (f64::from(viewport.0), f64::from(viewport.1)));
        Rect::new(x0, y0, x1, y1)
    }

    /// The transform that draws something whose own origin is the canvas
    /// point `(x, y)`.
    ///
    /// **Per object, not one transform for the canvas.** An object far from
    /// the origin — a million pixels out — would otherwise be encoded at
    /// coordinates `f32` can only place to a sixteenth of a pixel. The
    /// difference to the centre is taken here in `f64`, so what reaches the
    /// GPU is always a small number.
    pub fn object_transform(&self, viewport: (f32, f32), x: f64, y: f64) -> Affine {
        let (vx, vy) = self.to_view(viewport, (x, y));
        Affine::translate((vx, vy)) * Affine::scale(self.zoom)
    }

    /// Move what is on screen by `(dx, dy)` view pixels — a drag or a wheel.
    pub fn pan_view(&mut self, dx: f64, dy: f64) {
        self.center_x -= dx / self.zoom;
        self.center_y -= dy / self.zoom;
        *self = self.clamped();
    }

    /// Zoom by `factor` keeping the canvas point `point` where it is on
    /// screen. The zoom is clamped first, so the point stays put even when
    /// the factor asked for more than the limits allow.
    pub fn zoom_at(&mut self, point: (f64, f64), factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let ratio = self.zoom / zoom;
        self.center_x = point.0 - (point.0 - self.center_x) * ratio;
        self.center_y = point.1 - (point.1 - self.center_y) * ratio;
        self.zoom = zoom;
        *self = self.clamped();
    }

    /// The camera that shows all of `rect` in `viewport` with a margin, never
    /// closer than 100 %: fitting a single small card should not blow it up
    /// to four times its size.
    pub fn fit(rect: Rect, viewport: (f32, f32)) -> Self {
        const MARGIN: f64 = 48.0;
        let (vw, vh) = (f64::from(viewport.0), f64::from(viewport.1));
        let room_w = (vw - MARGIN * 2.0).max(1.0);
        let room_h = (vh - MARGIN * 2.0).max(1.0);
        let zoom = (room_w / rect.width().max(1.0))
            .min(room_h / rect.height().max(1.0))
            .min(1.0);
        let center = rect.center();
        Self::new(center.x, center.y, zoom)
    }
}
