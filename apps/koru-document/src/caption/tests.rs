// SPDX-License-Identifier: MPL-2.0

use super::*;

const W: f32 = 940.0;

/// The round-trip that matters, the same one the toolbar and the date grid
/// have: every button hit-tests back to itself, corners included.
#[test]
fn every_window_button_hit_tests_back_to_its_own_command() {
    for control in controls::controls(strip(W)) {
        let expected = Some(Hit::Control(control.action));
        let r = control.rect;
        assert_eq!(hit(r.x + r.w * 0.5, r.y + r.h * 0.5, W), expected);
        assert_eq!(hit(r.x + 0.5, r.y + 0.5, W), expected);
        assert_eq!(hit(r.x + r.w - 0.5, r.y + r.h - 0.5, W), expected);
    }
}

/// Close is in the corner. The pointer has to be able to be thrown at the
/// top-right of the screen without aiming, which is the whole reason a
/// maximised window's close button works at all.
///
/// It used to depend on an experimental flag: with the ⋮ button on, *that*
/// inherited the spot. The flag is gone and the caption is four stable
/// controls, so close owns the corner unconditionally
/// ([ADR-150](../../../../docs/decisions/README.md)).
#[test]
fn the_rightmost_button_owns_the_top_right_corner() {
    assert_eq!(hit(W - 0.5, 0.5, W), Some(Hit::Control(Action::Close)));
}

/// The rest of the row is draggable rather than inert: without this the window
/// could be closed but not moved.
#[test]
fn the_bare_caption_is_something_you_can_drag() {
    assert_eq!(
        hit(theme::MARGIN_X, theme::CAPTION_H * 0.5, W),
        Some(Hit::Caption)
    );
}

/// And it stops at its own bottom edge — one pixel lower is the formatting
/// toolbar, whose buttons would otherwise be shadowed by a drag.
#[test]
fn nothing_below_the_strip_belongs_to_the_caption() {
    assert_eq!(hit(theme::MARGIN_X, theme::CAPTION_H, W), None);
    assert_eq!(hit(theme::MARGIN_X, theme::CAPTION_H + 8.0, W), None);
}

/// A window narrower than the four buttons still resolves rather than
/// producing a negative rect — the buttons crowd, and close keeps the corner.
#[test]
fn a_very_narrow_window_still_hit_tests() {
    let narrow = theme::CAPTION_BTN;
    assert!(hit(narrow - 0.5, 0.5, narrow).is_some());
}
