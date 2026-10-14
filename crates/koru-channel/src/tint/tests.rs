// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The brand teal, roughly — what most of the spiral's pixels look like.
const TEAL: [u8; 4] = [0x3E, 0xC8, 0xB4, 0xFF];

#[test]
fn stable_is_a_no_op() {
    let mut px = TEAL;
    apply(&mut px, tint_of(ReleaseChannel::Stable, false));
    assert_eq!(px, TEAL);
}

#[test]
fn every_tint_preserves_alpha() {
    for channel in ReleaseChannel::ALL {
        for debug in [false, true] {
            let mut px = [0x3E, 0xC8, 0xB4, 0x7B];
            apply(&mut px, tint_of(channel, debug));
            assert_eq!(px[3], 0x7B, "{channel:?} debug={debug} moved alpha");
        }
    }
}

#[test]
fn greyscale_means_equal_channels() {
    let mut px = TEAL;
    apply(&mut px, tint_of(ReleaseChannel::Internal, true));
    assert_eq!(px[0], px[1]);
    assert_eq!(px[1], px[2]);
}

/// The hue lands where it was set — the beta pixel is blue-dominant and the
/// nightly one red-dominant — while lightness survives, which is the whole
/// "recolor, don't repaint" contract.
#[test]
fn hue_is_set_and_lightness_kept() {
    let mut beta = TEAL;
    apply(&mut beta, tint_of(ReleaseChannel::Beta, false));
    assert!(
        beta[2] > beta[0] && beta[2] > beta[1],
        "beta is blue: {beta:?}"
    );

    let mut nightly = TEAL;
    apply(&mut nightly, tint_of(ReleaseChannel::Nightly, false));
    assert!(
        nightly[0] > nightly[2],
        "nightly is warm (red over blue): {nightly:?}"
    );

    let lum = |px: [u8; 4]| px[0] as i32 + px[1] as i32 + px[2] as i32;
    let original = lum(TEAL);
    for tinted in [lum(beta), lum(nightly)] {
        assert!(
            (tinted - original).abs() < original / 3,
            "lightness moved too far: {original} -> {tinted}"
        );
    }
}

/// Black, white and transparent pixels — the edges and the empty corners of
/// the icon — come through every tint unchanged.
#[test]
fn neutrals_are_untouched() {
    for original in [[0, 0, 0, 255], [255, 255, 255, 255], [0, 0, 0, 0]] {
        for channel in ReleaseChannel::ALL {
            for debug in [false, true] {
                let mut px = original;
                apply(&mut px, tint_of(channel, debug));
                assert_eq!(px, original, "{channel:?} debug={debug}");
            }
        }
    }
}
