// SPDX-License-Identifier: MPL-2.0

//! Open a provider question using the run's existing controller handover.
//! The host still owns the connection and the session's approval/reply UI.

use koru_delegate_core::{Attention, ControlState, RunStatus};
use koru_session_protocol::TakeoverUrgency;

use super::{Command, Request, RunsSurface, Takeover};

impl RunsSurface {
    /// Answer the question that brought the person here. Refresh first: the
    /// canvas's attention marker may have cleared, or the run may have ended.
    /// This only requests cooperative control; it never answers an approval.
    pub fn respond(&mut self) {
        if self.run.is_none() {
            return;
        }
        if self.response_requested {
            return;
        }
        self.response_requested = true;
        self.links.send(Command::Refresh);
    }

    pub(super) fn respond_after_read(&mut self) {
        if !std::mem::take(&mut self.response_requested) {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        if !snapshot.standing().may_act() {
            return;
        }
        let Some(session) = snapshot.session else {
            return;
        };
        let RunStatus::Running { attention, .. } = &snapshot.header.status else {
            return;
        };
        if !attention.contains(Attention::ApprovalRequired)
            && !attention.contains(Attention::WaitingForInput)
        {
            return;
        }
        match (self.takeover, snapshot.control()) {
            (Takeover::Held, _) | (_, ControlState::Human { .. }) => {
                self.requests.push(Request::OpenSession { session });
            }
            (Takeover::Asked | Takeover::Released, _)
            | (_, ControlState::HandoverPending { .. }) => {}
            (Takeover::Idle, ControlState::Orchestrator) => {
                // ADR-389: an identified pending approval is a cooperative
                // boundary. Immediate takeover would cancel the active turn.
                self.ask_for_session(TakeoverUrgency::AtTurnBoundary);
            }
        }
    }
}
