// SPDX-License-Identifier: MPL-2.0

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTurnId, AttachmentId, ErrorCode, IdempotencyKey, SessionFailure, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentSource {
    Clipboard,
    Drop,
    FilePicker,
    PublicUpload,
    AgentGenerated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentStatus {
    Ingesting,
    Pending,
    Rejected { error: AttachmentValidationError },
    Removed,
}

impl AttachmentStatus {
    pub fn can_transition_to(&self, next: &Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (
                Self::Ingesting,
                Self::Pending | Self::Rejected { .. } | Self::Removed
            ) | (Self::Pending, Self::Removed)
                | (Self::Rejected { .. }, Self::Removed)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    NativeStructured,
    QuotedPath,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Delivering {
        turn_id: AgentTurnId,
    },
    Delivered {
        turn_id: AgentTurnId,
    },
    Failed {
        turn_id: AgentTurnId,
        error: SessionFailure,
    },
}

impl DeliveryState {
    pub fn can_transition_to(&self, next: &Self) -> bool {
        if self == next {
            return true;
        }
        match (self, next) {
            (Self::Pending, Self::Delivering { .. }) => true,
            (Self::Delivering { turn_id }, Self::Delivered { turn_id: next_turn })
            | (
                Self::Delivering { turn_id },
                Self::Failed {
                    turn_id: next_turn, ..
                },
            ) => turn_id == next_turn,
            (Self::Failed { turn_id, .. }, Self::Delivering { turn_id: next_turn }) => {
                turn_id == next_turn
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentValidationError {
    pub code: ErrorCode,
    /// Content-free developer detail. Never a path, filename, or input value.
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    pub id: AttachmentId,
    pub session_id: SessionId,
    pub kind: AttachmentKind,
    pub source: AttachmentSource,
    pub media_type: String,
    pub byte_count: u64,
    pub dimensions: Option<ImageDimensions>,
    pub status: AttachmentStatus,
    pub delivery_mode: DeliveryMode,
    pub delivery: DeliveryState,
    pub turn_id: Option<AgentTurnId>,
    pub preview_available: bool,
    pub created_at_ms: i64,
}

impl Attachment {
    /// Turn association is immutable. Replaying the same association is safe.
    pub fn associate_turn(&mut self, turn_id: AgentTurnId) -> Result<(), ErrorCode> {
        match self.turn_id {
            None => {
                self.turn_id = Some(turn_id);
                Ok(())
            }
            Some(existing) if existing == turn_id => Ok(()),
            Some(_) => Err(ErrorCode::InvalidTransition),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PendingInput {
    pub idempotency_key: IdempotencyKey,
    pub text: String,
    /// Ordered by the person composing the input.
    pub attachment_ids: Vec<AttachmentId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentDeliveryResult {
    pub attachment_id: AttachmentId,
    pub mode: DeliveryMode,
    pub state: DeliveryState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendPendingInputResult {
    pub turn_id: AgentTurnId,
    pub attachments: Vec<AttachmentDeliveryResult>,
}
