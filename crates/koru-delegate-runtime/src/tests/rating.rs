// SPDX-License-Identifier: MPL-2.0

//! D44: one gesture on a finished run, the note in the run's own content,
//! and nothing of the note anywhere else.

use koru_delegate_core::{ActorRef, Attention, ProfileId, Rating, RatingNote, StepKind};

use super::*;

/// Text no run's own vocabulary carries, so a leak into a fact or a log
/// line is unmistakable.
const NOTE_SENTINEL: &str = "NOTE-SENTINEL it kept editing the wrong file";

fn rater() -> ActorRef {
    ActorRef::Person {
        profile: ProfileId::from_uuid(v4(0xe0)),
    }
}

fn note() -> Option<RatingNote> {
    Some(RatingNote::new(NOTE_SENTINEL).unwrap())
}

#[test]
fn a_finished_run_takes_the_gesture_and_keeps_the_note_in_its_own_content() {
    let mut h = Harness::new("rate-finished");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert!(h.status(run).is_terminal());

    let header = h
        .orchestrator
        .rate(run, Rating::NotGood, note(), rater())
        .unwrap();
    let recorded = header.rating.expect("the run is rated");
    assert_eq!(recorded.rating, Rating::NotGood);
    assert_eq!(
        recorded.note.as_ref().map(|note| note.as_str()),
        Some(NOTE_SENTINEL),
        "the note lives in the run's own content"
    );
    assert_eq!(recorded.rated_by, rater());

    // The history says it was rated, and says nothing else about it.
    assert!(h.steps(run).contains(&StepKind::Rated {
        rating: Rating::NotGood
    }));

    // The fact that travels carries the rating and no byte of the note.
    let facts = h.facts();
    let rated = facts
        .iter()
        .find(|fact| matches!(fact, RunFact::Rated { .. }))
        .expect("a rating fact was published");
    assert!(matches!(
        rated,
        RunFact::Rated {
            rating: Rating::NotGood,
            ..
        }
    ));
    let travelled = serde_json::to_string(&facts).unwrap();
    assert!(
        !travelled.contains(NOTE_SENTINEL),
        "no fact may carry the note"
    );
    assert!(
        !format!("{rated:?}").contains(NOTE_SENTINEL),
        "no Debug may carry the note"
    );
}

#[test]
fn a_rating_without_a_note_is_a_complete_gesture() {
    let mut h = Harness::new("rate-bare");
    let run = h.running(2);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });

    let header = h
        .orchestrator
        .rate(run, Rating::Good, None, rater())
        .unwrap();
    let recorded = header.rating.expect("rated");
    assert_eq!(recorded.rating, Rating::Good);
    assert!(recorded.note.is_none());
}

#[test]
fn the_last_gesture_is_the_rating() {
    let mut h = Harness::new("rate-again");
    let run = h.running(3);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });

    h.orchestrator
        .rate(run, Rating::Good, None, rater())
        .unwrap();
    let header = h
        .orchestrator
        .rate(run, Rating::NotGood, note(), rater())
        .unwrap();
    assert_eq!(header.rating.as_ref().unwrap().rating, Rating::NotGood);
    // Both gestures are in the history: a mind changed is a fact.
    let rated: Vec<_> = h
        .steps(run)
        .into_iter()
        .filter(|kind| matches!(kind, StepKind::Rated { .. }))
        .collect();
    assert_eq!(
        rated,
        vec![
            StepKind::Rated {
                rating: Rating::Good
            },
            StepKind::Rated {
                rating: Rating::NotGood
            }
        ]
    );
}

#[test]
fn a_run_that_finished_needing_attention_is_rateable_and_a_working_one_is_not() {
    let mut h = Harness::new("rate-needs-attention");
    let run = h.running(4);
    // Still working: there is nothing yet to rate.
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    let refused = h
        .orchestrator
        .rate(run, Rating::Good, None, rater())
        .unwrap_err();
    assert!(matches!(
        refused,
        RunError::WrongStatus { status: "running" }
    ));

    // D29: the turn ended having registered nothing. That is over as far
    // as the person is concerned, and it is exactly the run worth rating.
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert!(
        h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::FinishedNeedingAttention)
    );
    let header = h
        .orchestrator
        .rate(run, Rating::NotGood, None, rater())
        .unwrap();
    assert_eq!(header.rating.unwrap().rating, Rating::NotGood);
}

#[test]
fn rating_is_refused_before_the_run_has_started() {
    let mut h = Harness::new("rate-created");
    let run = run_id(5);
    h.orchestrator
        .create(request(run, h.scope()), workspace())
        .unwrap();
    let refused = h
        .orchestrator
        .rate(run, Rating::Good, None, rater())
        .unwrap_err();
    assert!(matches!(
        refused,
        RunError::WrongStatus { status: "created" }
    ));
}
