// SPDX-License-Identifier: MPL-2.0

//! The execution rows: what a turn did, as §7.5's table records it.

use koru_delegate_core::{
    AgentTurnId, ApprovalId, ApprovalOutcome, AttachmentId, CancelReason, FailureStage,
    RunAttemptId, SessionId, TurnIdentity, TurnOriginator,
};
use koru_events::{Actor, Envelope, event_name};

use crate::chain::{DraftExt, RunChain, Stamp, count, flag, id, token};
use crate::tokens::{AttachmentRejection, RowToken, ToolOutcome};

/// A turn started.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TurnStarted {
    pub turn: TurnIdentity,
}

/// A turn completed — not necessarily the run (§7.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TurnCompleted {
    pub turn: TurnIdentity,
    /// Artifact versions registered during the turn.
    pub artifacts: u32,
}

/// A turn failed, at a stage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TurnFailed {
    pub turn: TurnIdentity,
    pub stage: FailureStage,
}

/// A turn was cancelled, for a reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TurnCancelled {
    pub turn: TurnIdentity,
    pub reason: CancelReason,
}

/// A turn's tool calls, coalesced: one fact per turn at the turn boundary
/// (ADR-346). The outcome token is derived from the two counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ToolsCompleted {
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
    pub calls: u32,
    pub failed: u32,
}

/// The provider asked for an approval.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApprovalRequired {
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
    pub approval: ApprovalId,
}

/// An approval was answered or expired.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApprovalResolved {
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
    pub approval: ApprovalId,
    pub outcome: ApprovalOutcome,
}

/// The provider is waiting for input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputWaiting {
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
}

/// A child session appeared under the run's session (§7.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChildSessionDetected {
    pub attempt: RunAttemptId,
    pub child_session: SessionId,
    pub terminated: bool,
}

/// An attachment the run offered was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AttachmentRejected {
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
    pub attachment: AttachmentId,
    pub reason: AttachmentRejection,
}

/// Facts Koru never received (§7.1). `events_lost` is `None` when the
/// eviction reported no count — unknown, never zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TimelineGap {
    pub attempt: RunAttemptId,
    pub events_lost: Option<u32>,
}

impl RunChain {
    /// The actor of a turn row is its originator: the orchestrator pursuing
    /// the objective, or the person holding control.
    fn originator(&self, originator: TurnOriginator) -> Actor {
        match originator {
            TurnOriginator::Orchestrator => self.orchestrator(),
            TurnOriginator::Human => Actor::user(),
        }
    }

    /// `delegate.turn.started`.
    pub fn turn_started(&self, stamp: Stamp, fact: TurnStarted) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.turn.started"),
                self.originator(fact.turn.originator),
            )
            .attr("attempt", id(fact.turn.attempt.as_uuid()))
            .attr("turn", id(fact.turn.turn.as_uuid()))
            .attr("originator", token(fact.turn.originator.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.turn.completed`.
    pub fn turn_completed(&self, stamp: Stamp, fact: TurnCompleted) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.turn.completed"),
                self.originator(fact.turn.originator),
            )
            .attr("attempt", id(fact.turn.attempt.as_uuid()))
            .attr("turn", id(fact.turn.turn.as_uuid()))
            .attr("originator", token(fact.turn.originator.token()))
            .attr("artifacts", count(fact.artifacts));
        self.finish(draft, stamp)
    }

    /// `delegate.turn.failed`.
    pub fn turn_failed(&self, stamp: Stamp, fact: TurnFailed) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.turn.failed"),
                self.originator(fact.turn.originator),
            )
            .attr("attempt", id(fact.turn.attempt.as_uuid()))
            .attr("turn", id(fact.turn.turn.as_uuid()))
            .attr("originator", token(fact.turn.originator.token()))
            .attr("stage", token(fact.stage.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.turn.cancelled`. The actor is the person who cancelled,
    /// whichever the originator of the turn they ended.
    pub fn turn_cancelled(&self, stamp: Stamp, fact: TurnCancelled) -> Envelope {
        let draft = self
            .row(event_name!("delegate.turn.cancelled"), Actor::user())
            .attr("attempt", id(fact.turn.attempt.as_uuid()))
            .attr("turn", id(fact.turn.turn.as_uuid()))
            .attr("originator", token(fact.turn.originator.token()))
            .attr("reason", token(fact.reason.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.tool.completed` — one per turn, at its boundary (ADR-346).
    pub fn tools_completed(&self, stamp: Stamp, fact: ToolsCompleted) -> Envelope {
        let draft = self
            .row(event_name!("delegate.tool.completed"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("turn", id(fact.turn.as_uuid()))
            .attr("calls", count(fact.calls))
            .attr("failed", count(fact.failed))
            .attr(
                "outcome",
                token(ToolOutcome::of(fact.calls, fact.failed).token()),
            );
        self.finish(draft, stamp)
    }

    /// `delegate.approval.required`.
    pub fn approval_required(&self, stamp: Stamp, fact: ApprovalRequired) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.approval.required"),
                self.orchestrator(),
            )
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("turn", id(fact.turn.as_uuid()))
            .attr("approval", id(fact.approval.as_uuid()));
        self.finish(draft, stamp)
    }

    /// `delegate.approval.resolved`. A person approved or rejected; nobody
    /// acted on an expired one, so the orchestrator records it.
    pub fn approval_resolved(&self, stamp: Stamp, fact: ApprovalResolved) -> Envelope {
        let actor = match fact.outcome {
            ApprovalOutcome::Approved | ApprovalOutcome::Rejected => Actor::user(),
            ApprovalOutcome::Expired => self.orchestrator(),
        };
        let draft = self
            .row(event_name!("delegate.approval.resolved"), actor)
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("turn", id(fact.turn.as_uuid()))
            .attr("approval", id(fact.approval.as_uuid()))
            .attr("outcome", token(fact.outcome.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.input.waiting`.
    pub fn input_waiting(&self, stamp: Stamp, fact: InputWaiting) -> Envelope {
        let draft = self
            .row(event_name!("delegate.input.waiting"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("turn", id(fact.turn.as_uuid()));
        self.finish(draft, stamp)
    }

    /// `delegate.child_session.detected`.
    pub fn child_session_detected(&self, stamp: Stamp, fact: ChildSessionDetected) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.child_session.detected"),
                self.orchestrator(),
            )
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("child_session", id(fact.child_session.as_uuid()))
            .attr("terminated", flag(fact.terminated));
        self.finish(draft, stamp)
    }

    /// `delegate.attachment.rejected`.
    pub fn attachment_rejected(&self, stamp: Stamp, fact: AttachmentRejected) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.attachment.rejected"),
                self.orchestrator(),
            )
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("turn", id(fact.turn.as_uuid()))
            .attr("attachment", id(fact.attachment.as_uuid()))
            .attr("reason", token(fact.reason.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.timeline.gap`.
    pub fn timeline_gap(&self, stamp: Stamp, fact: TimelineGap) -> Envelope {
        let draft = self
            .row(event_name!("delegate.timeline.gap"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr_if("events_lost", fact.events_lost.map(count));
        self.finish(draft, stamp)
    }
}
