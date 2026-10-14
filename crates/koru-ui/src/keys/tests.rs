// SPDX-License-Identifier: MPL-2.0

//! Both encodings and the Tab collision, because which one arrives is
//! platform trivia and every chord table in the tree matches through here.

use super::*;

fn ch(s: &str) -> Key {
    Key::Character(s.into())
}

/// Both encodings, because which one arrives is platform trivia and every
/// chord table in the tree matches through this function.
#[test]
fn ctrl_letter_accepts_both_encodings() {
    assert_eq!(ctrl_letter(&ch("k"), None), Some('k'));
    assert_eq!(ctrl_letter(&ch("K"), None), Some('k'));
    assert_eq!(
        ctrl_letter(&ch("\u{0b}"), None),
        Some('k'),
        "the ASCII control code Windows sends"
    );
    assert_eq!(
        ctrl_letter(&Key::Named(NamedKey::Space), Some("\u{0b}")),
        Some('k'),
        "and the same code arriving as text"
    );
    assert_eq!(ctrl_letter(&ch("\u{01}"), None), Some('a'));
    assert_eq!(ctrl_letter(&Key::Named(NamedKey::Enter), None), None);
}

/// Ctrl+I *is* Tab on Windows. Nothing at a call site hints at it, which is
/// why it is normalised in one place.
#[test]
fn tab_is_ctrl_i() {
    assert_eq!(ctrl_letter(&Key::Named(NamedKey::Tab), None), Some('i'));
}

/// A chord can be written on a key that is not a letter at all.
#[test]
fn punctuation_survives_in_both_forms() {
    assert_eq!(ctrl_letter(&ch("."), None), Some('.'));
    assert_eq!(ctrl_letter(&ch("\u{1c}"), None), Some('\\'));
    assert_eq!(ctrl_letter(&ch("1"), None), Some('1'));
}
