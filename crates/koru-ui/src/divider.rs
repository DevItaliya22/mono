// SPDX-License-Identifier: MPL-2.0

//! Shared divider lanes: the line owns the breathing room around itself.

use koru_draw::geometry::Rect;
use koru_draw::{scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

/// Horizontal clearance on either side of a vertical divider.
pub const MARGIN: f32 = 8.0;
/// The layout lane consumed by a divider: 8 px before and 8 px after.
pub const VERTICAL_LANE_W: f32 = MARGIN * 2.0;

/// Draw a vertical divider centred in its layout lane.
pub fn draw_vertical(canvas: &mut Scene, root: Affine, lane: Rect, inset_y: f32) {
    let x = lane.x + lane.w * 0.5;
    scene::stroke(
        canvas,
        root,
        theme::p().panel_border,
        theme::BORDER_W,
        &vello::kurbo::Line::new(
            (x as f64, (lane.y + inset_y) as f64),
            (x as f64, (lane.y + lane.h - inset_y) as f64),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_lane_owns_equal_eight_pixel_margins() {
        let lane = Rect::new(20.0, 0.0, VERTICAL_LANE_W, 30.0);
        let line_x = lane.x + lane.w * 0.5;
        assert_eq!(line_x - lane.x, MARGIN);
        assert_eq!(lane.x + lane.w - line_x, MARGIN);
    }
}
