// SPDX-License-Identifier: MPL-2.0

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ControllerLeaseToken, GridSize, IdempotencyKey, SessionId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalKey {
    Character { text: String },
    Enter,
    Tab,
    Backspace,
    Escape,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Function { number: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyStroke {
    pub key: TerminalKey,
    pub modifiers: KeyModifiers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    Press,
    Release,
    Move,
}

/// One mouse report in zero-based active-grid coordinates.
///
/// The runtime combines this provider-neutral record with the emulator's live
/// negotiated tracking and encoding modes. A move with no button represents
/// all-motion reporting; drag moves retain their pressed button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalMouseEvent {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub column: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}

impl TerminalMouseEvent {
    pub fn validate(self) -> Result<(), crate::ErrorCode> {
        let valid_button = match self.action {
            MouseAction::Press | MouseAction::Release => self.button.is_some(),
            MouseAction::Move => self.button.is_none_or(|button| {
                !matches!(button, MouseButton::WheelUp | MouseButton::WheelDown)
            }),
        };
        valid_button
            .then_some(())
            .ok_or(crate::ErrorCode::InvalidRequest)
    }
}

/// Raw, provider-independent actions. Attach/list/subscription envelopes belong
/// to the future private wire rather than being mislabeled PTY commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RawCommand {
    WriteBytes { bytes: Vec<u8> },
    WriteText { text: String },
    SendKey { key: KeyStroke },
    SendMouse { event: TerminalMouseEvent },
    SetFocus { focused: bool },
    PasteText { text: String },
    Interrupt,
    Resize { size: GridSize },
    Terminate,
}

impl RawCommand {
    /// First negotiated protocol version that can carry this behavior.
    pub fn minimum_protocol_version(&self) -> u32 {
        match self {
            Self::SendMouse { .. } | Self::SetFocus { .. } => 2,
            _ => 1,
        }
    }

    pub fn validate(&self) -> Result<(), crate::ErrorCode> {
        match self {
            Self::Resize { size } => size.validate(),
            Self::WriteBytes { bytes } if bytes.is_empty() => Err(crate::ErrorCode::InvalidRequest),
            Self::WriteText { text } | Self::PasteText { text } if text.is_empty() => {
                Err(crate::ErrorCode::InvalidRequest)
            }
            Self::SendKey {
                key:
                    KeyStroke {
                        key: TerminalKey::Function { number },
                        ..
                    },
            } if !(1..=24).contains(number) => Err(crate::ErrorCode::InvalidRequest),
            Self::SendMouse { event } => event.validate(),
            _ => Ok(()),
        }
    }
}

/// A retry-safe, lease-gated mutation of one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCommand {
    pub idempotency_key: IdempotencyKey,
    pub session_id: SessionId,
    pub controller_lease: ControllerLeaseToken,
    pub action: RawCommand,
}
