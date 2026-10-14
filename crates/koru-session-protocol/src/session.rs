// SPDX-License-Identifier: MPL-2.0

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AgentTurnId, CapabilitySet, ErrorCode, ProjectId, ProviderKind, SessionFailure, SessionId,
    SessionProfileKey,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    RawPty,
    Managed {
        provider: ProviderKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessState {
    Starting,
    Running,
    Exited { code: Option<i32> },
    Failed { error: SessionFailure },
}

impl ProcessState {
    /// The only process lifecycle transitions. Repeating the same state is an
    /// idempotent observation; terminal states never reopen.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (
                Self::Starting,
                Self::Running | Self::Exited { .. } | Self::Failed { .. }
            ) | (Self::Running, Self::Exited { .. } | Self::Failed { .. })
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControllerState {
    Detached,
    Controlled,
}

impl ControllerState {
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Detached, Self::Detached | Self::Controlled)
                | (Self::Controlled, Self::Controlled | Self::Detached)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ControllerLease {
    pub token: crate::ControllerLeaseToken,
    pub granted_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManagedState {
    Idle,
    Running,
    WaitingForInput,
    WaitingForApproval,
    Completed,
    Cancelled,
    Failed,
}

impl ManagedState {
    /// Transitions for one `AgentTurnId`. A later turn starts with a new id;
    /// terminal state for one turn cannot be reused as the next turn's state.
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::Idle, Self::Running | Self::Cancelled | Self::Failed)
                | (
                    Self::Running,
                    Self::WaitingForInput
                        | Self::WaitingForApproval
                        | Self::Completed
                        | Self::Cancelled
                        | Self::Failed
                )
                | (
                    Self::WaitingForInput | Self::WaitingForApproval,
                    Self::Running | Self::Completed | Self::Cancelled | Self::Failed
                )
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    BackgroundOutput,
    Bell,
    InputRequired,
    ApprovalRequired,
    Completed,
    Failed,
    Exited,
    AttachmentDelivered,
    AttachmentRejected,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttentionState {
    pub unread_count: u32,
    pub requires_user: bool,
    pub reason: Option<AttentionReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSummary {
    pub id: SessionId,
    pub parent_session_id: Option<SessionId>,
    pub profile: SessionProfileKey,
    pub display_name: String,
    pub working_directory: Option<String>,
    pub mode: SessionMode,
    pub process: ProcessState,
    pub controller: ControllerState,
    pub managed: Option<ManagedState>,
    pub active_turn_id: Option<AgentTurnId>,
    pub attention: AttentionState,
    pub pending_attachment_count: u32,
    #[serde(default)]
    pub capabilities: CapabilitySet,
    /// The project this session was placed in, if any. Absent from a peer
    /// that predates projects, which is why it defaults rather than being
    /// required; absent for a session nobody placed, which the tree draws
    /// under its virtual Unassigned bucket rather than a persisted project.
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// Protocol v14. Where a controller handover stands. `Idle` from a peer
    /// that predates handovers, which is the truth on a host that has none.
    #[serde(default)]
    pub handover: crate::HandoverState,
    /// Protocol v14. The model the session asked for and the one last
    /// reported, for a managed session whose adapter can say. `None` from a
    /// raw session, an older peer, or an adapter that reports neither.
    #[serde(default)]
    pub model: Option<crate::ModelIdentity>,
    /// Protocol v16. What created this session. `Manual` from a peer that
    /// predates origins, which is the truth for every session a window made.
    /// A chat's session is never drawn by a window, a tray or a flyout: its
    /// chat is its view.
    #[serde(default)]
    pub origin: crate::SessionOrigin,
    pub created_at_ms: i64,
}

impl SessionSummary {
    /// Controller presence is orthogonal to process lifecycle.
    pub fn with_controller(mut self, controller: ControllerState) -> Self {
        self.controller = controller;
        self
    }

    /// Placement is the project registry's answer, not the runtime's: the
    /// process that owns a PTY is never asked where the folder is.
    pub fn with_project(mut self, project_id: Option<ProjectId>) -> Self {
        self.project_id = project_id;
        self
    }

    pub fn validate(&self) -> Result<(), ErrorCode> {
        match (&self.mode, &self.managed) {
            (SessionMode::RawPty, Some(_)) | (SessionMode::Managed { .. }, None) => {
                Err(ErrorCode::InvalidRequest)
            }
            _ => Ok(()),
        }
    }
}
