// SPDX-License-Identifier: MPL-2.0

//! Pure planning tests: ordering, rotation, and what a park may honestly
//! name. The live-transfer behaviour is covered by the crate-level
//! `tests.rs` harness, which owns the connected-pair fixture these would
//! otherwise duplicate.

use super::*;

fn devices(n: usize) -> Vec<DeviceId> {
    (0..n).map(|_| DeviceId::new()).collect()
}

#[test]
fn attempt_order_prefers_known_holders() {
    let devices = devices(3);
    let holders = BTreeSet::from([devices[2]]);
    let order = order_attempts(devices.clone(), Some(&holders), None, 0);
    assert_eq!(order[0], devices[2]);
    assert_eq!(order.len(), 3);
}

#[test]
fn attempt_order_rotates_the_starting_peer() {
    let devices = devices(4);
    // The rotation offset is named rather than taken from the process-wide
    // cursor: it is the thing under test, and a global read inside the
    // function made four sibling tests in this binary share one counter.
    let first_starts: BTreeSet<DeviceId> = (0..4)
        .map(|start| order_attempts(devices.clone(), None, None, start)[0])
        .collect();
    assert_eq!(
        first_starts.len(),
        4,
        "four successive hydrations should start on four different peers, got {first_starts:?}"
    );
}

#[test]
fn rotation_keeps_holders_first() {
    let devices = devices(4);
    let holders = BTreeSet::from([devices[1], devices[3]]);
    for start in 0..4 {
        let order = order_attempts(devices.clone(), Some(&holders), None, start);
        assert!(holders.contains(&order[0]));
        assert!(holders.contains(&order[1]));
        assert!(!holders.contains(&order[2]));
    }
}

#[test]
fn the_durable_origin_is_asked_first_when_the_directory_has_not_seen_it() {
    // A fresh session whose inventory has not arrived yet: the directory is
    // empty, but the durable origin still says who once held the bytes.
    let devices = devices(3);
    let order = order_attempts(devices.clone(), None, Some(devices[1]), 0);
    assert_eq!(order[0], devices[1]);
    assert_eq!(order.len(), 3);
}

#[test]
fn an_offline_origin_is_not_asked() {
    // The origin is a hint for the attempt list only while connected; offline
    // it is the parking decision's business, so nothing here may return it.
    let devices = devices(2);
    let origin = DeviceId::new();
    let order = order_attempts(devices.clone(), None, Some(origin), 0);
    assert!(!order.contains(&origin));
}

#[test]
fn a_park_names_only_the_devices_known_to_hold_the_blob() {
    let devices = devices(3);
    let holders = BTreeSet::from([devices[1]]);
    let labels = known_offline_holders(&devices, Some(&holders), None, |device| {
        Some(format!("device-{}", device.to_storage()))
    });
    assert_eq!(labels, vec![format!("device-{}", devices[1].to_storage())]);
}

#[test]
fn the_durable_origin_names_a_holder_the_directory_has_not_seen() {
    let devices = devices(2);
    let labels = known_offline_holders(&devices, None, Some(devices[0]), |device| {
        Some(format!("device-{}", device.to_storage()))
    });
    assert_eq!(labels, vec![format!("device-{}", devices[0].to_storage())]);
}

#[test]
fn an_unknown_holder_is_no_label_rather_than_a_uuid() {
    let devices = devices(1);
    let labels = known_offline_holders(&devices, None, Some(devices[0]), |_| None);
    assert!(labels.is_empty());
}

#[test]
fn the_directory_records_and_forgets() {
    let workspace = koru_store_core::WorkspaceId::new();
    let hash = BlobHash::from_digest(BlobHash::BLAKE3, &"ab".repeat(32)).unwrap();
    let devices = devices(2);
    let mut directory = BlobDirectory::new();
    directory.record(workspace, &hash, devices[0]);
    directory.record(workspace, &hash, devices[1]);
    assert_eq!(
        directory.holders(workspace, &hash),
        Some(&BTreeSet::from([devices[0], devices[1]]))
    );
    directory.forget_device(devices[0]);
    assert_eq!(
        directory.holders(workspace, &hash),
        Some(&BTreeSet::from([devices[1]]))
    );
    directory.forget_device(devices[1]);
    assert_eq!(directory.holders(workspace, &hash), None);
}

/// What a waiting list reads: the advertisers of each hash asked about, in
/// this workspace only, and no entry for a hash nobody advertised.
#[test]
fn the_directory_answers_the_advertisers_of_the_hashes_asked_about() {
    let (workspace, other) = (
        koru_store_core::WorkspaceId::new(),
        koru_store_core::WorkspaceId::new(),
    );
    let held = BlobHash::from_digest(BlobHash::BLAKE3, &"ab".repeat(32)).unwrap();
    let unheard = BlobHash::from_digest(BlobHash::BLAKE3, &"cd".repeat(32)).unwrap();
    let devices = devices(3);
    let mut directory = BlobDirectory::new();
    directory.record(workspace, &held, devices[0]);
    directory.record(workspace, &held, devices[1]);
    directory.record(other, &unheard, devices[2]);
    let mut expected = vec![devices[0], devices[1]];
    expected.sort();
    assert_eq!(
        directory.advertised(workspace, &[held.clone(), unheard]),
        HashMap::from([(held, expected)])
    );
}

fn candidate(device: DeviceId, name: &str) -> protocol::AuthenticatedPeer {
    protocol::AuthenticatedPeer {
        relationship_id: koru_device::RelationshipId::new(),
        device_id: device,
        endpoint_id: koru_device::ReplicationEndpointId::from_bytes([0; 32]),
        all_data: true,
        authorized_profiles: BTreeSet::new(),
        authorized_workspaces: BTreeSet::new(),
        authorization_fence: protocol::AuthorizationFence::active(),
        display_name: Some(name.to_string()),
        incompatible: false,
        peer_protocol: Some(protocol::PEER_PROTOCOL_VERSION),
    }
}

#[test]
fn a_plan_with_nobody_connected_parks_and_names_the_known_holders() {
    let devices = devices(2);
    let candidates = vec![
        candidate(devices[0], "Studio"),
        candidate(devices[1], "Pixel 8"),
    ];
    let plan = plan_blob_hydration(&candidates, &BTreeSet::new(), None, Some(devices[0]));
    let HydrationPlan::Wait(BlobWaitReason::Offline { holders }) = plan else {
        panic!("nobody connected must park, got {plan:?}");
    };
    assert_eq!(holders, vec!["Studio".to_string()]);
}

#[test]
fn a_plan_asks_every_connected_candidate_and_parks_for_the_offline_rest() {
    let devices = devices(3);
    let candidates = vec![
        candidate(devices[0], "Studio"),
        candidate(devices[1], "Pixel 8"),
        candidate(devices[2], "Workshop"),
    ];
    let connected = BTreeSet::from([devices[1], devices[2]]);
    let plan = plan_blob_hydration(&candidates, &connected, None, Some(devices[0]));
    let HydrationPlan::Attempt { order, park } = plan else {
        panic!("connected candidates must be attempted, got {plan:?}");
    };
    // Round robin over everyone connected: the origin is offline and is the
    // park's business, not a filter on whom we ask.
    assert_eq!(order.len(), 2);
    assert!(order.contains(&devices[1]) && order.contains(&devices[2]));
    assert!(park.worth_waiting);
    assert_eq!(park.known_holders, vec!["Studio".to_string()]);
}

#[test]
fn a_plan_with_everyone_connected_is_not_worth_waiting_for_anyone() {
    let devices = devices(2);
    let candidates = vec![
        candidate(devices[0], "Studio"),
        candidate(devices[1], "Pixel 8"),
    ];
    let connected = BTreeSet::from([devices[0], devices[1]]);
    let plan = plan_blob_hydration(&candidates, &connected, None, None);
    let HydrationPlan::Attempt { order, park } = plan else {
        panic!("connected candidates must be attempted, got {plan:?}");
    };
    assert_eq!(order.len(), 2);
    assert!(!park.worth_waiting);
    assert!(park.known_holders.is_empty());
}
