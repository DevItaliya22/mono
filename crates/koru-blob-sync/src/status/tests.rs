// SPDX-License-Identifier: MPL-2.0

use super::*;

fn referenced(referenced: u32, durable: u32) -> BlobTransferStatus {
    BlobTransferStatus {
        referenced,
        present: referenced,
        durable,
        ..BlobTransferStatus::default()
    }
}

/// **"Complete" is never true while anything is in flight** (D259).
///
/// Found on hardware: the Cloud sync tab read *"5 of 5 on Koru Cloud ·
/// sending"* beside *"Documents and pictures are up to date"*, because the
/// sentence and the indicator were derived from different fields. This is the
/// engine's half of the one answer both now read.
#[test]
fn a_transfer_in_flight_is_not_complete() {
    let mut status = referenced(5, 5);
    assert!(status.complete(), "nothing referenced is outstanding");
    assert!(!status.in_flight());

    status.uploading = 1;
    assert!(status.in_flight());
    assert!(
        !status.complete(),
        "an upload is running: the Workspace is not up to date",
    );

    status.uploading = 0;
    status.downloading = 1;
    assert!(
        !status.complete(),
        "a download is running: the Workspace is not up to date",
    );
}

/// A reference this build cannot parse is not a completeness anybody
/// measured, and neither is one Cloud has not confirmed.
#[test]
fn what_else_stops_a_workspace_reading_complete() {
    assert!(!referenced(6, 5).complete(), "one file is still owed");

    let mut unreadable = referenced(5, 5);
    unreadable.unreadable_references = 1;
    assert!(!unreadable.complete());
    assert!(
        !unreadable.in_flight(),
        "and it is not motion - nothing is moving, it simply cannot be read",
    );
}

/// `direction` is the last way bytes went, not whether they are going. A
/// caller that read it as motion reported *sending* over a finished pass,
/// which is exactly what the tab did.
#[test]
fn a_remembered_direction_is_not_a_transfer() {
    let mut status = referenced(5, 5);
    status.direction = Some(Direction::Upload);
    assert!(!status.in_flight());
    assert!(status.complete());
}
