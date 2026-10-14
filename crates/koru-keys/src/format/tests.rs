// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::chord::named;
use crate::modifiers::{NONE, PRIMARY, PRIMARY_SHIFT};

#[test]
fn a_primary_letter_reads_the_platforms_way() {
    let got = label_for(PRIMARY, On::Letter('s'));
    if cfg!(target_os = "macos") {
        assert_eq!(got, "⌘S");
    } else {
        assert_eq!(got, "Ctrl+S");
    }
}

/// **macOS has a fixed modifier order, ⌃⌥⇧⌘**, and prints no separator. A menu
/// that writes `⌘⇧S` reads as wrong to anyone who uses the platform.
#[test]
fn mac_orders_modifiers_and_drops_the_separator() {
    let got = label_for(PRIMARY_SHIFT, On::Letter('s'));
    if cfg!(target_os = "macos") {
        assert_eq!(got, "⇧⌘S");
        assert!(!got.contains('+'));
    } else {
        assert_eq!(got, "Ctrl+Shift+S");
    }
}

#[test]
fn named_keys_are_glyphs_on_mac_and_words_elsewhere() {
    let backspace = label_for(NONE, named(NamedKey::Backspace));
    let escape = label_for(NONE, named(NamedKey::Escape));
    if cfg!(target_os = "macos") {
        assert_eq!(backspace, "⌫");
        assert_eq!(escape, "⎋");
    } else {
        assert_eq!(backspace, "Backspace");
        assert_eq!(escape, "Esc");
    }
}

/// The arrows are the one family both platforms write the same way, so a table
/// does not need a branch for them.
#[test]
fn arrows_are_glyphs_everywhere() {
    assert_eq!(label_for(NONE, named(NamedKey::ArrowUp)), "↑");
    assert_eq!(label_for(NONE, named(NamedKey::ArrowRight)), "→");
}

/// A chord with no modifiers is just the key — not a stray separator.
#[test]
fn an_unmodified_chord_is_just_the_key() {
    assert_eq!(label_for(NONE, named(NamedKey::F2)), "F2");
    assert!(!label_for(NONE, named(NamedKey::F2)).contains('+'));
}

/// Letters are upper-cased for display even though the chord matches
/// lower-case. `Ctrl+s` in a menu looks like a typo.
#[test]
fn letters_display_upper_case() {
    assert!(label_for(PRIMARY, On::Letter('b')).ends_with('B'));
}

/// The label and the chord that produces it come from one value, so a menu row
/// cannot advertise a chord the key map does not have.
#[test]
fn label_agrees_with_label_for() {
    let c = Chord::new(On::Letter('k'), PRIMARY);
    assert_eq!(label(&c), label_for(c.mods, c.on));
}
