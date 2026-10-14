// SPDX-License-Identifier: MPL-2.0

//! The actor rule, held per row: an orchestrator fact is the agent whose
//! identity is the run; a human-originated fact is the person.

use koru_events::{ActorId, ActorKind};

use super::*;

fn orchestrator_actor() -> Actor {
    Actor {
        kind: ActorKind::Agent,
        id: Some(ActorId(run_ref().run.as_uuid())),
        token: Some("delegate".to_string()),
    }
}

/// Which rows a person is the actor of, in the fixture `every_row` builds.
/// Everything else is the orchestrator. The list is the rule; a row that
/// moves sides changes this test on purpose.
const HUMAN_ROWS: &[&str] = &[
    "delegate.run.created",       // `requested_by` is a person in the fixture
    "delegate.run.cancelled",     // a person ended the job
    "delegate.turn.failed",       // the fixture's failed turn is a human turn
    "delegate.turn.cancelled",    // a person ended the step
    "delegate.approval.resolved", // the fixture approves
    "delegate.control.taken",     // the lease was granted to a person
    "delegate.run.rated",         // the rater
];

#[test]
fn every_row_carries_the_actor_its_origin_says() {
    for (name, envelope) in every_row() {
        match name {
            "delegate.schedule.created" | "delegate.schedule.updated" => {
                assert_eq!(envelope.actor, Actor::user(), "{name}");
                continue;
            }
            "delegate.schedule.paused" | "delegate.schedule.skipped" => {
                assert_eq!(envelope.actor, Actor::system("delegate-worker"), "{name}");
                continue;
            }
            "delegate.schedule.fired" => {
                assert_eq!(envelope.actor.kind, ActorKind::Agent, "{name}");
                assert_eq!(
                    envelope.actor.id,
                    Some(ActorId(scheduled_run().as_uuid())),
                    "{name}: the actor is the run that exists"
                );
                assert_eq!(envelope.actor.token.as_deref(), Some("delegate"));
                continue;
            }
            _ => {}
        }
        if name == "delegate.scope.skipped" {
            // The holder's orchestrator — the only run there is.
            assert_eq!(envelope.actor.kind, ActorKind::Agent, "{name}");
            assert_eq!(
                envelope.actor.id,
                Some(ActorId(v4(0x11))),
                "{name}: the actor is the holding run"
            );
            continue;
        }
        if HUMAN_ROWS.contains(&name) {
            assert_eq!(envelope.actor, Actor::user(), "{name} is a person's act");
        } else {
            assert_eq!(
                envelope.actor,
                orchestrator_actor(),
                "{name} is the orchestrator's act"
            );
        }
    }
}

#[test]
fn pause_actor_follows_the_closed_reason() {
    let chain = schedule_chain();
    let manual = chain.paused(
        stamp(54),
        SchedulePaused {
            revision: 2,
            reason: SchedulePauseReason::Manual,
        },
    );
    let automatic = chain.paused(
        stamp(55),
        SchedulePaused {
            revision: 2,
            reason: SchedulePauseReason::ScopeUnavailable,
        },
    );
    assert_eq!(manual.actor, Actor::user());
    assert_eq!(automatic.actor, Actor::system("delegate-worker"));
}

/// A turn row's actor follows the originator, not the row.
#[test]
fn a_turn_rows_actor_is_its_originator() {
    let chain = chain();
    let by_orchestrator = chain.turn_started(
        stamp(50),
        TurnStarted {
            turn: orchestrator_turn(),
        },
    );
    let by_person = chain.turn_started(stamp(51), TurnStarted { turn: human_turn() });
    assert_eq!(by_orchestrator.actor, orchestrator_actor());
    assert_eq!(by_person.actor, Actor::user());
    // ...and the token says the same thing, so a surface can read either.
    assert_eq!(
        by_person.attrs.get("originator"),
        Some(&koru_events::AttrValue::Token("human".into()))
    );
}

/// Approved and rejected are a person's; expired is nobody's, so the
/// orchestrator records it.
#[test]
fn an_expired_approval_is_recorded_by_the_orchestrator() {
    let chain = chain();
    let resolved = |outcome| {
        chain.approval_resolved(
            stamp(52),
            ApprovalResolved {
                attempt: attempt(),
                turn: turn_id(),
                approval: approval(),
                outcome,
            },
        )
    };
    assert_eq!(resolved(ApprovalOutcome::Approved).actor, Actor::user());
    assert_eq!(resolved(ApprovalOutcome::Rejected).actor, Actor::user());
    assert_eq!(
        resolved(ApprovalOutcome::Expired).actor,
        orchestrator_actor()
    );
}

/// `actor_for` covers the three ways a run can be asked for.
#[test]
fn actor_for_maps_every_requester() {
    assert_eq!(actor_for(person()), Actor::user());
    let by_run = actor_for(ActorRef::Orchestrator { run: run_ref().run });
    assert_eq!(by_run, orchestrator_actor());
    let by_session = actor_for(ActorRef::Agent { session: session() });
    assert_eq!(by_session.kind, ActorKind::Agent);
    assert_eq!(by_session.id, Some(ActorId(session().as_uuid())));
    assert_eq!(by_session.token.as_deref(), Some("agent"));
}

/// A takeover found after a crash is still the person's.
#[test]
fn a_discovered_takeover_is_still_the_persons() {
    let envelope = chain().control_taken(
        stamp(53),
        ControlTaken {
            attempt: attempt(),
            discovered: true,
        },
    );
    assert_eq!(envelope.actor, Actor::user());
    assert_eq!(
        envelope.attrs.get("discovered"),
        Some(&koru_events::AttrValue::Flag(true))
    );
}
