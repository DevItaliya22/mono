// SPDX-License-Identifier: MPL-2.0

//! A schedule's local activity chain.
//!
//! The schedule document is the subject and its stable [`ScheduleId`] is a
//! typed local-reference attribute on every row. `created` is the chain root;
//! retry storage can later rejoin that root without teaching this leaf crate
//! how anything is stored or published.

use koru_delegate_core::{ActorRef, DocumentId, RunId, ScheduleId, WorkspaceId};
use koru_events::{Actor, Draft, EmitContext, Envelope, EventId, EventName, Subject, event_name};

pub use koru_delegate_core::{
    ScheduleContextFailureStage, ScheduleFiringOutcome, SchedulePauseReason,
};

use crate::chain::{DraftExt, Stamp, actor_for, count, finish, id, orchestrator, token};
use crate::tokens::RowToken;

const WORKER_ACTOR_TOKEN: &str = "delegate-worker";

/// The identities every event in one schedule's chain shares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleRef {
    pub schedule: ScheduleId,
    /// The manifest id of the `koru-schedule` document: the event subject.
    pub document: DocumentId,
    pub workspace: WorkspaceId,
}

/// The Schedule document was committed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleCreated {
    pub requested_by: ActorRef,
    pub revision: u32,
}

/// A person committed a new active definition or resumed the schedule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleUpdated {
    pub revision: u32,
}

/// A committed transition to the paused lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SchedulePaused {
    pub revision: u32,
    pub reason: SchedulePauseReason,
}

/// One occurrence created a real run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleFired {
    pub revision: u32,
    pub occurrence_ms: i64,
    pub run: RunId,
}

/// Why one occurrence produced no run. This wrapper carries the canonical
/// schedule outcome while keeping its holder invariant structural: a missed
/// slot has no holder and a busy slot always has one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ScheduleSkipReason {
    outcome: ScheduleFiringOutcome,
    holder: Option<RunId>,
}

impl ScheduleSkipReason {
    pub fn missed() -> Self {
        Self {
            outcome: ScheduleFiringOutcome::SkippedMissed,
            holder: None,
        }
    }

    pub fn busy(holder: RunId) -> Self {
        Self {
            outcome: ScheduleFiringOutcome::SkippedBusy,
            holder: Some(holder),
        }
    }

    /// Convert the document's canonical outcome at the event boundary.
    /// `None` refuses a fired slot and any impossible reason/holder pair.
    pub fn from_outcome(outcome: ScheduleFiringOutcome, holder: Option<RunId>) -> Option<Self> {
        match (outcome, holder) {
            (ScheduleFiringOutcome::SkippedMissed, None) => Some(Self::missed()),
            (ScheduleFiringOutcome::SkippedBusy, Some(holder)) => Some(Self::busy(holder)),
            (ScheduleFiringOutcome::Fired, _)
            | (ScheduleFiringOutcome::SkippedMissed, Some(_))
            | (ScheduleFiringOutcome::SkippedBusy, None) => None,
        }
    }

    pub fn outcome(self) -> ScheduleFiringOutcome {
        self.outcome
    }

    pub fn holder(self) -> Option<RunId> {
        self.holder
    }
}

/// One occurrence was durably recorded without creating a run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleSkipped {
    pub revision: u32,
    pub occurrence_ms: i64,
    pub reason: ScheduleSkipReason,
}

impl RowToken for ScheduleContextFailureStage {
    fn token(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Commit => "commit",
            Self::Retention => "retention",
            Self::Materialize => "materialize",
        }
    }
}

impl RowToken for SchedulePauseReason {
    fn token(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::ProviderNotDelegatable => "provider_not_delegatable",
            Self::ScopeUnavailable => "scope_unavailable",
            Self::ContextUnavailable { .. } => "context_unavailable",
        }
    }
}

impl RowToken for ScheduleSkipReason {
    fn token(self) -> &'static str {
        match self.outcome {
            ScheduleFiringOutcome::SkippedMissed => "missed",
            ScheduleFiringOutcome::SkippedBusy => "busy",
            ScheduleFiringOutcome::Fired => {
                unreachable!("ScheduleSkipReason never contains Fired")
            }
        }
    }
}

/// One schedule's event chain, rooted at `delegate.schedule.created`.
#[derive(Clone, Debug)]
pub struct ScheduleChain {
    ctx: EmitContext,
    schedule: ScheduleRef,
    root: EventId,
}

impl ScheduleChain {
    /// Build `delegate.schedule.created` and retain its identity as the root
    /// every later row joins.
    pub fn created(
        ctx: &EmitContext,
        schedule: ScheduleRef,
        stamp: Stamp,
        fact: ScheduleCreated,
    ) -> (Envelope, Self) {
        let draft = subject_draft(event_name!("delegate.schedule.created"), &schedule)
            .actor(actor_for(fact.requested_by))
            .attr("workspace", id(schedule.workspace.as_uuid()))
            .attr("revision", count(fact.revision));
        let envelope = finish(ctx, draft, stamp);
        let chain = Self {
            ctx: ctx.clone(),
            schedule,
            root: envelope.event_id,
        };
        (envelope, chain)
    }

    /// Rejoin a chain from the durable id of its `created` row.
    pub fn rejoin(ctx: &EmitContext, schedule: ScheduleRef, created: EventId) -> Self {
        Self {
            ctx: ctx.clone(),
            schedule,
            root: created,
        }
    }

    pub fn root(&self) -> EventId {
        self.root
    }

    pub fn schedule(&self) -> &ScheduleRef {
        &self.schedule
    }

    /// `delegate.schedule.updated` — a committed human edit or resume.
    pub fn updated(&self, stamp: Stamp, fact: ScheduleUpdated) -> Envelope {
        let draft = self
            .row(event_name!("delegate.schedule.updated"), Actor::user())
            .attr("revision", count(fact.revision));
        self.finish(draft, stamp)
    }

    /// `delegate.schedule.paused`. The reason fixes the actor: a manual pause
    /// is the person's act; an eligibility failure is the worker's.
    pub fn paused(&self, stamp: Stamp, fact: SchedulePaused) -> Envelope {
        let actor = match fact.reason {
            SchedulePauseReason::Manual => Actor::user(),
            SchedulePauseReason::ProviderNotDelegatable
            | SchedulePauseReason::ScopeUnavailable
            | SchedulePauseReason::ContextUnavailable { .. } => worker(),
        };
        let (context_item, context_stage) = match fact.reason {
            SchedulePauseReason::ContextUnavailable { item, stage } => {
                (Some(count(item)), Some(token(stage.token())))
            }
            _ => (None, None),
        };
        let draft = self
            .row(event_name!("delegate.schedule.paused"), actor)
            .attr("revision", count(fact.revision))
            .attr("reason", token(fact.reason.token()))
            .attr_if("context_item", context_item)
            .attr_if("stage", context_stage);
        self.finish(draft, stamp)
    }

    /// `delegate.schedule.fired` — the real run is the actor and the attr.
    pub fn fired(&self, stamp: Stamp, fact: ScheduleFired) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.schedule.fired"),
                orchestrator(fact.run),
            )
            .attr("revision", count(fact.revision))
            .attr(
                "occurrence",
                koru_events::AttrValue::Time(fact.occurrence_ms),
            )
            .attr("run", id(fact.run.as_uuid()));
        self.finish(draft, stamp)
    }

    /// `delegate.schedule.skipped` — no candidate run exists. A busy holder
    /// is context, not the actor; the worker made and recorded the decision.
    pub fn skipped(&self, stamp: Stamp, fact: ScheduleSkipped) -> Envelope {
        let holder = fact.reason.holder().map(|holder| id(holder.as_uuid()));
        let draft = self
            .row(event_name!("delegate.schedule.skipped"), worker())
            .attr("revision", count(fact.revision))
            .attr(
                "occurrence",
                koru_events::AttrValue::Time(fact.occurrence_ms),
            )
            .attr("reason", token(fact.reason.token()))
            .attr_if("holder", holder);
        self.finish(draft, stamp)
    }

    fn row(&self, name: EventName, actor: Actor) -> Draft {
        subject_draft(name, &self.schedule)
            .actor(actor)
            .in_chain(self.root)
    }

    fn finish(&self, draft: Draft, stamp: Stamp) -> Envelope {
        finish(&self.ctx, draft, stamp)
    }
}

fn subject_draft(name: EventName, schedule: &ScheduleRef) -> Draft {
    Draft::new(name)
        .subject(Subject::document(schedule.document.to_storage()))
        .workspace(koru_events::WorkspaceId(schedule.workspace.as_uuid()))
        .attr("schedule", id(schedule.schedule.as_uuid()))
}

fn worker() -> Actor {
    Actor::system(WORKER_ACTOR_TOKEN)
}
