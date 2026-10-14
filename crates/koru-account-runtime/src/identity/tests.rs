// SPDX-License-Identifier: MPL-2.0

use koru_device::{
    AllDataGrantV1, ApplyOutcome, DeviceSigningKey, RecordId, RelationshipId,
    ReplicationEndpointSecret, StoredLocalDevice, TrustRepository,
};
use std::str::FromStr as _;

use super::*;

// Repository fixtures initialize public facts directly; no desktop secret store is used.
fn local_identity(path: &Path, now_millis: i64) -> StoredLocalDevice {
    let signing_key = DeviceSigningKey::generate();
    let endpoint_secret = ReplicationEndpointSecret::generate();
    let descriptor = DeviceDescriptorV1::new(
        DeviceId::new(),
        "Desktop".into(),
        &signing_key,
        endpoint_secret.endpoint_id(),
        now_millis,
    );
    SqliteTrustRepository::open(path)
        .unwrap()
        .initialize(&descriptor)
        .unwrap();
    StoredLocalDevice {
        descriptor,
        signing_key,
        endpoint_secret,
    }
}

fn enrollment(
    local: &StoredLocalDevice,
) -> (
    koru_account_session::VerifiedDeviceAttestationV1,
    koru_pairing::PreparedEnrollmentV1,
) {
    let peer_key = koru_device::DeviceSigningKey::generate();
    let peer_endpoint = koru_device::ReplicationEndpointSecret::generate();
    let peer = koru_device::DeviceDescriptorV1::new(
        koru_device::DeviceId::new(),
        "Phone".into(),
        &peer_key,
        peer_endpoint.endpoint_id(),
        43,
    );
    let profile =
        koru_store_core::ProfileId::parse("60000000-0000-4000-8000-000000000006").unwrap();
    let ring = koru_account_session::DeviceAttestationKeyRing::new(
        "attestation".into(),
        [(
            "attestation".into(),
            ed25519_dalek::SigningKey::from_bytes(&[61; 32]),
        )],
    )
    .unwrap();
    let signed = ring
        .sign(&koru_account_session::DeviceAttestationClaimsV1 {
            version: koru_account_session::DEVICE_ATTESTATION_VERSION,
            issuer: "https://api.koru.do".into(),
            audience: koru_account_session::DEVICE_ATTESTATION_AUDIENCE.into(),
            koru_identity_id: koru_account_core::KoruIdentityId::from_str(
                "10000000-0000-4000-8000-000000000001",
            )
            .unwrap(),
            device_id: peer.device_id,
            signing_public_key: peer.signing_public_key,
            endpoint_id: peer.endpoint_id,
            authorized_profile_ids: vec![profile],
            session_id: koru_account_core::AccountSessionId::from_str(
                "30000000-0000-4000-8000-000000000003",
            )
            .unwrap(),
            token_id: "atomic-enrollment".into(),
            issued_at_millis: 1_000_000,
            expires_at_millis: 1_120_000,
        })
        .unwrap();
    let verified = koru_account_session::verify_device_attestation(
        &signed,
        &ring.verification_keys(),
        "https://api.koru.do",
        1_001_000,
    )
    .unwrap();
    let transcript = koru_pairing::EnrollmentTranscriptV1::new(
        &verified,
        profile,
        local.descriptor.clone(),
        peer,
        [71; 32],
    )
    .unwrap();
    let join =
        koru_pairing::EnrollmentJoinV1::sign(signed, &verified, transcript, &peer_key, 1_002_000)
            .unwrap();
    let prepared = join.complete(&verified, &local.signing_key).unwrap();
    (verified, prepared)
}

#[test]
fn revoked_historical_enrollment_is_not_an_effective_completion() {
    let local_key = koru_device::DeviceSigningKey::generate();
    let local_endpoint = koru_device::ReplicationEndpointSecret::generate();
    let local_descriptor = koru_device::DeviceDescriptorV1::new(
        koru_device::DeviceId::new(),
        "Desktop".into(),
        &local_key,
        local_endpoint.endpoint_id(),
        42,
    );
    let local = StoredLocalDevice {
        descriptor: local_descriptor.clone(),
        signing_key: local_key,
        endpoint_secret: local_endpoint,
    };
    let (_, prepared) = enrollment(&local);
    let profile = prepared.transcript.profile_id;
    let peer = prepared.transcript.joining_device.clone();
    let mut view = TrustView {
        local_device: local_descriptor.clone(),
        devices: vec![peer.clone()],
        relationships: vec![prepared.relationship.clone()],
        profile_grants: vec![
            prepared.existing_grant.clone(),
            prepared.joining_grant.clone(),
        ],
        grants: Vec::new(),
        revocations: Vec::new(),
        names: Vec::new(),
        builds: Vec::new(),
        lifecycles: std::collections::BTreeMap::new(),
        effective_revision: 1,
    };
    assert!(enrollment_completion_is_effective(
        &prepared,
        &view,
        profile,
        local_descriptor.device_id,
    ));
    view.revocations.push(koru_device::RevocationV1::sign(
        prepared.relationship.relationship_id,
        peer.device_id,
        local_descriptor.device_id,
        RecordId::new(),
        1_003_000,
        &local.signing_key,
    ));
    assert!(!enrollment_completion_is_effective(
        &prepared,
        &view,
        profile,
        local_descriptor.device_id,
    ));
}

#[test]
fn version_two_identity_database_migrates_the_enrollment_replay_ledger() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("agent.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE local_device (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), descriptor_json BLOB NOT NULL);
             CREATE TABLE trust_records (record_id TEXT PRIMARY KEY, kind TEXT NOT NULL, record_json BLOB NOT NULL);
             CREATE TABLE trust_meta (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), effective_revision INTEGER NOT NULL);
             CREATE TABLE pairing_commits (
                invitation_id TEXT PRIMARY KEY,
                peer_device_id TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('prepared', 'committed')),
                prepared_json BLOB NOT NULL
             );
             INSERT INTO trust_meta(singleton, effective_revision) VALUES(1, 0);
             PRAGMA user_version = 2;",
        )
        .unwrap();
    drop(connection);

    let repository = SqliteTrustRepository::open(&path).unwrap();
    let version: i64 = repository
        .connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 5);
    let table: String = repository
        .connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'enrollment_commits'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table, "enrollment_commits");
    let target: String = repository
        .connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'enrollment_unattributed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "enrollment_unattributed");
}

#[test]
fn repository_application_is_idempotent_and_collision_safe() {
    let root = tempfile::tempdir().unwrap();
    let identity_db = root.path().join("identity.db");
    let local = local_identity(&identity_db, 42);
    let mut repository = SqliteTrustRepository::open(&identity_db.clone()).unwrap();
    let record_id = RecordId::new();
    let relationship = RelationshipId::new();
    let peer = koru_device::DeviceId::new();
    let grant = AllDataGrantV1::sign(
        relationship,
        peer,
        local.descriptor.device_id,
        record_id,
        1,
        &local.signing_key,
    );
    let verified = grant
        .clone()
        .verify(local.descriptor.signing_public_key)
        .unwrap();
    assert_eq!(
        repository.apply(verified).unwrap(),
        ApplyOutcome::Applied {
            effective_revision: 1
        }
    );
    assert_eq!(
        repository
            .apply(grant.verify(local.descriptor.signing_public_key).unwrap())
            .unwrap(),
        ApplyOutcome::AlreadyApplied {
            effective_revision: 1
        }
    );
    let conflicting = AllDataGrantV1::sign(
        relationship,
        peer,
        local.descriptor.device_id,
        record_id,
        2,
        &local.signing_key,
    );
    assert_eq!(
        repository
            .apply(
                conflicting
                    .verify(local.descriptor.signing_public_key)
                    .unwrap()
            )
            .unwrap_err()
            .code,
        "record_collision"
    );
}

#[test]
fn prepared_pairing_is_inactive_until_one_atomic_commit() {
    let root = tempfile::tempdir().unwrap();
    let identity_db = root.path().join("identity.db");
    let local = local_identity(&identity_db, 42);
    let peer_key = koru_device::DeviceSigningKey::generate();
    let peer_endpoint = koru_device::ReplicationEndpointSecret::generate();
    let peer = koru_device::DeviceDescriptorV1::new(
        koru_device::DeviceId::new(),
        "Phone".into(),
        &peer_key,
        peer_endpoint.endpoint_id(),
        43,
    );
    let invitation = koru_pairing::PairingInvitationV1::new(local.descriptor.clone(), None);
    let transcript = koru_pairing::PairingTranscriptV1::new(&invitation, peer);
    let prepared = koru_pairing::PreparedPairingV1::sign_for_test(
        transcript,
        &local.signing_key,
        &peer_key,
        44,
    )
    .unwrap();
    let mut repository = SqliteTrustRepository::open(&identity_db.clone()).unwrap();
    assert_eq!(
        repository
            .prepare_pairing(&prepared, local.descriptor.device_id)
            .unwrap(),
        PairingPersistenceState::Prepared
    );
    let before = repository.trust_view().unwrap();
    assert!(before.relationships.is_empty());
    assert!(before.grants.is_empty());
    assert_eq!(before.effective_revision, 0);

    assert_eq!(
        repository
            .commit_pairing(&prepared, local.descriptor.device_id)
            .unwrap(),
        PairingPersistenceState::Committed
    );
    let after = repository.trust_view().unwrap();
    assert_eq!(after.relationships.len(), 1);
    assert_eq!(after.grants.len(), 2);
    assert_eq!(after.effective_revision, 1);
    assert_eq!(
        repository
            .commit_pairing(&prepared, local.descriptor.device_id)
            .unwrap(),
        PairingPersistenceState::Committed
    );
    assert_eq!(repository.trust_view().unwrap().effective_revision, 1);
}

#[test]
fn enrollment_commit_is_atomic_and_replay_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let identity_db = root.path().join("identity.db");
    let local = local_identity(&identity_db, 42);
    let (attestation, prepared) = enrollment(&local);
    let mut repository = SqliteTrustRepository::open(&identity_db.clone()).unwrap();
    assert_eq!(
        repository
            .commit_enrollment(&prepared, &attestation, local.descriptor.device_id)
            .unwrap(),
        ApplyOutcome::Applied {
            effective_revision: 1
        }
    );
    let view = repository.trust_view().unwrap();
    assert_eq!(view.relationships.len(), 1);
    assert_eq!(view.profile_grants.len(), 2);
    assert!(view.grants.is_empty());
    assert_eq!(
        repository
            .commit_enrollment(&prepared, &attestation, local.descriptor.device_id)
            .unwrap(),
        ApplyOutcome::AlreadyApplied {
            effective_revision: 1
        }
    );

    let peer_root = tempfile::tempdir().unwrap();
    let peer_db = peer_root.path().join("agent.db");
    let mut peer_repository = SqliteTrustRepository::open(&peer_db).unwrap();
    peer_repository
        .initialize(&prepared.transcript.joining_device)
        .unwrap();
    assert!(matches!(
        peer_repository
            .commit_enrollment(
                &prepared,
                &attestation,
                prepared.transcript.joining_device.device_id,
            )
            .unwrap(),
        ApplyOutcome::Applied { .. }
    ));
    let peer_view = peer_repository.trust_view().unwrap();
    assert_eq!(peer_view.relationships, view.relationships);
    assert_eq!(peer_view.profile_grants, view.profile_grants);
}

#[test]
fn concurrent_duplicate_enrollment_has_one_durable_result() {
    let root = tempfile::tempdir().unwrap();
    let identity_db = root.path().join("identity.db");
    let local = local_identity(&identity_db, 42);
    let (attestation, prepared) = enrollment(&local);
    let local_id = local.descriptor.device_id;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let db_path = identity_db.clone();
    let results = std::thread::scope(|scope| {
        let handles = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let prepared = prepared.clone();
                let attestation = attestation.clone();
                let db_path = db_path.clone();
                scope.spawn(move || {
                    let mut repository = SqliteTrustRepository::open(&db_path).unwrap();
                    barrier.wait();
                    repository.commit_enrollment(&prepared, &attestation, local_id)
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, ApplyOutcome::Applied { .. }))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, ApplyOutcome::AlreadyApplied { .. }))
            .count(),
        1
    );
    let repository = SqliteTrustRepository::open(&db_path).unwrap();
    let view = repository.trust_view().unwrap();
    assert_eq!(view.relationships.len(), 1);
    assert_eq!(view.profile_grants.len(), 2);
    assert_eq!(view.effective_revision, 1);
}

#[test]
fn attestation_jti_cannot_be_replayed_into_a_second_relationship() {
    let root = tempfile::tempdir().unwrap();
    let identity_db = root.path().join("identity.db");
    let local = local_identity(&identity_db, 42);
    let (first_attestation, first) = enrollment(&local);
    let (second_attestation, second) = enrollment(&local);
    assert_eq!(
        first.transcript.attestation_jti,
        second.transcript.attestation_jti
    );
    assert_ne!(
        first.transcript.relationship_id,
        second.transcript.relationship_id
    );
    let mut repository = SqliteTrustRepository::open(&identity_db.clone()).unwrap();
    repository
        .commit_enrollment(&first, &first_attestation, local.descriptor.device_id)
        .unwrap();
    assert_eq!(
        repository
            .commit_enrollment(&second, &second_attestation, local.descriptor.device_id)
            .unwrap_err()
            .code,
        "enrollment_collision"
    );
    let view = repository.trust_view().unwrap();
    assert_eq!(view.relationships.len(), 1);
    assert_eq!(view.profile_grants.len(), 2);
    assert_eq!(view.effective_revision, 1);
}

#[test]
fn enrollment_record_collision_rolls_back_the_whole_bundle() {
    let root = tempfile::tempdir().unwrap();
    let identity_db = root.path().join("identity.db");
    let local = local_identity(&identity_db, 42);
    let (attestation, prepared) = enrollment(&local);
    let unrelated_key = koru_device::DeviceSigningKey::generate();
    let unrelated_endpoint = koru_device::ReplicationEndpointSecret::generate();
    let unrelated = koru_device::DeviceDescriptorV1::new(
        koru_device::DeviceId::new(),
        "Other".into(),
        &unrelated_key,
        unrelated_endpoint.endpoint_id(),
        50,
    );
    let collision = koru_device::TrustRelationshipV1::sign(
        prepared.transcript.relationship_id,
        local.descriptor.clone(),
        unrelated,
        [99; 32],
        51,
        &local.signing_key,
        &unrelated_key,
    );
    let mut repository = SqliteTrustRepository::open(&identity_db.clone()).unwrap();
    repository.apply(collision.verify().unwrap()).unwrap();
    assert_eq!(
        repository
            .commit_enrollment(&prepared, &attestation, local.descriptor.device_id)
            .unwrap_err()
            .code,
        "record_collision"
    );
    let view = repository.trust_view().unwrap();
    assert_eq!(
        view.relationships.len(),
        1,
        "only the seeded collision remains"
    );
    assert!(view.profile_grants.is_empty());
    let commits: u64 = repository
        .connection
        .query_row("SELECT count(*) FROM enrollment_commits", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(commits, 0);
}

#[test]
fn profile_lifecycle_repository_is_idempotent_ordered_and_restart_safe() {
    let root = tempfile::tempdir().unwrap();
    let identity_db = root.path().join("identity.db");
    let local = local_identity(&identity_db, 42);
    let profile = koru_store_core::ProfileId::new();
    let operation = uuid::Uuid::new_v4();
    let signed = |record_id, state, revision, operation| {
        koru_device::ProfileLifecycleV1::sign(
            record_id,
            local.descriptor.device_id,
            profile,
            state,
            revision,
            operation,
            50,
            &local.signing_key,
        )
    };
    let first = signed(
        RecordId::new(),
        koru_device::ProfileLifecycleStateV1::Deleted,
        3,
        operation,
    );
    let mut repository = SqliteTrustRepository::open(&identity_db.clone()).unwrap();
    assert_eq!(
        repository
            .apply(
                first
                    .clone()
                    .verify(local.descriptor.signing_public_key)
                    .unwrap()
            )
            .unwrap(),
        ApplyOutcome::Applied {
            effective_revision: 1
        }
    );

    let duplicate = signed(
        RecordId::new(),
        koru_device::ProfileLifecycleStateV1::Deleted,
        3,
        operation,
    );
    assert_eq!(
        repository
            .apply(
                duplicate
                    .verify(local.descriptor.signing_public_key)
                    .unwrap()
            )
            .unwrap(),
        ApplyOutcome::AlreadyApplied {
            effective_revision: 1
        }
    );
    let stale = signed(
        RecordId::new(),
        koru_device::ProfileLifecycleStateV1::Deleted,
        2,
        uuid::Uuid::new_v4(),
    );
    assert_eq!(
        repository
            .apply(stale.verify(local.descriptor.signing_public_key).unwrap())
            .unwrap(),
        ApplyOutcome::Stale {
            effective_revision: 1
        }
    );
    let conflict = signed(
        RecordId::new(),
        koru_device::ProfileLifecycleStateV1::Active,
        3,
        uuid::Uuid::new_v4(),
    );
    assert_eq!(
        repository
            .apply(
                conflict
                    .verify(local.descriptor.signing_public_key)
                    .unwrap()
            )
            .unwrap_err()
            .code,
        "profile_lifecycle_conflict"
    );

    let restored = signed(
        RecordId::new(),
        koru_device::ProfileLifecycleStateV1::Active,
        4,
        uuid::Uuid::new_v4(),
    );
    repository
        .apply(
            restored
                .clone()
                .verify(local.descriptor.signing_public_key)
                .unwrap(),
        )
        .unwrap();
    drop(repository);

    let repository = SqliteTrustRepository::open(&identity_db.clone()).unwrap();
    let view = repository.trust_view().unwrap();
    assert_eq!(view.lifecycles.get(&profile), Some(&restored));
    assert_eq!(view.effective_revision, 2);
    let lifecycle_rows: u64 = repository
        .connection
        .query_row(
            "SELECT count(*) FROM trust_records WHERE kind = 'profile_lifecycle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(lifecycle_rows, 1, "only the highest revision is retained");
}
