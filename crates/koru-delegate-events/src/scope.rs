// SPDX-License-Identifier: MPL-2.0

//! The busy-scope rows (D34, P1.5): a manual start that proceeded onto a
//! held folder, and a scheduled firing that was skipped because of one.

use koru_delegate_core::{DocumentId, RunAttemptId, RunId, ScheduleId, WorkspaceId};
use koru_events::{Draft, EmitContext, Envelope, Subject, event_name};

use crate::chain::{RunChain, Stamp, count, finish, id, orchestrator, token};
use crate::tokens::{RowToken, ScopeOverlap};

/// A manual start proceeded onto a busy folder: the person was shown who
/// holds it and went ahead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScopeBusy {
    /// The earliest overlapping claim — the holder the person was shown.
    pub holder: RunId,
    pub holder_attempt: RunAttemptId,
    pub overlap: ScopeOverlap,
    /// How many further runs also hold an overlapping folder.
    pub others: u32,
}

/// A scheduled firing was skipped because its folder was busy. **No run
/// exists**: nothing was minted, so this is built outside a [`RunChain`]. M2's
/// schedule product fact uses its separate [`crate::ScheduleChain`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScopeSkipped {
    pub workspace: WorkspaceId,
    pub schedule: ScheduleId,
    /// The schedule's own document, when one exists (M2) — the subject.
    /// `None` in M1, where a schedule has no document yet.
    pub schedule_document: Option<DocumentId>,
    pub holder: RunId,
    pub holder_attempt: RunAttemptId,
    pub overlap: ScopeOverlap,
    pub others: u32,
}

impl RunChain {
    /// `delegate.scope.busy`. The actor is the orchestrator: the fact is
    /// that the start proceeded, which is the orchestrator's act, and the
    /// person's decision to proceed is the manual start itself.
    pub fn scope_busy(&self, stamp: Stamp, fact: ScopeBusy) -> Envelope {
        let draft = self
            .row(event_name!("delegate.scope.busy"), self.orchestrator())
            .attr("holder", id(fact.holder.as_uuid()))
            .attr("holder_attempt", id(fact.holder_attempt.as_uuid()))
            .attr("overlap", token(fact.overlap.token()))
            .attr("others", count(fact.others));
        self.finish(draft, stamp)
    }
}

/// `delegate.scope.skipped` — its own chain root, with the schedule as the
/// only identity; the actor is the orchestrator for the run that holds the
/// folder, because that is the only run there is.
pub fn scope_skipped(ctx: &EmitContext, stamp: Stamp, fact: ScopeSkipped) -> Envelope {
    let mut draft = Draft::new(event_name!("delegate.scope.skipped"))
        .actor(orchestrator(fact.holder))
        .workspace(koru_events::WorkspaceId(fact.workspace.as_uuid()))
        .attr("schedule", id(fact.schedule.as_uuid()))
        .attr("holder", id(fact.holder.as_uuid()))
        .attr("holder_attempt", id(fact.holder_attempt.as_uuid()))
        .attr("overlap", token(fact.overlap.token()))
        .attr("others", count(fact.others));
    if let Some(document) = fact.schedule_document {
        draft = draft.subject(Subject::document(document.to_storage()));
    }
    finish(ctx, draft, stamp)
}
