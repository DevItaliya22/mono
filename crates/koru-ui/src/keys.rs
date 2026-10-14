// SPDX-License-Identifier: MPL-2.0

//! The one thing every chord table has to agree about: which letter a Ctrl
//! chord is on.
//!
//! It lives here rather than in `koru-editor` because [`TextField`] needs it
//! and a text field is a widget: `koru-editor` pulls in the node model, the
//! workspace and the syntax engine, and a one-line prompt box may not acquire
//! any of those to find out that Ctrl+K was pressed.
//! `koru_editor::input::keymap` re-exports this, so every existing chord table
//! still matches through the same function.
//!
//! [`debug`] is here for the same reason one step out: what it prints is a
//! `Key`, a `ModifiersState` and the answer [`ctrl_letter`] gave, so it is a
//! description of the thing this module normalises. It was `koru_workspace_host::
//! inputdebug`, which meant a surface could not say what key it received
//! without linking a workspace.
//!
//! [`TextField`]: crate::text_field::TextField

pub mod debug;

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// The character a Ctrl chord is on, normalised.
///
/// Ctrl+K reaches us as `Character("k")` on some platforms and as the ASCII
/// control character (0x0B) on others — Windows in particular delivers the
/// control code in `text`. Matching only on `"k"` is why Ctrl+K did nothing.
///
/// Punctuation has the same split and a smaller alphabet of it: Ctrl+`\` is
/// 0x1C, and the three codes after it belong to `]`, `^` and `_`. Anything else
/// printable is simply itself, which is what lets a chord be written on a key
/// that is not a letter at all.
pub fn ctrl_letter(key: &Key, text: Option<&str>) -> Option<char> {
    fn from_char(ch: char) -> Option<char> {
        if ch.is_ascii_alphabetic() {
            return Some(ch.to_ascii_lowercase());
        }
        // 0x01..=0x1A are Ctrl+A..Ctrl+Z.
        let code = ch as u32;
        if (1..=26).contains(&code) {
            return Some((b'a' + (code as u8 - 1)) as char);
        }
        match ch {
            '\u{1c}' => Some('\\'),
            '\u{1d}' => Some(']'),
            '\u{1e}' => Some('^'),
            '\u{1f}' => Some('_'),
            c if !c.is_control() => Some(c),
            _ => None,
        }
    }

    if let Key::Character(c) = key
        && let Some(found) = c.chars().next().and_then(from_char)
    {
        return Some(found);
    }
    // Ctrl+I is indistinguishable from Tab (0x09) on Windows, and arrives as a
    // named key with no text. Map it back so Ctrl+I can mean italic.
    if matches!(key, Key::Named(NamedKey::Tab)) {
        return Some('i');
    }
    text.and_then(|t| t.chars().next()).and_then(from_char)
}

/// Recover a modified Backspace when a platform or embedded host reports its
/// ASCII control character instead of a named logical key.
///
/// A window that has the `PhysicalKey` can do better, and `koru-workspace-host` does:
/// there, Backspace and Ctrl+H are distinguishable. **A reusable surface does
/// not always get that detail** — `View::key` takes a logical key on purpose,
/// because a `KeyEvent` cannot be constructed in a test — so this is the shared
/// fallback, used before shortcut parsing and by one-line inputs themselves.
///
/// Without it, `Ctrl+Backspace` in a text field is indistinguishable from
/// `Ctrl+H` and opens Help instead of deleting a word.
///
/// Here rather than in `koru-editor` for the same reason [`ctrl_letter`] is:
/// `TextField` needs it, and a prompt box may not link the node model to
/// delete a word. `koru_editor::input::keymap` re-exports it.
pub fn recover_modified_editing_key(key: &Key, mods: ModifiersState) -> Option<Key> {
    ((mods.control_key() || mods.super_key())
        && matches!(key, Key::Character(value) if matches!(value.as_str(), "\u{8}" | "\u{7f}")))
    .then_some(Key::Named(NamedKey::Backspace))
}

#[cfg(test)]
mod tests;
