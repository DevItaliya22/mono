// SPDX-License-Identifier: MPL-2.0

//! The primary modifier, and the predicates that ask about it.
//!
//! **Ctrl on Windows and Linux; ⌘ on macOS.** Every chord in the app that is
//! "the app's modifier plus a key" is written against [`PRIMARY`], so the
//! platform difference is resolved in one place rather than at fifteen call
//! sites testing `mods.control_key()` directly.
//!
//! ## Why the *foreign* one matters too
//!
//! A chord must not fire when the **other** platform's primary is held. Without
//! that check, ⌘+B on Windows — a key combination that means nothing there —
//! would italicise, because only Ctrl was being tested and ⌘ was ignored. The
//! editor's key map has had this right since it grew a macOS arm; nothing else
//! has. [`has_primary`] and [`has_foreign_primary`] are the pair, and a
//! resolver wants both.
//!
//! ## Why this is not `cfg!` at each call site
//!
//! It could be. It was — in one file, correctly, and nowhere else. The value of
//! a constant is that a test can name it: `apps/koru/src/tests.rs` and four
//! other suites declared `const CTRL: ModifiersState = ModifiersState::CONTROL`
//! and so asserted, on every platform, that *Control* does what only the
//! *primary* modifier should. That passed on Windows and failed 72 tests the
//! first time the suite was run on a Mac. Those constants now come from here.

use winit::keyboard::ModifiersState;

/// No modifiers. Named so a key-map test reads as a table rather than as
/// `ModifiersState::empty()` repeated forty times.
pub const NONE: ModifiersState = ModifiersState::empty();

/// Shift alone.
pub const SHIFT: ModifiersState = ModifiersState::SHIFT;

/// The app's own modifier: **Ctrl** on Windows and Linux, **⌘** on macOS.
///
/// `SUPER` is winit's name for the ⌘ key; it is the Windows key on Windows,
/// which is precisely why this is not simply `SUPER` everywhere.
pub const PRIMARY: ModifiersState = if cfg!(target_os = "macos") {
    ModifiersState::SUPER
} else {
    ModifiersState::CONTROL
};

/// [`PRIMARY`] with Shift — the second most common shape in the key map.
pub const PRIMARY_SHIFT: ModifiersState =
    ModifiersState::from_bits_truncate(PRIMARY.bits() | ModifiersState::SHIFT.bits());

/// [`PRIMARY`] with Alt. On macOS ⌥ composes characters, so a chord written
/// this way needs checking against a real keyboard before it is relied on —
/// which is why ⌘⌥W and ⌘⌥P are two presses on the runtime pass in
/// `docs/features/macos/CATCHUP.md` M4 rather than something a test can
/// answer.
pub const PRIMARY_ALT: ModifiersState =
    ModifiersState::from_bits_truncate(PRIMARY.bits() | ModifiersState::ALT.bits());

/// The modifier of the six view chords (Documents, Files, Tasks, Chats,
/// Agents, Artifacts): **Alt** on Windows and Linux, **⌃⌘** on macOS.
///
/// On a Mac, ⌥ with a digit *types* — `¡ ™ £ ¢ ∞ §` on a US layout, `@` and
/// others elsewhere — so `⌥1` reached the view resolver as `¡` and never
/// switched anything, and claiming it from the physical key would have taken
/// those characters away inside Koru. ⌃⌘ is free: macOS claims no ⌃⌘digit
/// system-wide, ⌘digit is a tab here and ⌘⌥digit a heading. The maintainers chose a
/// Mac chord over taking the characters (2026-10-09, M42).
pub const VIEW: ModifiersState = if cfg!(target_os = "macos") {
    ModifiersState::from_bits_truncate(
        ModifiersState::CONTROL.bits() | ModifiersState::SUPER.bits(),
    )
} else {
    ModifiersState::ALT
};

/// Is the platform's primary modifier held?
pub fn has_primary(mods: ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        mods.super_key()
    } else {
        mods.control_key()
    }
}

/// Is the **other** platform's primary held — ⌘ on Windows, Ctrl on macOS?
///
/// A chord should refuse to fire when this is true. It is not the same as "some
/// modifier we do not care about": on macOS, Ctrl is a real modifier with its
/// own meanings (Ctrl+Tab, emacs-style motion), and treating it as noise is how
/// a Mac app ends up firing two things at once.
pub fn has_foreign_primary(mods: ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        mods.control_key()
    } else {
        mods.super_key()
    }
}

/// The primary modifier and nothing else — no Shift, no Alt, and not the
/// foreign primary either.
///
/// This is the guard most call sites actually want. Written out, it is four
/// conditions, and the fifteen hand-written copies of it in this tree disagreed
/// about which ones to include: several checked Ctrl and Shift but not Alt, so
/// `Ctrl+Alt+N` opened a second window as well as setting a document Narrow.
pub fn primary_only(mods: ModifiersState) -> bool {
    has_primary(mods) && !has_foreign_primary(mods) && !mods.shift_key() && !mods.alt_key()
}

/// Is **either** platform's primary held, so this keystroke is a chord rather
/// than typing?
///
/// Deliberately not [`has_primary`]. A key that types a character must not type
/// it under Ctrl *or* ⌘ on either platform — Ctrl+K on a Mac is not a chord this
/// app binds, but it must still not put a `k` in the box. That is why the three
/// call sites for this spell out `control_key() || super_key()` rather than
/// asking about the primary alone, and why widening them to the primary would
/// be a regression rather than a tidy-up.
pub fn is_chord(mods: ModifiersState) -> bool {
    has_primary(mods) || has_foreign_primary(mods)
}

/// `mods` with the primary modifier cleared, so a caller can compare the rest.
pub const fn without_primary(mods: ModifiersState) -> ModifiersState {
    ModifiersState::from_bits_truncate(mods.bits() & !PRIMARY.bits())
}

#[cfg(test)]
mod tests;
