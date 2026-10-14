// SPDX-License-Identifier: MPL-2.0

//! The run's lifecycle rows: created, started, interrupted, resumed, and the
//! three terminal ones.

use koru_delegate_core::{
    ActorRef, AttemptGeneration, CancelReason, FailureStage, PriorAttempt, ResumeSeam,
    RunAttemptId, SessionId,
};
use koru_events::{EmitContext, Envelope, event_name};

use crate::chain::{DraftExt, RunChain, RunRef, Stamp, count, id, subject_draft, token};
use crate::tokens::{CompletionOutcome, DelegatedAgent, RowToken, ScopeKind};

/// The Run document was committed — the moment a run exists (§7.1 step 3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunCreated {
    /// Who asked: a person, or the orchestrator for a scheduled firing.
    pub requested_by: ActorRef,
    pub agent: DelegatedAgent,
    pub scope: ScopeKind,
    /// How many context items were pinned. Never which.
    pub context_items: u32,
}

/// An attempt's provider is up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunStarted {
    pub attempt: RunAttemptId,
    pub generation: AttemptGeneration,
    pub session: SessionId,
}

/// A restart ended the attempt (§7.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunInterrupted {
    pub attempt: RunAttemptId,
    pub generation: AttemptGeneration,
    pub prior_attempt: PriorAttempt,
}

/// A fresh attempt continued the run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunResumed {
    pub attempt: RunAttemptId,
    pub generation: AttemptGeneration,
    pub via: ResumeSeam,
}

/// The run reached `Completed`. The outcome token is D29's rule applied to
/// `artifacts`, not a second thing to get right.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunCompleted {
    pub attempt: RunAttemptId,
    pub generation: AttemptGeneration,
    /// Artifact versions the run registered in all.
    pub artifacts: u32,
}

/// The run reached `Failed`. `attempt` is `None` when the failure came
/// before any attempt existed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunFailed {
    pub attempt: Option<RunAttemptId>,
    pub generation: AttemptGeneration,
    pub stage: FailureStage,
}

/// A cancellation. Only `CancelReason::Run` is terminal (D36).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunCancelled {
    pub attempt: Option<RunAttemptId>,
    pub generation: AttemptGeneration,
    pub reason: CancelReason,
}

impl RunChain {
    /// `delegate.run.created` — the chain root. Returns the envelope and the
    /// chain every later row joins; the root's `event_id` is `stamp.event_id`,
    /// which `runs.db` keeps so a restart can [`RunChain::rejoin`].
    pub fn created(
        ctx: &EmitContext,
        run: RunRef,
        stamp: Stamp,
        fact: RunCreated,
    ) -> (Envelope, Self) {
        let draft = subject_draft(event_name!("delegate.run.created"), &run)
            .actor(crate::chain::actor_for(fact.requested_by))
            .attr("workspace", id(run.workspace.as_uuid()))
            .attr("agent", token(fact.agent.token()))
            .attr("scope_kind", token(fact.scope.token()))
            .attr("context_items", count(fact.context_items));
        Self::root_row(ctx, run, stamp, draft)
    }

    /// `delegate.run.started`.
    pub fn started(&self, stamp: Stamp, fact: RunStarted) -> Envelope {
        let draft = self
            .row(event_name!("delegate.run.started"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("generation", count(fact.generation.get()))
            .attr("session", id(fact.session.as_uuid()));
        self.finish(draft, stamp)
    }

    /// `delegate.run.interrupted`.
    pub fn interrupted(&self, stamp: Stamp, fact: RunInterrupted) -> Envelope {
        let draft = self
            .row(event_name!("delegate.run.interrupted"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("generation", count(fact.generation.get()))
            .attr("prior_attempt", token(fact.prior_attempt.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.run.resumed`.
    pub fn resumed(&self, stamp: Stamp, fact: RunResumed) -> Envelope {
        let draft = self
            .row(event_name!("delegate.run.resumed"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("generation", count(fact.generation.get()))
            .attr("via", token(fact.via.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.run.completed`.
    pub fn completed(&self, stamp: Stamp, fact: RunCompleted) -> Envelope {
        let draft = self
            .row(event_name!("delegate.run.completed"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("generation", count(fact.generation.get()))
            .attr(
                "outcome",
                token(CompletionOutcome::of(fact.artifacts).token()),
            )
            .attr("artifacts", count(fact.artifacts));
        self.finish(draft, stamp)
    }

    /// `delegate.run.failed`.
    pub fn failed(&self, stamp: Stamp, fact: RunFailed) -> Envelope {
        let draft = self
            .row(event_name!("delegate.run.failed"), self.orchestrator())
            .attr_if("attempt", fact.attempt.map(|attempt| id(attempt.as_uuid())))
            .attr("generation", count(fact.generation.get()))
            .attr("stage", token(fact.stage.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.run.cancelled`. The actor is the person, whichever the
    /// reason: a job ended or a step ended for a takeover are both theirs.
    pub fn cancelled(&self, stamp: Stamp, fact: RunCancelled) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.run.cancelled"),
                koru_events::Actor::user(),
            )
            .attr_if("attempt", fact.attempt.map(|attempt| id(attempt.as_uuid())))
            .attr("generation", count(fact.generation.get()))
            .attr("reason", token(fact.reason.token()));
        self.finish(draft, stamp)
    }
}
