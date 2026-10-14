// SPDX-License-Identifier: MPL-2.0

//! The artifact rows: a version registered by the run, and an earlier
//! result handed to a run as input.

use koru_artifact_core::ArtifactFamilyId;
use koru_delegate_core::{AgentTurnId, ArtifactVersionId, RunAttemptId};
use koru_events::{Envelope, event_name};

use crate::chain::{DraftExt, RunChain, Stamp, count, flag, id};

/// An artifact version was registered (P1.10). Ids and an ordinal — never
/// the title, the filename or the bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArtifactRegistered {
    pub attempt: RunAttemptId,
    /// The turn during which it was registered, when one was active.
    pub turn: Option<AgentTurnId>,
    pub artifact: ArtifactVersionId,
    pub family: ArtifactFamilyId,
    pub ordinal: u32,
    /// Whether the version declares a `derived_from` provenance.
    pub derived: bool,
}

/// An earlier result was handed to this run as input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArtifactAttached {
    pub artifact: ArtifactVersionId,
}

impl RunChain {
    /// `delegate.artifact.registered`.
    pub fn artifact_registered(&self, stamp: Stamp, fact: ArtifactRegistered) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.artifact.registered"),
                self.orchestrator(),
            )
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr_if("turn", fact.turn.map(|turn| id(turn.as_uuid())))
            .attr("artifact", id(fact.artifact.as_uuid()))
            .attr("family", id(fact.family.as_uuid()))
            .attr("ordinal", count(fact.ordinal))
            .attr("derived", flag(fact.derived));
        self.finish(draft, stamp)
    }

    /// `delegate.artifact.attached`.
    pub fn artifact_attached(&self, stamp: Stamp, fact: ArtifactAttached) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.artifact.attached"),
                self.orchestrator(),
            )
            .attr("artifact", id(fact.artifact.as_uuid()));
        self.finish(draft, stamp)
    }
}
