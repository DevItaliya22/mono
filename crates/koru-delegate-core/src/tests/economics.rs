// SPDX-License-Identifier: MPL-2.0

//! §17.2: replay selects the highest revision regardless of delivery order,
//! and unavailable is never zero.

use super::*;

fn fact(turn: u8, revision: u32, input: Option<u64>) -> UsageFact {
    UsageFact {
        identity: UsageFactId::from_uuid(v4(0x40 + turn)),
        attempt: attempt_id(0x20),
        turn: turn_id(turn),
        revision: AmendmentRevision(revision),
        input,
        output: Some(10),
        cached_input: None,
        model: ModelRef::from_uuid(v4(0x30)),
        completeness: Completeness::Complete,
    }
}

fn input_of(replayed: &std::collections::BTreeMap<TurnKey, UsageFact>, turn: u8) -> Option<u64> {
    replayed[&fact(turn, 0, None).key()].input
}

#[test]
fn replay_keeps_the_highest_revision_whatever_the_delivery_order() {
    // 100, then a correction to 140, then the original replayed: 140.
    let replayed = replay(&[
        fact(1, 0, Some(100)),
        fact(1, 1, Some(140)),
        fact(1, 0, Some(100)),
    ]);
    assert_eq!(input_of(&replayed, 1), Some(140));

    // A legitimate later correction to 90 wins over both.
    let replayed = replay(&[
        fact(1, 1, Some(140)),
        fact(1, 0, Some(100)),
        fact(1, 2, Some(90)),
        fact(1, 1, Some(140)),
    ]);
    assert_eq!(input_of(&replayed, 1), Some(90));

    // The correction arriving first and the original after: still 140.
    let replayed = replay(&[fact(1, 1, Some(140)), fact(1, 0, Some(100))]);
    assert_eq!(input_of(&replayed, 1), Some(140));
}

#[test]
fn a_retry_repeats_identity_and_revision_and_changes_nothing() {
    let retries: Vec<_> = (0..5).map(|_| fact(1, 0, Some(100))).collect();
    let replayed = replay(&retries);
    assert_eq!(replayed.len(), 1);
    assert_eq!(input_of(&replayed, 1), Some(100));
}

#[test]
fn turns_are_independent_accounting_units() {
    let replayed = replay(&[
        fact(1, 0, Some(100)),
        fact(2, 0, Some(7)),
        fact(1, 1, Some(50)),
    ]);
    assert_eq!(replayed.len(), 2);
    assert_eq!(input_of(&replayed, 1), Some(50));
    assert_eq!(input_of(&replayed, 2), Some(7));

    // The same turn id under a different attempt is a different key.
    let mut other_attempt = fact(1, 5, Some(1));
    other_attempt.attempt = attempt_id(0x21);
    let replayed = replay(&[fact(1, 0, Some(100)), other_attempt]);
    assert_eq!(replayed.len(), 2);
}

#[test]
fn unavailable_is_unavailable_never_zero() {
    let replayed = replay(&[fact(1, 0, Some(100)), fact(2, 0, None)]);
    let summary = EconomicsSummary::project(&replayed, &[], 0);
    assert_eq!(
        summary.input,
        TokenTotal {
            known: 100,
            unavailable_turns: 1
        }
    );
    assert_eq!(summary.input.complete(), None, "one turn did not report");
    assert_eq!(
        summary.output,
        TokenTotal {
            known: 20,
            unavailable_turns: 0
        }
    );
    assert_eq!(summary.output.complete(), Some(20));
    assert_eq!(summary.cached_input.known, 0);
    assert_eq!(summary.cached_input.unavailable_turns, 2);
    assert_eq!(
        summary.cached_input.complete(),
        None,
        "nothing reported is not zero"
    );

    // A genuine zero is a different fact from a missing one.
    let replayed = replay(&[fact(1, 0, Some(0))]);
    let summary = EconomicsSummary::project(&replayed, &[], 0);
    assert_eq!(summary.input.complete(), Some(0));
}

#[test]
fn one_incomplete_fact_makes_the_summary_incomplete() {
    let mut undeclared = fact(2, 0, Some(5));
    undeclared.completeness = Completeness::Incomplete {
        reason: IncompleteReason::CachedInputRelationshipUndeclared,
    };
    let replayed = replay(&[fact(1, 0, Some(100)), undeclared]);
    let summary = EconomicsSummary::project(&replayed, &[], 0);
    assert!(!summary.complete);
    assert_eq!(summary.turns_with_usage, 2);
    assert_eq!(summary.models.len(), 1);

    let summary = EconomicsSummary::project(&replay(&[fact(1, 0, Some(1))]), &[], 0);
    assert!(summary.complete);
}

#[test]
fn a_model_is_a_reference_and_its_metadata_keeps_requested_and_reported_apart() {
    let identity = ModelIdentity {
        id: ModelRef::from_uuid(v4(0x30)),
        provider: "codex".parse().unwrap(),
        requested: ModelReport::Named {
            name: ModelName::new("gpt-5-codex").unwrap(),
        },
        reported: ModelReport::Unknown,
    };
    let json = serde_json::to_value(&identity).unwrap();
    assert_eq!(json["requested"]["name"], "gpt-5-codex");
    assert_eq!(json["reported"]["kind"], "unknown");
    assert!(ModelName::new("").is_err());
    assert!(ModelName::new("has space").is_err());
    assert!(ModelName::new("x".repeat(129)).is_err());
    assert!(ModelName::new("claude-sonnet-4-5").is_ok());
}
