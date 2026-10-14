// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn the_mark_registers_with_its_size_and_colour_treatment() {
    let mut scene_out = Scene::new();
    let root = Affine::IDENTITY;
    let capture = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, 200.0, 200.0));
    KoruLogo::new(34.0).draw(&mut scene_out, root, 44.0, 40.0);
    let snapshot = capture.finish();
    let logo = &snapshot.records[0];
    assert_eq!(logo.name, "KoruLogo");
    assert_eq!(logo.rect(), scene::rect(44.0, 40.0, 34.0, 34.0));
    assert!(
        logo.properties
            .iter()
            .any(|p| p.name == "colour" && p.value == "brand bitmap")
    );
}

#[test]
fn a_tinted_mark_reports_the_tint_and_still_draws() {
    let mut scene_out = Scene::new();
    let root = Affine::IDENTITY;
    let capture = inspect::Frame::begin(2, root, scene::rect(0.0, 0.0, 200.0, 200.0));
    KoruLogo::new(20.0)
        .tint(Color::WHITE)
        .draw(&mut scene_out, root, 4.0, 4.0);
    let snapshot = capture.finish();
    let logo = &snapshot.records[0];
    assert_eq!(logo.name, "KoruLogo");
    assert!(
        logo.properties
            .iter()
            .any(|p| p.name == "colour" && p.value.starts_with("#FFFFFF"))
    );
}

#[test]
fn degenerate_sizes_draw_nothing_and_panic_nowhere() {
    let mut scene_out = Scene::new();
    KoruLogo::new(0.0).draw(&mut scene_out, Affine::IDENTITY, 0.0, 0.0);
    KoruLogo::new(-4.0)
        .tint(Color::BLACK)
        .draw(&mut scene_out, Affine::IDENTITY, 0.0, 0.0);
}
