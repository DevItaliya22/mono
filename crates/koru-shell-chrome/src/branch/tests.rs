// SPDX-License-Identifier: MPL-2.0

use super::*;

// **The three tests about which branches are worth naming are not here.** They
// moved with the policy to `apps/koru/src/build_stamp/tests.rs`; a crate that
// may not know `dev` is unremarkable may not assert it either.

/// Right-aligned in its band, like the status chip it sits beside — a pill
/// placed from the left would drift as the tabs grew.
#[test]
fn the_pill_sits_at_the_right_hand_end_of_its_band() {
    let band = Rect::new(100.0, 0.0, 400.0, theme::CAPTION_H);
    let r = pill(band, 80.0);
    assert_eq!(r.x + r.w, band.x + band.w);
    assert_eq!(r.w, 80.0);
    assert!(r.y > band.y && r.y + r.h < band.y + band.h);
}

/// A band narrower than the pill clamps rather than overhanging to the left of
/// it, which is what a very narrow window does to the strip.
#[test]
fn a_band_narrower_than_the_pill_clamps_it() {
    let band = Rect::new(0.0, 0.0, 40.0, theme::CAPTION_H);
    let r = pill(band, 120.0);
    assert_eq!(r.w, 40.0);
    assert_eq!(r.x, 0.0);
}

#[test]
fn branch_painter_registers_the_read_only_koru_chip() {
    let mut canvas = Scene::new();
    let mut engine = TextEngine::new();
    let rect = Rect::new(20.0, 10.0, 90.0, 20.0);
    let capture =
        koru_draw::inspect::Frame::begin(1, Affine::IDENTITY, scene::rect(0.0, 0.0, 200.0, 80.0));

    draw(
        &mut canvas,
        Affine::IDENTITY,
        &mut engine,
        rect,
        "refactor-ui",
    );

    let snapshot = capture.finish();
    let chip = snapshot
        .records
        .iter()
        .find(|record| record.name == "KoruBranchChip")
        .expect("shared branch painter owns an official component scope");
    assert_eq!(chip.kind, "component");
    assert_eq!(chip.rect(), scene::rect(rect.x, rect.y, rect.w, rect.h));
    assert_eq!(
        chip.properties
            .iter()
            .find(|property| property.name == "interaction")
            .unwrap()
            .value,
        "read-only / no hit target"
    );
}
