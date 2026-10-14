// SPDX-License-Identifier: MPL-2.0

//! What the mark samples from, which is not visible at any call site.

use super::*;

/// The caption tile is 20 px and the master is 512: without a mip, one texel in
/// twenty-five reaches the screen and the spiral draws as stairs. Pin the two
/// real destinations rather than the arithmetic — this is the test that fails
/// if the mip lookup is ever taken back out.
#[test]
fn a_small_tile_samples_a_small_source() {
    let native = 512;
    // The caption tile, at 100% and at the two common Windows scalings.
    assert_eq!(source_size(20.0, native), Some(32));
    assert_eq!(source_size(30.0, native), Some(32));
    assert_eq!(source_size(40.0, native), Some(64));
    // The welcome tile.
    assert_eq!(source_size(56.0, native), Some(64));
    // Never more than one power of two above what is asked for.
    for wanted in 1..native {
        let chosen = source_size(wanted as f64, native).unwrap_or(native);
        assert!(chosen >= wanted.max(MIN_MIP), "{wanted} sampled {chosen}");
        assert!(
            chosen <= (wanted * 2).max(MIN_MIP),
            "{wanted} sampled {chosen}"
        );
    }
}

/// Magnifying the master is the master's job — there is nothing above it to
/// resample from, and upscaling first would only throw pixels away.
#[test]
fn a_destination_past_the_master_uses_the_master() {
    assert_eq!(source_size(512.0, 512), None);
    assert_eq!(source_size(2048.0, 512), None);
    // A degenerate transform must not panic or ask for a zero-sized image.
    assert_eq!(source_size(0.0, 512), None);
    assert_eq!(source_size(f64::NAN, 512), None);
    assert_eq!(source_size(f64::INFINITY, 512), None);
}

/// The caption mark is squashed along x while the pane arrives. If the mip
/// followed that axis it would step down through the cache every frame of the
/// animation — resampling 512² repeatedly, in the middle of the one animation
/// the user is watching. It follows the larger axis, so the whole spin draws
/// from one source.
#[test]
fn the_spin_does_not_change_which_source_is_sampled() {
    let native = 512;
    let at = |x_scale: f64| {
        let t = Affine::scale(1.5) * Affine::scale_non_uniform(x_scale, 1.0);
        source_size((20.0 * device_scale(t)).ceil(), native)
    };
    let steady = at(1.0);
    for step in 0..=10 {
        assert_eq!(at(0.05 + 0.095 * f64::from(step)), steady);
    }
}

/// Device pixels, not logical ones: the same tile on a 200% display must
/// sample twice the detail, or high-DPI is where the stairs come back.
#[test]
fn the_scale_of_the_transform_is_what_counts() {
    assert_eq!(device_scale(Affine::IDENTITY), 1.0);
    assert_eq!(device_scale(Affine::scale(2.0)), 2.0);
    assert_eq!(device_scale(Affine::translate((40.0, 9.0))), 1.0);
    // Rotation is a scale of 1: the caption mark must not step up a mip
    // because it turned.
    let turned = device_scale(Affine::rotate(0.9));
    assert!((turned - 1.0).abs() < 1e-9, "{turned}");
}

/// One resample per size, not one per frame. A brush is `Arc`-backed, so the
/// cheap check is that the pixels are shared rather than rebuilt.
#[test]
fn a_size_is_only_resampled_once() {
    let first = mip(32);
    let again = mip(32);
    assert_eq!(first.image.width, 32);
    assert!(
        first.image.data.as_ref().as_ptr() == again.image.data.as_ref().as_ptr(),
        "the second ask rebuilt the image instead of using the cache"
    );
}

/// Every launch-product mark parses to a non-empty outline. This is the test
/// that makes `BrandIcon::bez`'s quiet `None` honest: a malformed path can
/// only mean an un-run test, never a shipped mark that draws nothing.
#[test]
fn every_brand_path_parses_to_an_outline() {
    for brand in ALL_BRANDS.iter().chain(SIGN_IN_MARKS.iter()) {
        let bez = brand
            .bez()
            .unwrap_or_else(|| panic!("{}'s path data does not parse", brand.name));
        assert!(!bez.elements().is_empty(), "{} parsed empty", brand.name);
        // And it is drawably sized: inside the view box, not degenerate.
        let bounds = vello::kurbo::Shape::bounding_box(bez);
        assert!(
            bounds.width() > 1.0 && bounds.height() > 1.0,
            "{}'s outline is degenerate: {bounds:?}",
            brand.name
        );
        assert!(
            bounds.min_x() >= -0.5
                && bounds.min_y() >= -0.5
                && bounds.max_x() <= VIEW_BOX + 0.5
                && bounds.max_y() <= VIEW_BOX + 0.5,
            "{}'s outline leaves the {VIEW_BOX} view box: {bounds:?}",
            brand.name
        );
    }
}

/// No two products share a mark object, and none is nameless — the same
/// two-identical-buttons failure `icons::no_two_icons_share_a_codepoint`
/// guards against, for the vector set.
#[test]
fn brand_marks_are_distinct() {
    for (i, a) in ALL_BRANDS.iter().enumerate() {
        assert!(!a.name.is_empty());
        for b in &ALL_BRANDS[i + 1..] {
            assert!(a != b, "{} and {} are one static", a.name, b.name);
            assert_ne!(a.name, b.name);
        }
    }
}

/// The wire keys of `koru_session_protocol::built_in_profile` (frozen,
/// ADR-263) each map, and to the right kind of mark. Totality against the
/// *live* catalogue is asserted in `apps/koru`'s context-menu tests, where
/// the catalogue is a dependency; these arms pin what each key means.
#[test]
fn for_profile_maps_the_frozen_wire_keys() {
    // The System row wears the OS's own mark, never a product's — and never
    // the generic terminal glyph.
    let system = for_profile("platform-shell").expect("the platform shell maps");
    assert!(matches!(
        system,
        ProfileMark::Glyph(crate::icons::WINDOWS_LOGO)
            | ProfileMark::Glyph(crate::icons::APPLE_LOGO)
            | ProfileMark::Glyph(crate::icons::LINUX_LOGO)
    ));
    assert_ne!(system, ProfileMark::Glyph(crate::icons::TERMINAL));
    assert_eq!(
        for_profile("powershell"),
        Some(ProfileMark::Brand(&POWERSHELL))
    );
    assert_eq!(for_profile("bash"), Some(ProfileMark::Brand(&BASH)));
    assert_eq!(for_profile("claude"), Some(ProfileMark::Brand(&CLAUDE)));
    assert_eq!(for_profile("opencode"), Some(ProfileMark::Brand(&OPENCODE)));
    // One product behind two launch paths wears one face.
    assert_eq!(for_profile("codex"), Some(ProfileMark::Brand(&OPENAI)));
    assert_eq!(for_profile("codex-cli"), Some(ProfileMark::Brand(&OPENAI)));
    assert_eq!(for_profile("grok"), Some(ProfileMark::Brand(&GROK)));
    assert_eq!(for_profile("kimi"), Some(ProfileMark::Brand(&KIMI)));
    // An unknown key keeps the caller's generic glyph.
    assert_eq!(for_profile("some-future-agent"), None);
    assert_eq!(for_profile(""), None);
}
