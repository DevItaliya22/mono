// SPDX-License-Identifier: MPL-2.0

//! The one rule about cursors, in the one place that states it.
//!
//! **Koru does not use the hand cursor.** A press target is identified by its
//! own appearance — its lozenge, its hover background, its icon, its label —
//! not by the pointer changing shape over it. The hand is a *web* convention:
//! a browser has no other way to say "this word is a link", so it borrowed the
//! pointer. A desktop application draws controls that look like controls, and
//! native ones do not switch cursor over a button, a tab or a menu row. Koru
//! reads as a desktop application, so it does not either.
//!
//! This **reverses** the rule that stood until 2026-08-27, which said that
//! anything a press did something to showed `CursorIcon::Pointer` and that the
//! rule had no exceptions. See `docs/features/design/DESIGN.md` and
//! [ADR-215](../../../docs/decisions/README.md).
//!
//! Three shapes survive, and each survives because it carries information the
//! appearance of the thing cannot:
//!
//! - `Text` — a caret can go here, and the I-beam is also an aiming device.
//! - `Grab` / `Grabbing` — this can be picked up, and it is being held.
//! - `NsResize` and the other resize shapes — this edge or lane resizes.
//!
//! Everything else is [`PRESS`], which is [`CursorIcon::Default`]. It is a
//! named constant rather than a bare `Default` at forty call sites so that the
//! rule lives in one place: a hit-test that answers `PRESS` is still saying
//! *the pointer is over a control*, and the answer to what that looks like is
//! here rather than spread across six crates where it drifted back before.

use winit::window::CursorIcon;

/// The cursor over anything a press does something to — a chip, a pill, a
/// checkbox, a picker row, a menu item, the `+`, the `…`, a bubble button, an
/// emoji category, a tab, a window button, a column toggle, an explore row, a
/// search result.
///
/// It is the plain arrow, deliberately. See the module docs.
pub const PRESS: CursorIcon = CursorIcon::Default;

/// The cursor over nothing in particular — a margin, a bare strip of caption,
/// a disabled row.
///
/// Identical to [`PRESS`] and separate on purpose: the two say different things
/// about the hit-test that produced them, and collapsing them would lose the
/// only remaining record of which surfaces are pressable.
pub const IDLE: CursorIcon = CursorIcon::Default;

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule itself, pinned. Koru does not use the hand cursor; if this ever
    /// goes back to `Pointer`, the reversal was undone rather than re-decided.
    #[test]
    fn a_press_target_wears_the_plain_arrow() {
        assert_eq!(PRESS, CursorIcon::Default);
        assert_ne!(PRESS, CursorIcon::Pointer);
        assert_eq!(IDLE, CursorIcon::Default);
    }
}
