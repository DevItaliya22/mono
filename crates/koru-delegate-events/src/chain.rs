// SPDX-License-Identifier: MPL-2.0

//! One run's chain: the subject every row shares, the root every row
//! correlates to, and the actor rule.

use koru_delegate_core::{ActorRef, DocumentId, RunId, WorkspaceId};
use koru_events::{
    Actor, ActorId, ActorKind, AttrValue, Draft, EmitContext, Envelope, EventId, EventName,
    Subject, validate,
};
use uuid::Uuid;

/// The actor token an orchestrator fact carries. A registered lowercase name
/// token, as `Actor::agent` requires; the run itself is the actor's durable
/// identity (`Actor::id`), so a flow rule can say "if the actor was this run".
pub const ORCHESTRATOR_ACTOR_TOKEN: &str = "delegate";

/// The token for a fact an agent session requested on its own behalf — a
/// delegation asked for over MCP, say. The session is the identity.
const AGENT_SESSION_ACTOR_TOKEN: &str = "agent";

/// What every row of one run shares: the run, the document it is stored in
/// (the subject, §7.5), and the workspace it was delegated from (the
/// envelope's scope).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunRef {
    pub run: RunId,
    /// The manifest id the store minted for the Run document (`koru-run`).
    /// The subject, never the `RunId`: a run is addressed as the document
    /// it is.
    pub document: DocumentId,
    pub workspace: WorkspaceId,
}

/// The identity and the clock of one publication, supplied by the caller —
/// which is what makes retry identity a caller's decision rather than a
/// builder's accident (§7.1). A retry passes the stamp it stored; an
/// amendment passes [`Stamp::fresh`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub event_id: EventId,
    /// Wall clock, epoch ms UTC. Display and bucketing only; the journal's
    /// `seq` orders.
    pub occurred_at_ms: i64,
}

impl Stamp {
    /// A newly minted identity — the shipping path for a first publication
    /// and for every usage amendment (§17.2: a correction gets its own id).
    pub fn fresh(occurred_at_ms: i64) -> Self {
        Self {
            event_id: EventId::mint(),
            occurred_at_ms,
        }
    }
}

/// The actor rule, spelled once. A person's request is a person; the
/// orchestrator acting for a run is an agent whose identity is the run; an
/// agent session acting for itself is an agent whose identity is the session.
pub fn actor_for(actor: ActorRef) -> Actor {
    match actor {
        ActorRef::Person { .. } => Actor::user(),
        ActorRef::Orchestrator { run } => orchestrator(run),
        ActorRef::Agent { session } => Actor {
            kind: ActorKind::Agent,
            id: Some(ActorId(session.as_uuid())),
            token: Some(AGENT_SESSION_ACTOR_TOKEN.to_string()),
        },
    }
}

/// The orchestrator, as the actor of the run it pursues.
pub(crate) fn orchestrator(run: RunId) -> Actor {
    Actor {
        kind: ActorKind::Agent,
        id: Some(ActorId(run.as_uuid())),
        token: Some(ORCHESTRATOR_ACTOR_TOKEN.to_string()),
    }
}

/// One run's chain of events. Constructed by building `delegate.run.created`
/// ([`RunChain::created`]) or by rejoining an existing chain after a restart
/// ([`RunChain::rejoin`]); every builder on it threads the root as
/// `correlation_id` and the Run document as the subject.
#[derive(Clone, Debug)]
pub struct RunChain {
    ctx: EmitContext,
    run: RunRef,
    root: EventId,
}

impl RunChain {
    /// Rejoin a chain whose root was published earlier — on a restart, or
    /// in a process that did not create the run. `created` is the `EventId`
    /// of `delegate.run.created`, which `runs.db` keeps.
    pub fn rejoin(ctx: &EmitContext, run: RunRef, created: EventId) -> Self {
        Self {
            ctx: ctx.clone(),
            run,
            root: created,
        }
    }

    /// The `EventId` of `delegate.run.created` — what every row of this run
    /// carries as `correlation_id`, and what a restart hands
    /// [`RunChain::rejoin`].
    pub fn root(&self) -> EventId {
        self.root
    }

    pub fn run(&self) -> &RunRef {
        &self.run
    }

    /// The orchestrator, as this run's actor.
    pub(crate) fn orchestrator(&self) -> Actor {
        orchestrator(self.run.run)
    }

    /// A draft of one row of this chain: the subject, the scope, the `run`
    /// attribute and the correlation, so a builder adds only what its row
    /// says. `Self::created` is the one row built without this, because it
    /// is the root and carries no correlation.
    pub(crate) fn row(&self, name: EventName, actor: Actor) -> Draft {
        subject_draft(name, &self.run)
            .actor(actor)
            .in_chain(self.root)
    }

    /// Assemble and check. The registry is the boundary's authority and every
    /// builder is held to it by test; the assertion is the belt beside those
    /// braces, and it costs nothing in a release build.
    pub(crate) fn finish(&self, draft: Draft, stamp: Stamp) -> Envelope {
        finish(&self.ctx, draft, stamp)
    }

    /// Build the root: `delegate.run.created`. Consumes nothing the caller
    /// keeps; returns the envelope to publish and the chain every later row
    /// joins. The root's `event_id` is `stamp.event_id`.
    pub(crate) fn root_row(
        ctx: &EmitContext,
        run: RunRef,
        stamp: Stamp,
        draft: Draft,
    ) -> (Envelope, Self) {
        let envelope = finish(ctx, draft, stamp);
        let chain = Self {
            ctx: ctx.clone(),
            run,
            root: envelope.event_id,
        };
        (envelope, chain)
    }
}

/// A draft addressed at one run: the Run document as the subject, the
/// workspace as the scope, and the `run` attribute every row carries.
pub(crate) fn subject_draft(name: EventName, run: &RunRef) -> Draft {
    Draft::new(name)
        .subject(Subject::document(run.document.to_storage()))
        .workspace(koru_events::WorkspaceId(run.workspace.as_uuid()))
        .attr("run", id(run.run.as_uuid()))
}

pub(crate) fn finish(ctx: &EmitContext, draft: Draft, stamp: Stamp) -> Envelope {
    let envelope = draft.build(ctx, stamp.event_id, stamp.occurred_at_ms);
    debug_assert!(
        validate(&envelope).is_ok(),
        "a delegate builder produced an envelope the registry refuses: {:?}",
        validate(&envelope).err()
    );
    envelope
}

// ------------------------------------------------------------ attr values
//
// The eight `AttrValue` kinds a `delegate.*` row uses, as functions rather
// than a trait: a `u32` count and a `u64` count are both `Count`, and an
// `Option` is an *absent* attribute — never a zero (§17.2).

pub(crate) fn id(value: Uuid) -> AttrValue {
    AttrValue::Id(value)
}

pub(crate) fn count(value: impl Into<u64>) -> AttrValue {
    AttrValue::Count(value.into())
}

pub(crate) fn token(value: &'static str) -> AttrValue {
    AttrValue::Token(value.to_string())
}

pub(crate) fn flag(value: bool) -> AttrValue {
    AttrValue::Flag(value)
}

pub(crate) fn int(value: i64) -> AttrValue {
    AttrValue::Int(value)
}

/// An attribute that is present only when its value is known.
pub(crate) trait DraftExt {
    fn attr_if(self, key: &str, value: Option<AttrValue>) -> Self;
}

impl DraftExt for Draft {
    fn attr_if(self, key: &str, value: Option<AttrValue>) -> Self {
        match value {
            Some(value) => self.attr(key, value),
            None => self,
        }
    }
}
