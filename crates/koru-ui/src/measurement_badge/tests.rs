// SPDX-License-Identifier: MPL-2.0

use super::*;

fn legacy(
    engine: &mut TextEngine,
    canvas: &mut Scene,
    root: Affine,
    value: &str,
    scale: f32,
    x: f32,
    y: f32,
) -> KoruMeasurementGeometry {
    let hint = theme::role::hint();
    let label = engine.simple_layout_in(
        value,
        theme::TextStyle::new(
            theme::TextMetrics::new(hint.metrics.size * scale, hint.metrics.line_height * scale),
            hint.weight,
        ),
        theme::code_stack(),
    );
    let width = label.width() + 14.0;
    let height = label.height() + 14.0;
    scene::floating_panel(
        canvas,
        root,
        x,
        y,
        width,
        height,
        &scene::Panel::tooltip(6.0),
    );
    scene::draw_layout(
        canvas,
        root,
        &label,
        x + 7.0,
        y + 7.0,
        Some(theme::p().bubble_fg),
    );
    KoruMeasurementGeometry {
        badge: Rect::new(x, y, width, height),
        text: Rect::new(x + 7.0, y + 7.0, label.width(), label.height()),
    }
}

#[test]
fn legacy_recipe_and_shared_measurement_have_identical_geometry_paint_and_private_captures() {
    let mut engine = TextEngine::new();
    for text in [
        "232 px",
        "480 × 360",
        "١٢٣ × ٤٥٦",
        "عرض ١٢٣ px",
        "private synthetic",
    ] {
        for scale in [1.0, 1.5, 2.0] {
            for zoom in [1.0, 2.0] {
                let root = Affine::scale(zoom);
                let mut before = Scene::new();
                let capture = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, 900.0, 650.0));
                let old = legacy(&mut engine, &mut before, root, text, scale, 50.0, 40.0);
                assert!(
                    !capture
                        .finish()
                        .records
                        .iter()
                        .any(|r| r.name == "KoruMeasurementBadge")
                );
                let badge = KoruMeasurementBadge::measure(&mut engine, text, scale);
                assert_eq!(badge.geometry(50.0, 40.0), old);
                for enabled in [false, true] {
                    let mut after = Scene::new();
                    let capture = enabled.then(|| {
                        inspect::Frame::begin(2, root, scene::rect(0.0, 0.0, 900.0, 650.0))
                    });
                    assert_eq!(badge.draw(&mut after, root, 50.0, 40.0), old);
                    if let Some(capture) = capture {
                        let snapshot = capture.finish();
                        assert_eq!(snapshot.records[0].name, "KoruMeasurementBadge");
                        assert_eq!(
                            snapshot.records[0].rect(),
                            scene::rect(old.badge.x, old.badge.y, old.badge.w, old.badge.h)
                        );
                        let label = snapshot
                            .records
                            .iter()
                            .find(|r| r.name == "Measurement text")
                            .unwrap();
                        assert_eq!(
                            label.rect(),
                            scene::rect(old.text.x, old.text.y, old.text.w, old.text.h)
                        );
                        assert!(!snapshot.export(None).unwrap().contains("private synthetic"));
                    }
                    let (a, b) = (before.encoding(), after.encoding());
                    assert!(a.path_tags == b.path_tags);
                    assert_eq!(a.path_data, b.path_data);
                    assert!(a.draw_tags == b.draw_tags);
                    assert_eq!(a.draw_data, b.draw_data);
                    assert_eq!(a.transforms, b.transforms);
                    assert_eq!(a.styles, b.styles);
                }
            }
        }
    }
}

#[test]
fn text_scale_grows_measured_bounds_without_scaling_padding_or_reflowing_on_draw() {
    let mut engine = TextEngine::new();
    let small = KoruMeasurementBadge::measure(&mut engine, "123 × 456", 1.0);
    let large = KoruMeasurementBadge::measure(&mut engine, "123 × 456", 2.0);
    assert!(large.size().0 > small.size().0);
    assert!(large.size().1 > small.size().1);
    for badge in [small, large] {
        let geometry = badge.geometry(30.0, 40.0);
        assert_eq!(geometry.text.x - geometry.badge.x, 7.0);
        assert_eq!(geometry.text.y - geometry.badge.y, 7.0);
        assert!((geometry.badge.w - geometry.text.w - 14.0).abs() < 0.001);
        for x in [10.0, 50.0, 250.0] {
            let drawn = badge.draw(&mut Scene::new(), Affine::IDENTITY, x, 40.0);
            assert_eq!((drawn.badge.w, drawn.badge.h), badge.size());
        }
    }
}

#[test]
fn equal_length_digits_keep_the_current_code_fonts_measured_width() {
    let mut engine = TextEngine::new();
    let a = KoruMeasurementBadge::measure(&mut engine, "111 px", 1.0);
    let b = KoruMeasurementBadge::measure(&mut engine, "888 px", 1.0);
    assert_eq!(
        a.size(),
        b.size(),
        "current code-stack digits are fixed width; no new font policy"
    );
}
