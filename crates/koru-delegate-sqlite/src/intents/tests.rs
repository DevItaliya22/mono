// SPDX-License-Identifier: MPL-2.0

//! §7.1 steps 1 and 4: a request is not a run, a duplicate is one row, and
//! the payload never leaves the file except as the request it was.

use koru_delegate_core::Objective;

use super::*;
use crate::fixtures::{
    FOLDER_SENTINEL, OBJECTIVE_SENTINEL, Root, document, request, run_id, workspace,
};

const WS: u8 = 0xa2;

#[test]
fn an_intent_is_pending_until_satisfied_and_listed_as_a_request_meanwhile() {
    let root = Root::new();
    let mut db = root.open();
    assert_eq!(
        db.record_intent(&request(1), workspace(WS)).unwrap(),
        WriteOutcome::Applied
    );
    let record = db.intent(run_id(1)).unwrap().unwrap();
    assert_eq!(record.state, IntentState::Pending);
    assert_eq!(record.workspace, workspace(WS));
    assert_eq!(record.run_document, None);
    assert_eq!(record.created_at_ms, root.now_ms());
    let pending = db.pending_intents().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].payload, request(1));

    root.tick(500);
    assert_eq!(
        db.satisfy_intent(run_id(1), document(9)).unwrap(),
        WriteOutcome::Applied
    );
    let record = db.intent(run_id(1)).unwrap().unwrap();
    assert_eq!(record.state, IntentState::Satisfied);
    assert_eq!(record.run_document, Some(document(9)));
    assert_eq!(record.updated_at_ms, record.created_at_ms + 500);
    assert!(
        db.pending_intents().unwrap().is_empty(),
        "a run is not a request"
    );
}

#[test]
fn pending_intents_list_oldest_first() {
    let root = Root::new();
    let mut db = root.open();
    db.record_intent(&request(2), workspace(WS)).unwrap();
    root.tick(1);
    db.record_intent(&request(1), workspace(WS)).unwrap();
    let runs: Vec<_> = db
        .pending_intents()
        .unwrap()
        .into_iter()
        .map(|p| p.run)
        .collect();
    assert_eq!(runs, vec![run_id(2), run_id(1)]);
}

/// A duplicate request for a known run: the same payload is idempotent,
/// a different payload or workspace is refused, and there is never a
/// second row.
#[test]
fn a_duplicate_request_is_idempotent_or_refused_never_a_second_row() {
    let root = Root::new();
    let mut db = root.open();
    db.record_intent(&request(1), workspace(WS)).unwrap();
    assert_eq!(
        db.record_intent(&request(1), workspace(WS)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    let mut changed = request(1);
    changed.objective = Objective::new("something else entirely").unwrap();
    assert_eq!(
        db.record_intent(&changed, workspace(WS)),
        Err(LedgerError::IntentConflict)
    );
    assert_eq!(
        db.record_intent(&request(1), workspace(WS + 1)),
        Err(LedgerError::IntentConflict)
    );
    assert_eq!(db.pending_intents().unwrap().len(), 1);
    assert_eq!(
        db.pending_intents().unwrap()[0].payload,
        request(1),
        "the first request stands"
    );
    // Still idempotent once the run exists.
    db.satisfy_intent(run_id(1), document(9)).unwrap();
    assert_eq!(
        db.record_intent(&request(1), workspace(WS)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
}

#[test]
fn satisfy_and_abandon_follow_the_closed_domain() {
    let root = Root::new();
    let mut db = root.open();
    assert_eq!(
        db.satisfy_intent(run_id(1), document(9)),
        Err(LedgerError::UnknownRun)
    );
    assert_eq!(db.abandon_intent(run_id(1)), Err(LedgerError::UnknownRun));

    db.record_intent(&request(1), workspace(WS)).unwrap();
    db.record_intent(&request(2), workspace(WS)).unwrap();

    // Abandoned: idempotent, and never satisfied afterwards.
    assert_eq!(db.abandon_intent(run_id(1)).unwrap(), WriteOutcome::Applied);
    assert_eq!(
        db.abandon_intent(run_id(1)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(
        db.satisfy_intent(run_id(1), document(9)),
        Err(LedgerError::IntentAbandoned)
    );
    assert_eq!(
        db.intent(run_id(1)).unwrap().unwrap().state,
        IntentState::Abandoned
    );

    // Satisfied: idempotent on the same document, a conflict on another,
    // and never abandoned afterwards (D31).
    db.satisfy_intent(run_id(2), document(9)).unwrap();
    assert_eq!(
        db.satisfy_intent(run_id(2), document(9)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(
        db.satisfy_intent(run_id(2), document(10)),
        Err(LedgerError::IntentConflict)
    );
    assert_eq!(db.abandon_intent(run_id(2)), Err(LedgerError::RunExists));
    assert!(db.pending_intents().unwrap().is_empty());
}

/// Standing requirement 7, structurally: the objective and the folder are
/// in the file and in nothing this crate renders or refuses.
#[test]
fn the_payload_is_stored_and_never_rendered() {
    let root = Root::new();
    let mut db = root.open();
    db.record_intent(&request(1), workspace(WS)).unwrap();
    let stored: String = db
        .conn()
        .query_row("SELECT payload_json FROM run_intents", [], |row| row.get(0))
        .unwrap();
    assert!(
        stored.contains(OBJECTIVE_SENTINEL),
        "it is what a restart completes from"
    );
    assert!(stored.contains(FOLDER_SENTINEL));

    let mut changed = request(1);
    changed.objective = Objective::new("another objective").unwrap();
    let renderings = [
        format!("{:?}", db.record_intent(&changed, workspace(WS))),
        format!("{}", db.record_intent(&changed, workspace(WS)).unwrap_err()),
        format!("{:?}", db.intent(run_id(1))),
        format!("{:?}", db.pending_intents()),
        format!("{:#?}", db.pending_intents().unwrap()[0]),
        format!("{db:?}"),
    ];
    for rendered in renderings {
        assert!(!rendered.contains(OBJECTIVE_SENTINEL), "{rendered}");
        assert!(!rendered.contains(FOLDER_SENTINEL), "{rendered}");
        assert!(!rendered.contains("another objective"), "{rendered}");
    }
}

#[test]
fn a_model_selection_survives_creation_retry_and_conflicts_with_a_changed_choice() {
    let mut db = crate::RunsDb::open(std::path::Path::new(":memory:")).unwrap();
    let mut chosen = request(1);
    chosen.requested_model =
        Some(koru_delegate_core::ModelName::new("deepseek/deepseek-chat").unwrap());
    assert_eq!(
        db.record_intent(&chosen, workspace(WS)).unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        db.pending_intents().unwrap()[0].payload.requested_model,
        chosen.requested_model
    );
    assert_eq!(
        db.record_intent(&chosen, workspace(WS)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    let mut changed = chosen.clone();
    changed.requested_model = None;
    assert_eq!(
        db.record_intent(&changed, workspace(WS)),
        Err(LedgerError::IntentConflict)
    );
    assert_eq!(db.pending_intents().unwrap()[0].payload, chosen);
}
