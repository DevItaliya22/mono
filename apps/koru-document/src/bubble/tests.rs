// SPDX-License-Identifier: MPL-2.0

//! The bubble's geometry, tested without a viewport or a document.
//!
//! `buttons` deliberately needs no font context, which is what makes this
//! possible — and what makes the renderer and the click path share one
//! arithmetic rather than agreeing by inspection.

use super::*;

const VP: (f32, f32) = (940.0, 600.0);

/// A selection in the middle of the page, well clear of every edge.
fn selection() -> Rect {
    Rect::new(300.0, 260.0, 180.0, 22.0)
}

/// The round-trip that matters: every button hit-tests back to itself, at its
/// centre and at both corners. An off-by-one passes a centre test.
#[test]
fn every_button_hit_tests_back_to_itself() {
    let p = panel(selection(), VP, 0.0);
    for placed in buttons(p) {
        let r = placed.rect;
        assert_eq!(hit(p, r.centre().0, r.centre().1), Some(placed.button));
        assert_eq!(hit(p, r.x + 0.5, r.y + 0.5), Some(placed.button));
        assert_eq!(
            hit(p, r.x + r.w - 0.5, r.y + r.h - 0.5),
            Some(placed.button)
        );
    }
}

/// Every button is inside the panel, and the last one ends inside its padding.
/// This is what `width()` claims and what a drifted `buttons` would break.
#[test]
fn the_buttons_fill_the_panel_they_claim() {
    let p = panel(selection(), VP, 0.0);
    let placed = buttons(p);
    assert_eq!(placed.first().map(|b| b.rect.x), Some(p.x + PAD_X));
    let last = placed.last().expect("the bubble has buttons");
    assert!((last.rect.x + last.rect.w + PAD_X - (p.x + p.w)).abs() < 0.01);
    for b in &placed {
        assert!(b.rect.y >= p.y && b.rect.y + b.rect.h <= p.y + p.h);
    }
}

/// A separator falls in a gap, not on a button.
#[test]
fn a_separator_never_lands_on_a_button() {
    let p = panel(selection(), VP, 0.0);
    for x in separators(p) {
        assert_eq!(hit(p, x, p.y + p.h * 0.5), None, "separator at {x}");
    }
}

/// It floats above the selection by preference — a bubble under it would cover
/// the next line of what you are reading.
#[test]
fn it_sits_above_the_selection() {
    let sel = selection();
    let p = panel(sel, VP, 0.0);
    assert!(p.y + p.h <= sel.y, "bubble clears the selection");
}

/// A selection against the top of the viewport flips it below, rather than
/// drawing it off-screen or under the caption.
#[test]
fn a_selection_at_the_top_flips_the_bubble_below() {
    let sel = Rect::new(300.0, 8.0, 180.0, 22.0);
    let p = panel(sel, VP, theme::CAPTION_H);
    assert!(p.y >= sel.y + sel.h, "bubble drops under the selection");
}

/// It clamps horizontally rather than running off either edge.
#[test]
fn it_clamps_to_the_viewport_horizontally() {
    let left = panel(Rect::new(0.0, 260.0, 20.0, 22.0), VP, 0.0);
    assert!(left.x >= EDGE_MARGIN);
    let right = panel(Rect::new(VP.0 - 20.0, 260.0, 20.0, 22.0), VP, 0.0);
    assert!(right.x + right.w <= VP.0 - EDGE_MARGIN);
}

/// A menu button is one with a rect to hang a panel from — asked for by name,
/// because `press_bubble` looks it up rather than remembering where it drew
/// it. Only the block-type button today: `⋯` is off the bubble while its two
/// commands do not work (see `LAYOUT`), and rejoins this list when it returns.
#[test]
fn the_menu_buttons_report_the_rect_they_were_drawn_at() {
    let p = panel(selection(), VP, 0.0);
    for button in [Button::BlockType, Button::List] {
        let r = button_rect(p, button).expect("the menu button is in the layout");
        assert_eq!(hit(p, r.centre().0, r.centre().1), Some(button));
    }
    assert!(
        button_rect(p, Button::More).is_none(),
        "the overflow stays off the bubble while super/subscript do nothing"
    );
}

/// Every button says something on hover. A tooltip that is the empty string is
/// a button nobody can learn.
#[test]
fn every_button_has_a_tooltip() {
    let p = panel(selection(), VP, 0.0);
    for placed in buttons(p) {
        assert!(
            !placed.button.tooltip().is_empty(),
            "{:?} has no tooltip",
            placed.button
        );
    }
}

#[test]
fn list_tooltip_names_the_active_style() {
    let mut state = docview::State::default();
    for (style, expected) in [
        (
            koru_core::ListStyle::Bullet,
            Command::BulletedList.tooltip(),
        ),
        (
            koru_core::ListStyle::Numbered,
            Command::NumberedList.tooltip(),
        ),
        (koru_core::ListStyle::Checkbox, Command::Tasks.tooltip()),
    ] {
        state.list_style = Some(style);
        assert_eq!(Button::List.tooltip_for(&state), expected);
    }
}

#[test]
fn list_trigger_icon_names_the_active_style() {
    let mut state = docview::State::default();
    for (style, expected) in [
        (koru_core::ListStyle::Bullet, koru_draw::icons::LIST),
        (koru_core::ListStyle::Numbered, koru_draw::icons::LIST_NUM),
        (
            koru_core::ListStyle::Checkbox,
            koru_draw::icons::LIST_CHECKS,
        ),
    ] {
        state.list_style = Some(style);
        assert_eq!(list_icon(&state), expected);
    }
}
