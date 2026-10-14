// SPDX-License-Identifier: MPL-2.0

//! How a chord is written where a person reads it.
//!
//! `Ctrl+Shift+S` on Windows; `⇧⌘S` on macOS. The two conventions differ in
//! more than the glyphs:
//!
//! - **macOS uses no separator.** `⌘S`, not `⌘+S`.
//! - **macOS has a fixed modifier order** — ⌃ ⌥ ⇧ ⌘ — regardless of how the
//!   chord is written in the table. Windows has a conventional order too
//!   (Ctrl, Alt, Shift) but tolerates others; macOS does not, and a menu that
//!   prints `⌘⇧S` reads as wrong to anyone who uses the platform.
//! - **Named keys are glyphs**: ⌫ ⌦ ⏎ ⇥ ⎋ ↑ ↓ ← →, and the arrows are the same
//!   on both.
//!
//! This exists because 53 `"Ctrl+…"` literals and 24 `"Alt+…"`/`"Win+…"`
//! literals are spread across `blockmenu.rs`, `page_menu.rs`, `window_menu.rs`,
//! `context_menu.rs`, `welcome.rs`, `explore.rs` and `seed.rs`. Every one of
//! them is wrong on macOS, and correcting them individually is how the two
//! platforms drift the way `dist.sh` and `dist.ps1` did.

use winit::keyboard::{ModifiersState, NamedKey};

use crate::chord::{Chord, On};

/// Write `chord` the way this platform writes it.
pub fn label(chord: &Chord) -> String {
    label_for(chord.mods, chord.on)
}

/// Write a chord given its parts, for callers that do not have a [`Chord`].
pub fn label_for(mods: ModifiersState, on: On) -> String {
    let key = key_name(on);
    if cfg!(target_os = "macos") {
        // The platform's fixed order, and no separators.
        let mut out = String::new();
        if mods.control_key() {
            out.push('⌃');
        }
        if mods.alt_key() {
            out.push('⌥');
        }
        if mods.shift_key() {
            out.push('⇧');
        }
        if mods.super_key() {
            out.push('⌘');
        }
        out.push_str(&key);
        out
    } else {
        let mut parts: Vec<&str> = Vec::new();
        if mods.control_key() {
            parts.push("Ctrl");
        }
        if mods.alt_key() {
            parts.push("Alt");
        }
        if mods.shift_key() {
            parts.push("Shift");
        }
        if mods.super_key() {
            parts.push("Win");
        }
        if parts.is_empty() {
            return key;
        }
        format!("{}+{key}", parts.join("+"))
    }
}

/// How the key itself is written.
///
/// On macOS the editing and motion keys are glyphs — that is the convention,
/// and spelling them out reads as a port. Elsewhere they are words.
fn key_name(on: On) -> String {
    match on {
        On::Letter(c) => c.to_ascii_uppercase().to_string(),
        On::Named(n) => named_name(n),
    }
}

fn named_name(n: NamedKey) -> String {
    let mac = cfg!(target_os = "macos");
    match n {
        NamedKey::ArrowUp => "↑",
        NamedKey::ArrowDown => "↓",
        NamedKey::ArrowLeft => "←",
        NamedKey::ArrowRight => "→",
        NamedKey::Enter => {
            if mac {
                "⏎"
            } else {
                "Enter"
            }
        }
        NamedKey::Backspace => {
            if mac {
                "⌫"
            } else {
                "Backspace"
            }
        }
        NamedKey::Delete => {
            if mac {
                "⌦"
            } else {
                "Delete"
            }
        }
        NamedKey::Tab => {
            if mac {
                "⇥"
            } else {
                "Tab"
            }
        }
        NamedKey::Escape => {
            if mac {
                "⎋"
            } else {
                "Esc"
            }
        }
        NamedKey::Space => "Space",
        NamedKey::Home => "Home",
        NamedKey::End => "End",
        NamedKey::PageUp => {
            if mac {
                "⇞"
            } else {
                "PageUp"
            }
        }
        NamedKey::PageDown => {
            if mac {
                "⇟"
            } else {
                "PageDown"
            }
        }
        // The function keys, and anything else, print as winit names them —
        // `F2`, `F7`. Both platforms write those the same way.
        other => return format!("{other:?}"),
    }
    .to_string()
}

#[cfg(test)]
mod tests;
