// SPDX-License-Identifier: MPL-2.0

//! The economics rows (§17): one turn's usage, the run's fold, and the
//! rating — ids, counts, one signed duration, flags and closed tokens, and
//! never the rating's note (D44).

use koru_delegate_core::{ActorRef, EconomicsSummary, Rating, RunRating, TokenTotal, UsageFact};
use koru_events::{Envelope, event_name};
use koru_session_protocol::CachedInputRelationship;

use crate::chain::{DraftExt, RunChain, Stamp, actor_for, count, flag, id, int, token};
use crate::tokens::{RowToken, RunOutcome};

/// One turn's usage: the durable fact as the Run document committed it
/// (§17.1), plus the cached-input relationship the adapter declared, which
/// the fact records only as a reason for incompleteness.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TurnUsage {
    pub fact: UsageFact,
    pub cached_relationship: CachedInputRelationship,
}

/// The run's economics summary, as the header carries it, and the terminal
/// status it was published under.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunUsage {
    pub summary: EconomicsSummary,
    pub outcome: RunOutcome,
}

/// A person rated a finished run. `has_note` is the only thing the event
/// learns about the note.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunRated {
    pub rated_by: ActorRef,
    pub rating: Rating,
    pub has_note: bool,
}

/// From the rating as the Run document holds it. The note's presence is
/// read; the note is not — the sentinel test proves it never reaches the
/// envelope even when this conversion is fed one.
impl From<&RunRating> for RunRated {
    fn from(rating: &RunRating) -> Self {
        Self {
            rated_by: rating.rated_by,
            rating: rating.rating,
            has_note: rating.note.is_some(),
        }
    }
}

/// A total's `known` sum, present only when at least one turn reported it
/// — absent is unavailable, never zero (§17.2).
fn reported(total: TokenTotal, turns_with_usage: u32) -> Option<koru_events::AttrValue> {
    (total.unavailable_turns < turns_with_usage).then(|| count(total.known))
}

impl RunChain {
    /// `delegate.turn.usage`. **The stamp decides retry identity**: a retry
    /// passes the stamp it stored, an amendment passes a fresh one (§17.2).
    pub fn turn_usage(&self, stamp: Stamp, usage: &TurnUsage) -> Envelope {
        let fact = &usage.fact;
        let draft = self
            .row(event_name!("delegate.turn.usage"), self.orchestrator())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("turn", id(fact.turn.as_uuid()))
            .attr("usage", id(fact.identity.as_uuid()))
            .attr("revision", count(fact.revision.0))
            .attr_if("input", fact.input.map(count))
            .attr_if("output", fact.output.map(count))
            .attr_if("cached_input", fact.cached_input.map(count))
            .attr(
                "cached_relationship",
                token(usage.cached_relationship.token()),
            )
            .attr("model", id(fact.model.as_uuid()))
            .attr("completeness", token(fact.completeness.token()));
        self.finish(draft, stamp)
    }

    /// `delegate.run.usage` — the fold, published when the run ends and
    /// re-published under a fresh stamp when a late fact amends it.
    pub fn run_usage(&self, stamp: Stamp, usage: &RunUsage) -> Envelope {
        let summary = &usage.summary;
        let turns = summary.turns_with_usage;
        let draft = self
            .row(event_name!("delegate.run.usage"), self.orchestrator())
            .attr("outcome", token(usage.outcome.token()))
            .attr("orchestrator_turns", count(summary.orchestrator_turns))
            .attr("human_turns", count(summary.human_turns))
            .attr("turns_with_usage", count(turns))
            .attr_if("input", reported(summary.input, turns))
            .attr(
                "input_unavailable_turns",
                count(summary.input.unavailable_turns),
            )
            .attr_if("output", reported(summary.output, turns))
            .attr(
                "output_unavailable_turns",
                count(summary.output.unavailable_turns),
            )
            .attr_if("cached_input", reported(summary.cached_input, turns))
            .attr(
                "cached_input_unavailable_turns",
                count(summary.cached_input.unavailable_turns),
            )
            .attr("tool_calls", count(summary.tool_calls))
            .attr("interventions", count(summary.interventions))
            .attr_if(
                "elapsed_ms",
                summary
                    .elapsed_ms
                    .map(|ms| int(i64::try_from(ms).unwrap_or(i64::MAX))),
            )
            .attr("models", count(summary.models.len() as u64))
            .attr("complete", flag(summary.complete));
        self.finish(draft, stamp)
    }

    /// `delegate.run.rated`. The rater is the actor.
    pub fn run_rated(&self, stamp: Stamp, fact: RunRated) -> Envelope {
        let draft = self
            .row(event_name!("delegate.run.rated"), actor_for(fact.rated_by))
            .attr("rating", token(fact.rating.token()))
            .attr("has_note", flag(fact.has_note));
        self.finish(draft, stamp)
    }
}
