// SPDX-License-Identifier: MPL-2.0

//! Lossless, host-neutral input records delivered by the shared window loop.
//!
//! Logical and physical keys remain winit's stable cross-platform vocabulary;
//! native scan codes and platform messages stop below this module. The records
//! borrow winit's text for one synchronous dispatch, so a key event allocates
//! nothing merely because an application asked for the richer hook.

use winit::event::{ElementState, Ime, MouseButton};
use winit::keyboard::{Key, KeyLocation, ModifiersState, PhysicalKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    Pressed,
    Released,
}

impl KeyState {
    pub fn is_pressed(self) -> bool {
        self == Self::Pressed
    }
}

impl From<ElementState> for KeyState {
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => Self::Pressed,
            ElementState::Released => Self::Released,
        }
    }
}

/// One keyboard event with both text-editing and terminal-routing identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RichKeyEvent<'a> {
    pub logical_key: &'a Key,
    pub physical_key: PhysicalKey,
    pub location: KeyLocation,
    pub text: Option<&'a str>,
    pub state: KeyState,
    pub repeat: bool,
    pub modifiers: ModifiersState,
}

/// Input-method composition with byte-indexed preedit selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeEvent<'a> {
    Enabled,
    Preedit {
        text: &'a str,
        cursor: Option<(usize, usize)>,
    },
    Commit {
        text: &'a str,
    },
    Disabled,
}

impl<'a> From<&'a Ime> for ImeEvent<'a> {
    fn from(event: &'a Ime) -> Self {
        match event {
            Ime::Enabled => Self::Enabled,
            Ime::Preedit(text, cursor) => Self::Preedit {
                text,
                cursor: *cursor,
            },
            Ime::Commit(text) => Self::Commit { text },
            Ime::Disabled => Self::Disabled,
        }
    }
}

/// A button transition for a surface which owns terminal-style mouse policy.
/// Pointer motion and wheel deltas keep using the existing allocation-free
/// callbacks; this record adds every button, release coordinates, modifiers,
/// and a click count without exposing native platform messages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RichMouseEvent {
    pub x: f32,
    pub y: f32,
    pub button: MouseButton,
    pub state: KeyState,
    pub modifiers: ModifiersState,
    pub click_count: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_preedit_and_commit_keep_text_and_byte_offsets() {
        let preedit = Ime::Preedit("a b".into(), Some((1, 3)));
        assert_eq!(
            ImeEvent::from(&preedit),
            ImeEvent::Preedit {
                text: "a b",
                cursor: Some((1, 3)),
            }
        );
        let commit = Ime::Commit("啊不".into());
        assert_eq!(ImeEvent::from(&commit), ImeEvent::Commit { text: "啊不" });
    }
}
