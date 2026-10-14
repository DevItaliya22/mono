// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::{allowlist::OriginRefused, ports::TransferHold};

fn refused(code: BlobCode) -> TransferError {
    TransferError::Refused(code)
}

/// **The plan's §12 rule, as a table.** Authentication, epoch, incarnation,
/// malformed-data and checksum failures do not retry blindly.
#[test]
fn nothing_that_cannot_change_on_its_own_is_retried() {
    for code in [
        BlobCode::MalformedRange,
        BlobCode::LengthMismatch,
        BlobCode::HashMismatch,
        BlobCode::WrongObject,
        BlobCode::ObjectMissing,
        BlobCode::ObjectTooLarge,
        BlobCode::WrongRegion,
        BlobCode::WrongWorkspace,
        BlobCode::StaleIncarnation,
        BlobCode::StaleEpoch,
        BlobCode::CapabilityInvalid,
        BlobCode::ModeRefusesTransport,
    ] {
        assert_ne!(classify(&refused(code)), Disposition::Retry, "{code}");
    }
}

/// The three that are not a failure at all, and each has its own remedy.
#[test]
fn the_three_recoveries_are_distinct() {
    assert_eq!(
        classify(&refused(BlobCode::CapabilityExpired)),
        Disposition::Renew,
        "nothing is discarded; the same session gets a fresh capability"
    );
    assert_eq!(
        classify(&refused(BlobCode::RangeConflict)),
        Disposition::Resync,
        "the worker holds the bytes, so the worker is right about the offset"
    );
    assert_eq!(
        classify(&refused(BlobCode::UploadExpired)),
        Disposition::Restart,
        "the session is terminal; the local object has not moved"
    );
}

/// "The server said no" and "there was no server" are different facts, and a
/// client that merged them either spins or gives up.
#[test]
fn a_transport_failure_is_retried_and_a_refusal_is_not() {
    assert_eq!(classify(&TransferError::Unreachable), Disposition::Retry);
    assert_eq!(
        classify(&refused(BlobCode::StorageUnavailable)),
        Disposition::Retry,
        "retryable by the contract's own word"
    );
}

/// A control plane that named somewhere else is a loud session-wide stop.
/// Retrying would only ask it again.
#[test]
fn a_refused_byte_origin_stops_the_session() {
    assert_eq!(
        classify(&TransferError::Origin(OriginRefused::NotAllowed)),
        Disposition::ParkSession
    );
    assert_eq!(
        TransferError::Origin(OriginRefused::Scheme).code(),
        "byte_origin_refused"
    );
}

/// The Store's classes are the document pass's line, drawn once — with the one
/// named exception whose remedy is not "try again".
#[test]
fn the_store_keeps_its_own_classes_except_for_a_checksum() {
    assert_eq!(
        classify(&TransferError::Local(Code::BlobChecksumMismatch)),
        Disposition::ParkUnit,
        "the same read fails the same way forever"
    );
    assert_eq!(
        classify(&TransferError::Local(Code::StorageFailed)),
        Disposition::Retry
    );
    assert_eq!(
        classify(&TransferError::Local(Code::BlobNotFound)),
        Disposition::ParkUnit
    );
    assert_eq!(
        classify(&TransferError::Local(Code::Forbidden)),
        Disposition::ParkSession
    );
}

/// Policy and access are statements about the session, and the trigger that
/// changes them is on §10's list.
#[test]
fn policy_and_access_park_the_session() {
    assert_eq!(
        classify(&TransferError::NotPermitted(TransferHold::TransportMode)),
        Disposition::ParkSession
    );
    assert_eq!(
        classify(&TransferError::NotPermitted(TransferHold::Metered)),
        Disposition::ParkSession
    );
    assert_eq!(classify(&TransferError::NoAccess), Disposition::ParkSession);
}

/// A disk that cannot hold one picture may still hold the one beside it, so
/// the refusal is about **that blob** and the pass carries on — and it never
/// backs off into a retry loop, because a full disk does not empty on a
/// schedule and the trigger that changes it is somebody deleting something.
#[test]
fn a_full_volume_defers_one_blob_rather_than_the_session() {
    assert_eq!(
        classify(&TransferError::InsufficientStorage),
        Disposition::ParkUnit
    );
    assert_eq!(
        TransferError::InsufficientStorage.code(),
        "insufficient_storage"
    );
}

/// Every hold has a code of its own, and no two share one. §13: a silent
/// pause is indistinguishable from being refused.
#[test]
fn every_hold_names_itself() {
    let holds = [
        TransferHold::TransportMode,
        TransferHold::Metered,
        TransferHold::LowData,
        TransferHold::Offline,
    ];
    let codes: std::collections::BTreeSet<&str> = holds.iter().map(|hold| hold.as_str()).collect();
    assert_eq!(codes.len(), holds.len(), "two holds answer to one code");
    for hold in holds {
        assert_eq!(TransferError::NotPermitted(hold).code(), hold.as_str());
        assert!(!hold.as_str().is_empty());
    }
    // Only the two D50 names a person can waive are waivable.
    assert!(TransferHold::Metered.overridable());
    assert!(TransferHold::LowData.overridable());
    assert!(!TransferHold::TransportMode.overridable());
    assert!(!TransferHold::Offline.overridable());
    // And only the transport mode is a refusal rather than a pause.
    assert!(!TransferHold::TransportMode.is_pause());
    assert!(TransferHold::Metered.is_pause());
}

/// Bounded, and bounded from both ends: never below the floor, never above the
/// derived ceiling, however many times it has failed.
#[test]
fn the_backoff_is_bounded_at_both_ends() {
    let mut backoff = Backoff::new();
    for attempt in 1..=40u32 {
        let wait = backoff.failed();
        assert!(wait >= MIN_BACKOFF, "attempt {attempt}: {wait:?}");
        assert!(wait <= MAX_BACKOFF, "attempt {attempt}: {wait:?}");
    }
    assert_eq!(backoff.failures(), 40);
    backoff.cleared();
    assert_eq!(backoff.failures(), 0, "consecutive means consecutive");
}

/// Full jitter, not a fraction of one number: two devices that lost the same
/// network must not come back at the same instant.
#[test]
fn the_wait_is_spread_rather_than_one_number() {
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        let mut backoff = Backoff::new();
        // Four failures, so the ceiling is well above the floor and a spread
        // is observable at all.
        for _ in 0..3 {
            backoff.failed();
        }
        seen.insert(backoff.failed().as_millis());
    }
    assert!(
        seen.len() > 8,
        "64 devices produced {} distinct waits",
        seen.len()
    );
}

#[test]
fn a_missing_object_waits_minutes_and_the_wait_grows_to_its_ceiling() {
    let mut backoff = Backoff::new();
    let first = backoff.failed_within(MISSING_MIN_BACKOFF, MISSING_MAX_BACKOFF);
    assert!(first >= MISSING_MIN_BACKOFF, "{first:?}");
    assert!(first <= MISSING_MIN_BACKOFF * 2, "{first:?}");
    for _ in 0..10 {
        let wait = backoff.failed_within(MISSING_MIN_BACKOFF, MISSING_MAX_BACKOFF);
        assert!(wait >= MISSING_MIN_BACKOFF, "{wait:?}");
        assert!(wait <= MISSING_MAX_BACKOFF, "{wait:?}");
    }
    // The seconds-scale wait is untouched by the minutes-scale one.
    let mut quick = Backoff::new();
    assert!(quick.failed() <= MAX_BACKOFF);
}
