// SPDX-License-Identifier: MPL-2.0

//! Every closed domain is refused twice: by the database and by the type.

use koru_delegate_core::PublicationKind;

use super::*;
use crate::fixtures::Root;

#[test]
fn every_token_round_trips_and_no_stranger_parses() {
    for state in [
        IntentState::Pending,
        IntentState::Satisfied,
        IntentState::Abandoned,
    ] {
        assert_eq!(IntentState::parse(state.token()).unwrap(), state);
    }
    for phase in AttemptPhase::ALL {
        assert_eq!(AttemptPhase::parse(phase.token()).unwrap(), phase);
    }
    for outcome in SlotOutcome::ALL {
        assert_eq!(
            SlotState::parse(outcome.token()).unwrap(),
            SlotState::Settled(outcome)
        );
    }
    assert_eq!(SlotState::parse("claimed").unwrap(), SlotState::Claimed);
    for kind in [
        PublicationKind::Lifecycle,
        PublicationKind::ArtifactRegistration,
        PublicationKind::Usage,
    ] {
        assert_eq!(
            parse_publication_kind(publication_token(kind)).unwrap(),
            kind
        );
    }
    assert_eq!(
        publication_token(PublicationKind::Usage),
        "usage",
        "the DDL spells the kind the way the vocabulary crate serializes it"
    );
    assert_eq!(
        IntentState::parse("finished").unwrap_err(),
        LedgerError::Corrupt {
            table: "run_intents",
            column: "state"
        }
    );
    assert_eq!(
        AttemptPhase::parse("failed").unwrap_err().code(),
        "corrupt",
        "there is no failed attempt (D10)"
    );
    assert!(SlotState::parse("fired!").is_err());
    assert!(parse_publication_kind("economics").is_err());
}

#[test]
fn the_attempt_ladder_is_closed() {
    use AttemptPhase::*;
    let allowed = |from: AttemptPhase, to: AttemptPhase| from.may_advance_to(to);
    assert!(allowed(Created, Starting));
    assert!(allowed(Starting, Running));
    assert!(!allowed(Created, Running), "no skipping a rung");
    assert!(!allowed(Running, Starting), "no going back");
    for from in [Created, Starting, Running] {
        for to in [Interrupted, Ended, Unknown] {
            assert!(allowed(from, to), "{from:?} -> {to:?}");
        }
        assert!(!allowed(from, from), "staying put is not a transition");
    }
    for terminal in [Interrupted, Ended, Unknown] {
        assert!(terminal.is_terminal());
        for to in AttemptPhase::ALL {
            assert!(!allowed(terminal, to), "{terminal:?} accepts nothing");
        }
    }
}

/// The database refuses a value outside a closed domain even when the Rust
/// side is bypassed: `'finished'` as an intent state, `'failed'` as an
/// attempt state, a satisfied intent with no document.
#[test]
fn a_value_outside_a_closed_domain_is_refused_by_the_database() {
    let root = Root::new();
    let db = root.open();
    let insert = |sql: &str| db.conn().execute(sql, []);
    assert!(
        insert(
            "INSERT INTO run_intents (run_id, workspace_id, payload_json, state, created_at_ms, updated_at_ms)
             VALUES ('r', 'ws', '{}', 'finished', 1, 1)"
        )
        .is_err(),
        "'finished' is not in the intent domain"
    );
    assert!(
        insert(
            "INSERT INTO run_intents (run_id, workspace_id, payload_json, state, created_at_ms, updated_at_ms)
             VALUES ('r', 'ws', '{}', 'satisfied', 1, 1)"
        )
        .is_err(),
        "satisfied without a document"
    );
    insert(
        "INSERT INTO run_intents (run_id, workspace_id, payload_json, state, run_document_id, created_at_ms, updated_at_ms)
         VALUES ('r', 'ws', '{}', 'satisfied', 'doc', 1, 1)",
    )
    .unwrap();
    assert!(
        insert(
            "INSERT INTO run_attempts (run_id, generation, attempt_id, state, updated_at_ms)
             VALUES ('r', 1, 'a', 'failed', 1)"
        )
        .is_err(),
        "'failed' is not an attempt state (D10)"
    );
    assert!(
        insert(
            "INSERT INTO run_attempts (run_id, generation, attempt_id, state, updated_at_ms)
             VALUES ('r', 0, 'a', 'created', 1)"
        )
        .is_err(),
        "generation zero was never minted"
    );
    assert!(
        insert(
            "INSERT INTO run_attempts (run_id, generation, attempt_id, state, updated_at_ms)
             VALUES ('unknown-run', 1, 'a', 'created', 1)"
        )
        .is_err(),
        "an attempt belongs to a recorded run"
    );
    insert(
        "INSERT INTO run_attempts (run_id, generation, attempt_id, state, updated_at_ms)
         VALUES ('r', 1, 'a', 'interrupted', 1)",
    )
    .unwrap();
    assert!(
        insert(
            "INSERT INTO pending_publications (event_id, run_id, kind, body_json, queued_at_ms)
             VALUES ('e', 'r', 'economics', '{}', 1)"
        )
        .is_err(),
        "the kind domain is the vocabulary crate's spelling"
    );
    assert!(
        insert(
            "INSERT INTO projection_cursors (run_id, projection, timeline_index, updated_at_ms)
             VALUES ('r', 'search', 0, 1)"
        )
        .is_err()
    );
    assert!(
        insert(
            "INSERT INTO schedule_claims (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms)
             VALUES ('s', 1, 0, 'x', 'claimed', 1)"
        )
        .is_err(),
        "revision starts at one"
    );
}
