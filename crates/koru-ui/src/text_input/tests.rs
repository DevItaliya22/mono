// SPDX-License-Identifier: MPL-2.0

//! The arithmetic this type exists to have only one of, and the capture rule.
//!
//! The *editing* is `TextField`'s and is tested there; what is new here is the
//! pairing with a rectangle.

use super::*;

const R: Rect = Rect {
    x: 100.0,
    y: 50.0,
    w: 200.0,
    h: 26.0,
};

fn input() -> (TextInput, TextEngine) {
    let mut e = TextEngine::new();
    let i = TextInput::with_text(&mut e, theme::role::row(), "hello world");
    (i, e)
}

/// A press outside is not this input's, and says so — a caller uses the answer
/// to decide whether the press was theirs at all.
#[test]
fn a_press_outside_is_not_taken() {
    let (mut i, mut e) = input();
    assert!(!i.click(&mut e, R, 10.0, 10.0));
    assert!(i.click(&mut e, R, 150.0, 60.0));
}

/// **A drag that leaves the box is still this input's.** Selecting by dragging
/// past the right-hand edge is ordinary, and a hit test on every move would
/// drop the selection the moment the pointer left.
#[test]
fn a_drag_keeps_the_field_once_it_has_started() {
    let (mut i, mut e) = input();
    assert!(i.click(&mut e, R, 150.0, 60.0));
    assert!(i.drag(&mut e, R, 900.0), "far outside, still ours");
    i.release();
    assert!(!i.drag(&mut e, R, 150.0), "and released, it is not");
}

/// A drag that never began is not one.
#[test]
fn a_drag_without_a_press_does_nothing() {
    let (mut i, mut e) = input();
    assert!(!i.drag(&mut e, R, 150.0));
}

/// The placeholder is a hint about what to type, so it goes the moment there is
/// something typed. Asserted through `is_empty`, which is what gates it.
#[test]
fn the_placeholder_only_applies_to_an_empty_field() {
    let mut e = TextEngine::new();
    let mut i = TextInput::new(&mut e, theme::role::row());
    assert!(i.is_empty());
    i.set_text(&mut e, "x");
    assert!(!i.is_empty());
}

/// Styles differ in the box, never in the behaviour — that is the whole
/// arrangement. A search chip has no border; an inline field has one; and an
/// invalid one differs from its valid self **only** in the border colour.
#[test]
fn a_style_changes_the_box_and_nothing_else() {
    let search = InputStyle::search();
    let inline = InputStyle::inline();
    assert!(search.border.is_none());
    assert!(inline.border.is_some());

    let invalid = inline.invalid();
    assert_ne!(invalid.border, inline.border, "the border says it");
    assert_eq!(invalid.fill, inline.fill);
    assert_eq!(invalid.radius, inline.radius);
    assert_eq!(invalid.style, inline.style);

    let bare = InputStyle::bare(theme::role::hint());
    assert!(bare.fill.is_none() && bare.border.is_none());
    assert_eq!(bare.style, theme::role::hint());
}

/// `Ctrl+Backspace` deletes a word. Not this type's behaviour — it is
/// `TextField`'s — but the reason this type forwards every key untouched rather
/// than filtering, so a box built with it behaves like the slash menu's.
#[test]
fn ctrl_backspace_deletes_a_word_through_the_input() {
    use winit::keyboard::NamedKey;
    let (mut i, mut e) = input();
    i.key(
        &mut e,
        &Key::Named(NamedKey::Backspace),
        None,
        koru_keys::PRIMARY,
    );
    assert_eq!(
        i.value(),
        "hello",
        "the last word went, not the last letter"
    );
}

/// Embedded hosts can deliver the same chord as an ASCII control character.
/// The shared input owns that fallback so search and command prompts do not
/// need different editing implementations.
#[test]
fn raw_ctrl_backspace_deletes_a_word_through_the_input() {
    let (mut i, mut e) = input();
    for raw in ["\u{8}", "\u{7f}"] {
        i.set_text(&mut e, "hello world");
        i.key(
            &mut e,
            &Key::Character(raw.into()),
            None,
            ModifiersState::CONTROL,
        );
        assert_eq!(i.value(), "hello");
    }
}
// ---------------------------------------------------------------- dictation
//
// The composition itself is `TextField`'s and is tested there. What is new
// here is that an input **forwards** it: the field is private, so nothing is
// inherited, and a box that silently accepted no dictation would be the exact
// divergence this type exists to prevent.

/// Every verb reaches the field, and the authored/on-screen split survives the
/// forwarding rather than being flattened by it.
#[test]
fn an_input_forwards_a_whole_dictation() {
    let (mut i, mut e) = input();
    i.select_all(&mut e);
    assert!(i.begin_composition(&mut e));
    i.update_composition(&mut e, "goodbye");

    assert_eq!(i.raw_text(), "hello world", "authored");
    assert_eq!(i.value(), "hello world");
    assert!(i.is_composing());
    assert!(i.composition_rect().is_some());

    i.commit_composition(&mut e, "goodbye moon");
    assert_eq!(i.value(), "goodbye moon");
    assert!(!i.is_composing());

    i.key(
        &mut e,
        &Key::Character("z".into()),
        None,
        ModifiersState::CONTROL,
    );
    assert_eq!(i.value(), "hello world");
}

#[test]
fn an_input_cancels_a_dictation_without_a_trace() {
    let (mut i, mut e) = input();
    i.begin_composition(&mut e);
    i.update_composition(&mut e, "nonsense");
    i.cancel_composition(&mut e);
    assert_eq!(i.raw_text(), "hello world");
    assert!(!i.is_composing());
}

/// The named trap, at the layer that actually draws the ghost: an
/// authored-empty box holding a provisional transcript is empty, and `draw`
/// asks `shows_placeholder` rather than `is_empty` for exactly that reason.
#[test]
fn an_empty_input_holding_a_partial_shows_no_placeholder() {
    let mut e = TextEngine::new();
    let mut i = TextInput::new(&mut e, theme::role::row());
    assert!(i.shows_placeholder());

    i.begin_composition(&mut e);
    i.update_composition(&mut e, "hello");
    assert!(i.is_empty(), "nothing authored");
    assert!(!i.shows_placeholder(), "but the ghost would sit under it");
}

/// Both answer from what is on screen (ADR-272's table): a caret sitting
/// before a provisional run is not at the end of anything, and there is no
/// selection to paint once the dictation has replaced it.
#[test]
fn the_on_screen_queries_answer_from_the_screen() {
    let (mut i, mut e) = input();
    i.select_all(&mut e);
    assert!(i.has_selection());

    i.begin_composition(&mut e);
    i.update_composition(&mut e, "goodbye");
    assert!(!i.has_selection(), "the selection is gone from the screen");
    assert!(i.caret_at_end(), "the caret follows the provisional run");
}

/// **The end icon is the input's own.** A press on the eye reveals a secret
/// field, a second hides it, and neither moves the caret or starts a drag —
/// it is a button inside the box, not text.
#[test]
fn the_reveal_end_icon_toggles_a_secret_and_is_not_a_text_press() {
    let (i, mut e) = input();
    let mut i = i.secret().with_end_icon(EndIcon::Reveal);
    let box_r = Rect::new(100.0, 50.0, 300.0, 44.0);
    assert!(i.masked());
    let eye = i.end_icon_rect(box_r).expect("an eye");
    assert!(eye.x + eye.w <= box_r.x + box_r.w && eye.x > box_r.x + box_r.w * 0.5);
    i.move_to_end(&mut e);
    let caret = i.caret();
    assert!(i.click(&mut e, box_r, eye.x + 4.0, eye.y + 4.0));
    assert!(i.revealed() && !i.masked());
    assert_eq!(i.caret(), caret, "the caret did not move");
    assert!(!i.is_dragging());
    assert_eq!(i.press_end_icon(), Some(EndIcon::Reveal));
    assert!(i.masked());
    // A press on the text is a text press, and leaves the eye alone.
    assert!(i.click(&mut e, box_r, box_r.x + 20.0, box_r.y + 20.0));
    assert!(i.masked());
    i.release();
    // Revealed, then cleared: concealed again.
    i.press_end_icon();
    i.conceal();
    assert!(i.masked());
}

/// An input without an end icon has none to press or draw.
#[test]
fn a_plain_input_has_no_end_icon() {
    let (mut i, _) = input();
    assert!(i.end_icon_rect(R).is_none());
    assert_eq!(i.press_end_icon(), None);
    assert!(!i.masked());
}
