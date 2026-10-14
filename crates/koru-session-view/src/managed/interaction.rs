// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::{AgentTurnId, ApprovalDecision, ApprovalId, ErrorCode, StreamSequence};

/// Managed-session payload carried by the shared session-surface boundary.
///
/// The host adds session identity, controller authority, IPC idempotency, and
/// transport. In particular, presenting an approval and answering it are two
/// different intents so selecting a background session can never approve work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagedIntent {
    AnswerApproval {
        approval_id: ApprovalId,
        decision: ApprovalDecision,
        choice: Option<koru_session_protocol::ApprovalChoiceId>,
    },
    CancelTurn {
        turn_id: AgentTurnId,
    },
    AttentionPresented {
        through: StreamSequence,
    },
}

/// Window-independent key vocabulary for the managed pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedKey {
    Tab { reverse: bool },
    Enter,
    Space,
    Escape,
    CancelTurn,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManagedInputOutcome {
    pub handled: bool,
    pub redraw: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedHit {
    ReviewApproval(ApprovalId),
    ApprovalReject,
    ApprovalApprove,
    ApprovalChoice(koru_session_protocol::ApprovalChoiceId),
    CancelTurn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalFocus {
    Reject,
    Approve,
    Choice(koru_session_protocol::ApprovalChoiceId),
}

impl ApprovalFocus {
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Reject => Self::Approve,
            Self::Approve | Self::Choice(_) => Self::Reject,
        }
    }
}

/// Result returned by a host after an asynchronous approval command fails.
/// `ApprovalStale` and `ApprovalNotFound` make the visible request inert;
/// transient failures restore its answer controls.
pub(crate) fn approval_is_stale(code: &ErrorCode) -> bool {
    matches!(code, ErrorCode::ApprovalStale | ErrorCode::ApprovalNotFound)
}
