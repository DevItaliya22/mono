// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_device::{
    AllDataGrantV1, Authorization, AuthorizationDecision, DeviceDescriptorV1, DeviceSigningKey,
    RecordId, RelationshipId, ReplicationEndpointSecret, RevocationV1, TrustRelationshipV1,
};

#[derive(Default)]
struct Source {
    bytes: Option<Vec<u8>>,
    reads: usize,
    archives: usize,
    fail_archive: bool,
    fail_read: bool,
}
impl LegacySource for Source {
    fn read(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.reads += 1;
        if self.fail_read {
            Err("trust_migration_failed".into())
        } else {
            Ok(self.bytes.clone())
        }
    }
    fn archive(&mut self) -> Result<(), String> {
        self.archives += 1;
        if self.fail_archive {
            Err("fixture_archive_failed".into())
        } else {
            self.bytes = None;
            Ok(())
        }
    }
}

fn device(seed: u8, name: &str) -> (DeviceSigningKey, DeviceDescriptorV1) {
    let key = DeviceSigningKey::from_secret_bytes([seed; 32]);
    let endpoint = ReplicationEndpointSecret::from_secret_bytes([seed + 1; 32]);
    let descriptor = DeviceDescriptorV1::new(
        DeviceId::new(),
        name.into(),
        &key,
        endpoint.endpoint_id(),
        1,
    );
    (key, descriptor)
}

fn pair(
    local: &DeviceDescriptorV1,
    local_key: &DeviceSigningKey,
    peer: &DeviceDescriptorV1,
    peer_key: &DeviceSigningKey,
) -> Vec<TrustRecord> {
    let id = RelationshipId::new();
    vec![
        TrustRecord::Relationship(TrustRelationshipV1::sign(
            id,
            local.clone(),
            peer.clone(),
            koru_device::transcript_hash(b"legacy QR"),
            2,
            local_key,
            peer_key,
        )),
        TrustRecord::AllDataGrant(AllDataGrantV1::sign(
            id,
            peer.device_id,
            local.device_id,
            RecordId::new(),
            3,
            local_key,
        )),
        TrustRecord::AllDataGrant(AllDataGrantV1::sign(
            id,
            local.device_id,
            peer.device_id,
            RecordId::new(),
            3,
            peer_key,
        )),
    ]
}

fn source(local: &DeviceDescriptorV1, records: Vec<TrustRecord>, revision: u64) -> Source {
    Source {
        bytes: Some(
            serde_json::to_vec(&PersistedTrust {
                descriptor: local.clone(),
                records,
                effective_revision: revision,
            })
            .unwrap(),
        ),
        ..Default::default()
    }
}

fn assert_empty(repository: &SqliteTrustRepository) {
    assert!(repository.local_device().unwrap().is_none());
    assert!(repository.records().unwrap().is_empty());
}

#[test]
fn legacy_qr_authority_revocations_and_revision_survive_import_and_reentry() {
    let (key, local) = device(11, "Phone");
    let (active_key, active) = device(21, "Desktop");
    let (revoked_key, revoked) = device(31, "Old desktop");
    let mut records = pair(&local, &key, &active, &active_key);
    let old = pair(&local, &key, &revoked, &revoked_key);
    let TrustRecord::Relationship(relationship) = &old[0] else {
        unreachable!()
    };
    records.push(TrustRecord::Revocation(RevocationV1::sign(
        relationship.relationship_id,
        revoked.device_id,
        local.device_id,
        RecordId::new(),
        4,
        &key,
    )));
    records.extend(old);
    let mut source = source(&local, records.clone(), 37);
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    migrate(&mut repository, &local, &mut source).unwrap();
    koru_device::sort_for_gossip(&mut records);
    assert_eq!(repository.records().unwrap(), records);
    let view = repository.trust_view().unwrap();
    assert_eq!(view.local_device, local);
    assert_eq!(view.effective_revision, 37);
    let workspace = WorkspaceId::new();
    let inventory = [(workspace, ProfileId::new())].into_iter().collect();
    let authorization = Authorization::build(&view, &inventory);
    assert!(matches!(
        authorization.check(active.endpoint_id, workspace),
        AuthorizationDecision::Allowed(_)
    ));
    assert!(matches!(
        authorization.check(revoked.endpoint_id, workspace),
        AuthorizationDecision::Denied(_)
    ));
    assert_eq!((source.reads, source.archives), (1, 1));

    // Re-enter the startup migration with the same committed database. The
    // source could have been replaced by an older build; it is never read.
    source.bytes = Some(b"not even JSON".to_vec());
    source.fail_read = true;
    migrate(&mut repository, &local, &mut source).unwrap();
    assert_eq!((source.reads, source.archives), (1, 1));
    assert_eq!(repository.trust_view().unwrap(), view);
    assert_eq!(repository.records().unwrap(), records);
}

#[test]
fn archive_failure_is_after_commit_and_a_lingering_json_never_overwrites_sqlite() {
    let (key, mut local) = device(41, "Name in device.json");
    let mut renamed = local.clone();
    renamed.display_name = "Name in trust.json".into();
    let mut source = source(&renamed, vec![], 9);
    source.fail_archive = true;
    let bytes = source.bytes.clone();
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    migrate(&mut repository, &local, &mut source).unwrap();
    assert_eq!(source.bytes, bytes);
    assert_eq!(repository.local_device().unwrap(), Some(renamed));
    let newer = TrustRecord::DeviceName(koru_device::DeviceNameV1::sign(
        RecordId::new(),
        local.device_id,
        "Renamed after SQLite import".into(),
        10,
        &key,
    ));
    let verified =
        koru_device::verify_gossip_record(&repository.trust_view().unwrap(), newer.clone())
            .unwrap();
    repository.apply(verified).unwrap();
    assert_eq!(repository.trust_view().unwrap().effective_revision, 10);
    local.display_name = "Later stale startup copy".into();
    migrate(&mut repository, &local, &mut source).unwrap();
    assert_eq!((source.reads, source.archives), (1, 1));
    assert_eq!(
        repository.local_device().unwrap().unwrap().display_name,
        "Name in trust.json"
    );
    let view = repository.trust_view().unwrap();
    assert_eq!(view.effective_revision, 10);
    assert_eq!(
        view.display_name_of(local.device_id).as_deref(),
        Some("Renamed after SQLite import")
    );
    assert_eq!(repository.records().unwrap(), vec![newer]);
    assert_eq!(source.bytes, bytes);
}

#[test]
fn migration_preserves_a_historical_lifecycle_fact_signed_before_its_peer_was_revoked() {
    let (key, local) = device(131, "Phone");
    let (peer_key, peer) = device(141, "Desktop");
    let mut records = pair(&local, &key, &peer, &peer_key);
    let TrustRecord::Relationship(relationship) = &records[0] else {
        unreachable!()
    };
    let relationship_id = relationship.relationship_id;
    let profile = ProfileId::new();
    let lifecycle = koru_device::ProfileLifecycleV1::sign(
        RecordId::new(),
        peer.device_id,
        profile,
        koru_device::ProfileLifecycleStateV1::Deleted,
        7,
        RecordId::new().as_uuid(),
        5,
        &peer_key,
    );

    // Establish that this is a fact accepted while the peer held the QR
    // all-data grant, not authority manufactured by the import exception.
    let mut earlier = SqliteTrustRepository::in_memory().unwrap();
    earlier.import_legacy(&local, &records, 3).unwrap();
    assert!(
        koru_device::verify_gossip_record(
            &earlier.trust_view().unwrap(),
            TrustRecord::ProfileLifecycle(lifecycle.clone()),
        )
        .is_ok()
    );
    records.push(TrustRecord::ProfileLifecycle(lifecycle.clone()));
    records.push(TrustRecord::Revocation(RevocationV1::sign(
        relationship_id,
        peer.device_id,
        local.device_id,
        RecordId::new(),
        6,
        &key,
    )));
    let mut input = source(&local, records.clone(), 5);
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    migrate(&mut repository, &local, &mut input).unwrap();
    let view = repository.trust_view().unwrap();
    assert_eq!(view.effective_revision, 5);
    assert_eq!(view.lifecycles.get(&profile), Some(&lifecycle));
    assert!(view.profile_is_deleted(profile));
    assert!(matches!(
        koru_device::verify_gossip_record(&view, TrustRecord::ProfileLifecycle(lifecycle),),
        Err(koru_device::GossipRejection::UnauthorizedProfile)
    ));
    koru_device::sort_for_gossip(&mut records);
    assert_eq!(repository.records().unwrap(), records);

    // Historical authority bypasses present-day admission only; its original
    // signature is still mandatory and a forged fact rolls back the import.
    let TrustRecord::ProfileLifecycle(forged) = records
        .iter_mut()
        .find(|record| matches!(record, TrustRecord::ProfileLifecycle(_)))
        .unwrap()
    else {
        unreachable!()
    };
    forged.lifecycle_revision += 1;
    let mut input = source(&local, records, 5);
    let bytes = input.bytes.clone();
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    assert!(migrate(&mut repository, &local, &mut input).is_err());
    assert_empty(&repository);
    assert_eq!(input.bytes, bytes);
    assert_eq!(input.archives, 0);
}

#[test]
fn corrupt_partial_unknown_and_forged_legacy_input_leave_source_and_database_untouched() {
    let (key, local) = device(51, "Phone");
    let (peer_key, peer) = device(61, "Desktop");
    let records = pair(&local, &key, &peer, &peer_key);
    let mut forged = records.clone();
    let TrustRecord::AllDataGrant(grant) = &mut forged[1] else {
        unreachable!()
    };
    grant.issued_at_millis += 1;
    let mut unknown = records.clone();
    unknown.push(TrustRecord::Unknown);
    let mut invalid_descriptor = local.clone();
    invalid_descriptor.endpoint_binding_signature = peer.endpoint_binding_signature;
    let valid = source(&local, records, 17).bytes.unwrap();
    for bytes in [
        b"corrupt".to_vec(),
        valid[..valid.len() / 2].to_vec(),
        source(&local, forged, 17).bytes.unwrap(),
        source(&local, unknown, 17).bytes.unwrap(),
        source(&invalid_descriptor, vec![], 17).bytes.unwrap(),
        source(&local, vec![], u64::MAX).bytes.unwrap(),
    ] {
        let mut source = Source {
            bytes: Some(bytes.clone()),
            ..Default::default()
        };
        let mut repository = SqliteTrustRepository::in_memory().unwrap();
        assert!(migrate(&mut repository, &local, &mut source).is_err());
        assert_empty(&repository);
        assert_eq!(source.bytes, Some(bytes));
        assert_eq!((source.reads, source.archives), (1, 0));
        // A corrected retry is a first import, not a partly committed repair.
        source.bytes = Some(valid.clone());
        migrate(&mut repository, &local, &mut source).unwrap();
        assert_eq!(repository.trust_view().unwrap().effective_revision, 17);
    }
}

#[test]
fn mismatched_identity_and_unreadable_source_do_not_initialize_or_archive() {
    let (_, local) = device(71, "Phone");
    let (_, other) = device(81, "Other installation");
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    let mut input = source(&other, vec![], 0);
    let original = input.bytes.clone();
    assert_eq!(
        migrate(&mut repository, &local, &mut input),
        Err("device_identity_mismatch".into())
    );
    assert_empty(&repository);
    assert_eq!(input.bytes, original);
    assert_eq!(input.archives, 0);
    input.fail_read = true;
    assert!(migrate(&mut repository, &local, &mut input).is_err());
    assert_empty(&repository);
    assert_eq!(input.archives, 0);
}

#[test]
fn fresh_install_initializes_once_without_an_archive() {
    let (_, local) = device(91, "Phone");
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    let mut input = Source::default();
    migrate(&mut repository, &local, &mut input).unwrap();
    assert_eq!(repository.local_device().unwrap(), Some(local.clone()));
    assert_eq!(repository.trust_view().unwrap().effective_revision, 0);
    assert!(repository.records().unwrap().is_empty());
    migrate(&mut repository, &local, &mut input).unwrap();
    assert_eq!((input.reads, input.archives), (1, 0));
}

#[test]
fn migrated_host_keeps_build_name_gossip_and_unlink_policy_idempotent() {
    let (key, local) = device(101, "Phone");
    let (peer_key, peer) = device(111, "Desktop");
    let records = pair(&local, &key, &peer, &peer_key);
    let relationship = records[0].clone();
    let mut input = source(&local, records, 20);
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    migrate(&mut repository, &local, &mut input).unwrap();
    // No method used in this test accesses root; all storage is SQLite memory.
    let mut state = TrustState {
        root: PathBuf::new(),
        repository,
    };
    let payload = koru_device::BuildPayload {
        app_version: "test".into(),
        channel: "test".into(),
        branch: None,
        commit: None,
        os: Some("Android".into()),
        os_version: None,
        peer_protocol: Some(1),
    };
    assert!(state.advertise_build(&payload, &key).unwrap());
    let revision = state.view().unwrap().effective_revision;
    assert!(!state.advertise_build(&payload, &key).unwrap());
    assert_eq!(state.view().unwrap().effective_revision, revision);
    assert_eq!(
        state.apply_direct_trust(peer.device_id, relationship),
        Err("unexpected_trust_record".into())
    );
    let name = TrustRecord::DeviceName(koru_device::DeviceNameV1::sign(
        RecordId::new(),
        peer.device_id,
        "Renamed desktop".into(),
        8,
        &peer_key,
    ));
    assert!(state.apply_remote_records(vec![name.clone()]).unwrap());
    assert!(!state.apply_remote_records(vec![name]).unwrap());
    assert_eq!(
        state
            .view()
            .unwrap()
            .display_name_of(peer.device_id)
            .as_deref(),
        Some("Renamed desktop")
    );
    let revocations = state.revoke(peer.device_id, &key).unwrap();
    let revision = state.view().unwrap().effective_revision;
    assert!(!state.apply_remote_records(revocations).unwrap());
    assert_eq!(state.view().unwrap().effective_revision, revision);
    assert_eq!(
        state.revoke(peer.device_id, &key),
        Err("device_not_linked".into())
    );
}

#[test]
fn a_mixed_gossip_batch_reports_rejection_but_retains_its_valid_durable_fact() {
    let (key, local) = device(121, "Phone");
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    migrate(&mut repository, &local, &mut Source::default()).unwrap();
    let mut state = TrustState {
        root: PathBuf::new(),
        repository,
    };
    let payload = koru_device::BuildPayload {
        app_version: "test".into(),
        channel: "test".into(),
        branch: None,
        commit: None,
        os: None,
        os_version: None,
        peer_protocol: None,
    };
    let build = TrustRecord::DeviceBuild(koru_device::DeviceBuildV1::sign(
        RecordId::new(),
        local.device_id,
        &payload,
        10,
        &key,
    ));
    let mut forged = koru_device::DeviceNameV1::sign(
        RecordId::new(),
        local.device_id,
        "Original".into(),
        10,
        &key,
    );
    forged.display_name = "Forged".into();
    assert_eq!(
        state.apply_remote_records(vec![build.clone(), TrustRecord::DeviceName(forged)]),
        Err("rejected_trust_records".into())
    );
    let view = state.view().unwrap();
    assert_eq!(view.effective_revision, 1);
    assert!(view.build_is_current(local.device_id, &payload));
    assert_eq!(
        view.display_name_of(local.device_id).as_deref(),
        Some("Phone")
    );
    assert!(!state.apply_remote_records(vec![build]).unwrap());
    assert_eq!(state.view().unwrap().effective_revision, 1);
}

#[derive(Default)]
struct Archive {
    effects: Vec<String>,
    fail: Option<&'static str>,
}
impl IdentityArchive for Archive {
    fn create(&mut self) -> Result<(), String> {
        self.effects.push("create".into());
        if self.fail == Some("create") {
            Err("fixture_create_failed".into())
        } else {
            Ok(())
        }
    }
    fn move_if_present(&mut self, name: &str) -> Result<(), String> {
        self.effects.push(name.into());
        if self.fail == Some(name) {
            Err("fixture_move_failed".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn reset_archives_trust_and_sidecars_before_the_identity_marker() {
    let mut archive = Archive::default();
    archive_identity(&mut archive).unwrap();
    assert_eq!(
        archive.effects,
        [
            "create",
            "trust.db-wal",
            "trust.db-shm",
            "trust.db",
            "trust.json",
            "trust.migrated-to-sqlite.json",
            "device.json",
        ]
    );
}

#[test]
fn reset_stops_on_archive_failure_before_moving_device_identity() {
    let mut archive = Archive {
        fail: Some("create"),
        ..Default::default()
    };
    assert_eq!(
        archive_identity(&mut archive),
        Err("fixture_create_failed".into())
    );
    assert_eq!(archive.effects, ["create"]);
    for failed in [
        "trust.db-wal",
        "trust.db-shm",
        "trust.db",
        "trust.json",
        "trust.migrated-to-sqlite.json",
    ] {
        let mut archive = Archive {
            fail: Some(failed),
            ..Default::default()
        };
        assert_eq!(
            archive_identity(&mut archive),
            Err("fixture_move_failed".into())
        );
        assert_eq!(archive.effects.last().map(String::as_str), Some(failed));
        assert!(!archive.effects.iter().any(|name| name == "device.json"));
    }
}
