// SPDX-License-Identifier: MPL-2.0

//! A cursor per projection per run, forward only.

use super::*;
use crate::fixtures::{Root, created_run, run_id};

#[test]
fn a_cursor_starts_absent_moves_forward_and_refuses_to_move_back() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    assert_eq!(db.cursor(run, Projection::HeaderSummary).unwrap(), None);
    assert_eq!(
        db.advance_cursor(run, Projection::HeaderSummary, 0)
            .unwrap(),
        WriteOutcome::Applied,
        "zero is a position: nothing folded yet, but the projection has started"
    );
    assert_eq!(db.cursor(run, Projection::HeaderSummary).unwrap(), Some(0));
    assert_eq!(
        db.advance_cursor(run, Projection::HeaderSummary, 12)
            .unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        db.advance_cursor(run, Projection::HeaderSummary, 12)
            .unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(
        db.advance_cursor(run, Projection::HeaderSummary, 11),
        Err(LedgerError::CursorMovedBackwards {
            current: 12,
            offered: 11
        })
    );
    assert_eq!(db.cursor(run, Projection::HeaderSummary).unwrap(), Some(12));
}

#[test]
fn cursors_are_per_projection_and_per_run() {
    let root = Root::new();
    let mut db = root.open();
    let a = created_run(&mut db, 1);
    let b = created_run(&mut db, 2);
    db.advance_cursor(a, Projection::HeaderSummary, 5).unwrap();
    db.advance_cursor(a, Projection::Bus, 2).unwrap();
    db.advance_cursor(b, Projection::Bus, 9).unwrap();
    assert_eq!(db.cursor(a, Projection::HeaderSummary).unwrap(), Some(5));
    assert_eq!(db.cursor(a, Projection::Bus).unwrap(), Some(2));
    assert_eq!(db.cursor(b, Projection::HeaderSummary).unwrap(), None);
    assert_eq!(db.cursor(b, Projection::Bus).unwrap(), Some(9));
    assert_eq!(db.cursor(run_id(3), Projection::Bus).unwrap(), None);
    // The bookmark is the device's, not the run document's: a run this
    // ledger never recorded an intent for may still be projected here.
    assert_eq!(
        db.advance_cursor(run_id(3), Projection::Bus, 1).unwrap(),
        WriteOutcome::Applied
    );
}
