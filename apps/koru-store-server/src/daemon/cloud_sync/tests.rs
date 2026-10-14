// SPDX-License-Identifier: MPL-2.0

//! The two decisions this module owns that are not about the carrier: where
//! the line between "this unit" and "this session" is drawn, and the bound
//! that stops a deferral from becoming a silent permanent one. The pass
//! itself is exercised end to end against a real carrier and real Store jobs
//! in `store_jobs::tests::client_io::reconcile`.
use super::*;

fn unit(n: u8) -> Unit {
    Unit::Document {
        workspace_id: CloudId::from_bytes([
            0, 1, 2, 3, 4, 5, 0x40, 7, 0x80, 9, 10, 11, 12, 13, 14, 15,
        ]),
        document_id: CloudId::from_bytes([
            n, 1, 2, 3, 4, 5, 0x40, 7, 0x80, 9, 10, 11, 12, 13, 14, n,
        ]),
    }
}

/// The line, stated as the codes that actually caused trouble.
///
/// Every `Code` is covered by `Code::ALL` below, so a code added later cannot
/// quietly land on the wrong side of it without someone choosing which side.
#[test]
fn a_refusal_about_the_unit_defers_and_a_refusal_about_the_session_does_not() {
    for code in [
        // The Profile-name disagreement that froze a whole dataset, and its
        // neighbours in `Class::Conflict`.
        Code::ProfileIdentityConflict,
        Code::ProfileUnitScopeMismatch,
        Code::SchemaTooNew,
        Code::ReadOnly,
        // A unit whose target is not here.
        Code::DocumentNotFound,
        Code::ProfileNotFound,
        // A record this Store will not take.
        Code::CrdtRejected,
        Code::PayloadTooLarge,
        Code::VersionUnavailable,
    ] {
        assert!(deferrable(code), "{code} is about one unit");
    }
    for code in [
        // Nothing beside this unit would fare any better.
        Code::StoreUnreachable,
        Code::StorageFailed,
        Code::CorruptState,
        Code::Internal,
        Code::Forbidden,
        Code::Unauthenticated,
    ] {
        assert!(!deferrable(code), "{code} is about the session");
    }
}

/// Deferring is decided by `Class` alone, so a `Code` added later is decided
/// by the class its author gave it rather than by being forgotten here.
#[test]
fn every_code_has_a_side() {
    for code in Code::ALL {
        let expected = matches!(
            code.class(),
            Class::NotFound | Class::Invalid | Class::Conflict
        );
        assert_eq!(deferrable(*code), expected, "{code}");
    }
}

#[test]
fn a_run_of_refusals_counts_and_a_settle_clears_it() {
    let mut held = Deferrals::new();
    assert_eq!(held.refused(unit(1)), 1);
    assert_eq!(held.refused(unit(1)), 2);
    // A different unit keeps its own run.
    assert_eq!(held.refused(unit(2)), 1);
    assert_eq!(held.refused(unit(1)), MAX_CONSECUTIVE_REFUSALS);
    held.cleared(unit(1));
    assert_eq!(held.tracked(), 1, "a served unit stops being tracked");
    assert_eq!(
        held.refused(unit(1)),
        1,
        "consecutive means consecutive: the run starts again"
    );
}

/// A ledger with no room left says the bound is crossed rather than going
/// quiet, because going quiet is the failure this whole thing exists to avoid.
#[test]
fn a_full_ledger_is_loud_rather_than_forgetful() {
    let mut held = Deferrals::new();
    for n in 0..MAX_TRACKED_UNITS {
        let padded = Unit::Document {
            workspace_id: CloudId::from_bytes([
                0, 1, 2, 3, 4, 5, 0x40, 7, 0x80, 9, 10, 11, 12, 13, 14, 15,
            ]),
            document_id: CloudId::from_bytes([
                (n >> 8) as u8,
                n as u8,
                2,
                3,
                4,
                5,
                0x40,
                7,
                0x80,
                9,
                10,
                11,
                12,
                13,
                14,
                15,
            ]),
        };
        assert_eq!(held.refused(padded), 1);
    }
    assert_eq!(held.tracked(), MAX_TRACKED_UNITS);
    assert_eq!(
        held.refused(unit(200)),
        MAX_CONSECUTIVE_REFUSALS,
        "one more refusing unit than the ledger holds is itself the verdict"
    );
    assert_eq!(held.tracked(), MAX_TRACKED_UNITS, "and nothing was evicted");
}
