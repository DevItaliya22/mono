// SPDX-License-Identifier: MPL-2.0

use super::*;

/// A small deterministic generator: the property tests below want many
/// cameras, not a dependency, and a failure has to reproduce.
pub(crate) struct Lcg(u64);

impl Lcg {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// A value in `[lo, hi)`.
    pub(crate) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let unit = (self.0 >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * unit
    }
}

fn random_camera(rng: &mut Lcg) -> Camera {
    Camera::new(
        rng.range(-1_000_000.0, 1_000_000.0),
        rng.range(-1_000_000.0, 1_000_000.0),
        rng.range(Camera::MIN_ZOOM, Camera::MAX_ZOOM),
    )
}

fn close(a: (f64, f64), b: (f64, f64), tolerance: f64) -> bool {
    (a.0 - b.0).abs() <= tolerance && (a.1 - b.1).abs() <= tolerance
}

#[test]
fn canvas_to_view_to_canvas_round_trips_over_many_cameras() {
    let mut rng = Lcg::new(7);
    for _ in 0..2_000 {
        let camera = random_camera(&mut rng);
        let viewport = (
            rng.range(200.0, 3000.0) as f32,
            rng.range(200.0, 2000.0) as f32,
        );
        let point = (
            camera.center_x + rng.range(-5_000.0, 5_000.0),
            camera.center_y + rng.range(-5_000.0, 5_000.0),
        );
        let back = camera.to_canvas(viewport, camera.to_view(viewport, point));
        assert!(
            close(back, point, 1e-6),
            "{point:?} came back as {back:?} through {camera:?}"
        );
        let view = (rng.range(0.0, 3000.0), rng.range(0.0, 2000.0));
        let again = camera.to_view(viewport, camera.to_canvas(viewport, view));
        assert!(close(again, view, 1e-6), "{view:?} came back as {again:?}");
    }
}

/// The transform a thing is drawn with and the mapping it is hit-tested with
/// are one arithmetic: a point drawn at the object's local `(lx, ly)` lands
/// where `to_view` says the canvas point is.
#[test]
fn the_draw_transform_agrees_with_to_view() {
    let mut rng = Lcg::new(11);
    for _ in 0..500 {
        let camera = random_camera(&mut rng);
        let viewport = (1280.0, 800.0);
        let (x, y) = (
            (camera.center_x + rng.range(-900.0, 900.0)).round(),
            (camera.center_y + rng.range(-900.0, 900.0)).round(),
        );
        let (lx, ly) = (rng.range(0.0, 400.0), rng.range(0.0, 400.0));
        let drawn = camera.object_transform(viewport, x, y) * vello::kurbo::Point::new(lx, ly);
        let mapped = camera.to_view(viewport, (x + lx, y + ly));
        assert!(close((drawn.x, drawn.y), mapped, 1e-6));
    }
}

#[test]
fn zooming_keeps_the_point_under_the_pointer_still() {
    let mut rng = Lcg::new(3);
    let viewport = (1200.0, 900.0);
    for _ in 0..1_000 {
        let mut camera = random_camera(&mut rng);
        let pointer = (rng.range(0.0, 1200.0), rng.range(0.0, 900.0));
        let under = camera.to_canvas(viewport, pointer);
        let factor = rng.range(0.2, 5.0);
        camera.zoom_at(under, factor);
        let after = camera.to_view(viewport, under);
        assert!(
            close(after, pointer, 1e-6),
            "the point moved from {pointer:?} to {after:?}"
        );
    }
}

#[test]
fn zoom_is_held_between_ten_and_four_hundred_percent() {
    let mut camera = Camera::default();
    camera.zoom_at((0.0, 0.0), 1_000.0);
    assert_eq!(camera.zoom, Camera::MAX_ZOOM);
    camera.zoom_at((0.0, 0.0), 1e-6);
    assert_eq!(camera.zoom, Camera::MIN_ZOOM);
    // A factor that is not a number changes nothing.
    camera.zoom_at((0.0, 0.0), f64::NAN);
    camera.zoom_at((0.0, 0.0), -2.0);
    assert_eq!(camera.zoom, Camera::MIN_ZOOM);
}

#[test]
fn the_centre_is_clamped_and_nonsense_falls_back() {
    let far = Camera::new(1e12, -1e12, 1.0);
    assert_eq!(far.center_x, Camera::LIMIT);
    assert_eq!(far.center_y, -Camera::LIMIT);
    let broken = Camera::new(f64::NAN, f64::INFINITY, f64::NAN);
    assert_eq!(broken, Camera::default());
}

#[test]
fn panning_moves_the_content_with_the_pointer() {
    let viewport = (800.0, 600.0);
    let mut camera = Camera::new(100.0, 50.0, 2.0);
    let before = camera.to_view(viewport, (120.0, 70.0));
    camera.pan_view(30.0, -12.0);
    let after = camera.to_view(viewport, (120.0, 70.0));
    assert!(close(after, (before.0 + 30.0, before.1 - 12.0), 1e-9));
}

#[test]
fn fit_shows_the_whole_rect_and_never_zooms_past_one_hundred_percent() {
    let viewport = (1000.0, 700.0);
    let big = Rect::new(-4_000.0, -2_000.0, 4_000.0, 3_000.0);
    let camera = Camera::fit(big, viewport);
    let seen = camera.visible(viewport);
    assert!(seen.x0 <= big.x0 && seen.y0 <= big.y0);
    assert!(seen.x1 >= big.x1 && seen.y1 >= big.y1);
    let small = Rect::new(10.0, 10.0, 60.0, 40.0);
    assert_eq!(Camera::fit(small, viewport).zoom, 1.0);
}
