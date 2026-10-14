// SPDX-License-Identifier: MPL-2.0

//! The parked/arrived bookkeeping: what the placeholder is told, and what the
//! reconnect retry remembers.

use super::*;

#[test]
fn a_parked_load_names_the_devices_it_waits_for() {
    let mut loader = ImageLoader::for_test();
    let mut images = ImageCache::new();
    let workspace = WorkspaceId::new();
    let error = koru_store_core::errors::Error::new(
        Code::BlobWaitingForPeer,
        "blob hydration failed: blob_waiting_for_peer (retryable=true)",
    )
    .with_waiting_on(vec!["Studio".to_string()]);
    loader.receive_for_test(workspace, "hash-1", Err(error), workspace, &mut images);
    assert_eq!(images.pending("hash-1"), Some(&["Studio".to_string()][..]));
    assert!(loader.retry.contains(&(workspace, "hash-1".to_string())));
}

#[test]
fn a_park_without_names_leaves_the_placeholder_generic() {
    let mut loader = ImageLoader::for_test();
    let mut images = ImageCache::new();
    let workspace = WorkspaceId::new();
    let error = koru_store_core::errors::Error::new(Code::BlobWaitingForPeer, "parked, no names");
    loader.receive_for_test(workspace, "hash-1", Err(error), workspace, &mut images);
    assert_eq!(images.pending("hash-1"), None);
}

#[test]
fn a_result_for_another_workspace_is_not_ours() {
    let mut loader = ImageLoader::for_test();
    let mut images = ImageCache::new();
    let current = WorkspaceId::new();
    let other = WorkspaceId::new();
    let error = koru_store_core::errors::Error::new(Code::BlobWaitingForPeer, "parked")
        .with_waiting_on(vec!["Studio".to_string()]);
    loader.receive_for_test(other, "hash-1", Err(error), current, &mut images);
    assert_eq!(images.pending("hash-1"), None);
    assert!(loader.retry.is_empty());
}

#[test]
fn a_definitive_miss_stops_the_placeholder_waiting() {
    let mut loader = ImageLoader::for_test();
    let mut images = ImageCache::new();
    let workspace = WorkspaceId::new();
    let parked = koru_store_core::errors::Error::new(Code::BlobWaitingForPeer, "parked")
        .with_waiting_on(vec!["Studio".to_string()]);
    loader.receive_for_test(workspace, "hash-1", Err(parked), workspace, &mut images);
    assert!(images.pending("hash-1").is_some());

    let missing = koru_store_core::errors::Error::new(Code::BlobNotFound, "nobody has it");
    loader.receive_for_test(workspace, "hash-1", Err(missing), workspace, &mut images);
    assert_eq!(images.pending("hash-1"), None);
}

/// **The hold reaches the placeholder** (D259).
///
/// The Store carries the blob engine's reason beside its own code precisely
/// so the placeholder does not have to guess, and this is the one hop in
/// between. Without it the sentence falls back to rung 1's peer answer, which
/// is what sent a person to restart hardware for a Workspace this device had
/// never been switched on for.
#[test]
fn a_hold_is_recorded_for_the_placeholder_to_read() {
    let mut loader = ImageLoader::for_test();
    let mut images = ImageCache::new();
    let workspace = WorkspaceId::new();
    let error = koru_store_core::errors::Error::new(Code::BlobWaitingForPeer, "parked")
        .with_waiting_on(vec!["Studio".to_string()])
        .with_hold(Some("mode_refuses_transport".to_string()));
    let redraw = loader.receive_for_test(workspace, "hash-1", Err(error), workspace, &mut images);
    assert!(redraw, "the placeholder's words changed");
    assert_eq!(images.hold("hash-1"), Some("mode_refuses_transport"));
    // Both facts are kept: a device *would* have it, and this one is not
    // asking. The placeholder decides which of them a person reads.
    assert_eq!(images.pending("hash-1"), Some(&["Studio".to_string()][..]));
}

/// A repeated hold is not news. The retry cadence re-reports the same answer
/// every few seconds, and a redraw per report is a redraw per tick.
#[test]
fn the_same_hold_reported_again_is_not_a_redraw() {
    let mut loader = ImageLoader::for_test();
    let mut images = ImageCache::new();
    let workspace = WorkspaceId::new();
    let held = || {
        koru_store_core::errors::Error::new(Code::StorageFailed, "held")
            .with_hold(Some("network_metered".to_string()))
    };
    assert!(loader.receive_for_test(workspace, "h", Err(held()), workspace, &mut images));
    assert!(!loader.receive_for_test(workspace, "h", Err(held()), workspace, &mut images));
    assert_eq!(images.hold("h"), Some("network_metered"));
}

/// A failure that names no hold clears the one that was there: the reason
/// stopped applying, and a placeholder still explaining it would be a stale
/// sentence about a state that has passed.
#[test]
fn a_failure_with_no_hold_clears_the_last_one() {
    let mut loader = ImageLoader::for_test();
    let mut images = ImageCache::new();
    let workspace = WorkspaceId::new();
    let held = koru_store_core::errors::Error::new(Code::StorageFailed, "held")
        .with_hold(Some("paused_by_person".to_string()));
    loader.receive_for_test(workspace, "h", Err(held), workspace, &mut images);
    let plain = koru_store_core::errors::Error::new(Code::BlobWaitingForPeer, "parked");
    assert!(loader.receive_for_test(workspace, "h", Err(plain), workspace, &mut images));
    assert_eq!(images.hold("h"), None);
}
