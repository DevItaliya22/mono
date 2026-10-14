// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_device::{
    DeviceSigningKey, ReplicationEndpointSecret, StoredLocalDevice, TrustRepository,
};

// Frozen Plan 1.9 schema 4. Do not replace this with the current creation path:
// a migration test must begin with the schema an existing installation owns.
const SCHEMA_4: &str = "
CREATE TABLE local_device (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), descriptor_json BLOB NOT NULL);
CREATE TABLE trust_records (record_id TEXT PRIMARY KEY, kind TEXT NOT NULL, record_json BLOB NOT NULL);
CREATE TABLE trust_meta (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), effective_revision INTEGER NOT NULL);
CREATE TABLE pairing_commits (
    invitation_id TEXT PRIMARY KEY,
    peer_device_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('prepared', 'committed')),
    prepared_json BLOB NOT NULL
);
CREATE TABLE enrollment_commits (
    attestation_jti TEXT PRIMARY KEY,
    peer_device_id TEXT NOT NULL,
    prepared_json BLOB NOT NULL
);
CREATE TABLE enrollment_target (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    profile_id TEXT NOT NULL
);
INSERT INTO trust_meta(singleton, effective_revision) VALUES(1, 0);
PRAGMA user_version = 4;
";

fn device(seed: u8) -> StoredLocalDevice {
    let signing_key = DeviceSigningKey::from_secret_bytes([seed; 32]);
    let endpoint_secret = ReplicationEndpointSecret::from_secret_bytes([seed + 1; 32]);
    let descriptor = DeviceDescriptorV1::new(
        DeviceId::parse(&format!("40000000-0000-4000-8000-{seed:012}")).unwrap(),
        format!("Device {seed}"),
        &signing_key,
        endpoint_secret.endpoint_id(),
        42,
    );
    StoredLocalDevice {
        descriptor,
        signing_key,
        endpoint_secret,
    }
}

fn signed_enrollment(
    local: &StoredLocalDevice,
    peer: &StoredLocalDevice,
) -> (
    koru_account_session::VerifiedDeviceAttestationV1,
    koru_pairing::PreparedEnrollmentV1,
) {
    let ring = koru_account_session::DeviceAttestationKeyRing::new(
        "migration-fixture".into(),
        [(
            "migration-fixture".into(),
            ed25519_dalek::SigningKey::from_bytes(&[61; 32]),
        )],
    )
    .unwrap();
    let signed = ring
        .sign(&koru_account_session::DeviceAttestationClaimsV1 {
            version: koru_account_session::DEVICE_ATTESTATION_VERSION,
            issuer: "https://account.example.invalid".into(),
            audience: koru_account_session::DEVICE_ATTESTATION_AUDIENCE.into(),
            koru_identity_id: owner(1, 1).account,
            device_id: peer.descriptor.device_id,
            signing_public_key: peer.descriptor.signing_public_key,
            endpoint_id: peer.descriptor.endpoint_id,
            authorized_profile_ids: vec![profile(7)],
            session_id: "50000000-0000-4000-8000-000000000005".parse().unwrap(),
            token_id: "migration-enrollment-replay".into(),
            issued_at_millis: 1_000_000,
            expires_at_millis: 1_120_000,
        })
        .unwrap();
    let attestation = koru_account_session::verify_device_attestation(
        &signed,
        &ring.verification_keys(),
        "https://account.example.invalid",
        1_001_000,
    )
    .unwrap();
    let transcript = koru_pairing::EnrollmentTranscriptV1::new(
        &attestation,
        profile(7),
        local.descriptor.clone(),
        peer.descriptor.clone(),
        [71; 32],
    )
    .unwrap();
    let join = koru_pairing::EnrollmentJoinV1::sign(
        signed,
        &attestation,
        transcript,
        &peer.signing_key,
        1_002_000,
    )
    .unwrap();
    let prepared = join.complete(&attestation, &local.signing_key).unwrap();
    (attestation, prepared)
}

fn table_rows(connection: &Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns).map(|column| row.get(column)).collect()
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn schema(connection: &Connection) -> Vec<(String, String, String, String)> {
    let mut statement = connection.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name").unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, String>(3)?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn owner(account: u8, auth: u8) -> AccountOwner {
    AccountOwner {
        account: format!("10000000-0000-4000-8000-{account:012}")
            .parse()
            .unwrap(),
        auth_identity: format!("20000000-0000-4000-8000-{auth:012}")
            .parse()
            .unwrap(),
    }
}
fn profile(value: u8) -> ProfileId {
    ProfileId::parse(&format!("30000000-0000-4000-8000-{value:012}")).unwrap()
}
fn ready(
    repository: &mut SqliteTrustRepository,
    namespace: &str,
    owner: AccountOwner,
) -> DirectoryRecord {
    let record = repository.begin_account_write(namespace, owner).unwrap();
    repository
        .finish_account_write(namespace, &record, owner, true, false)
        .unwrap();
    record
}

#[test]
fn fresh_and_upgraded_schema_are_identical_and_legacy_target_is_unattributed() {
    let fresh = SqliteTrustRepository::in_memory().unwrap();
    let legacy = Connection::open_in_memory().unwrap();
    legacy.execute_batch(SCHEMA_4).unwrap();
    let mut old = SqliteTrustRepository { connection: legacy };
    let local = device(1);
    let paired = device(3);
    let enrolled = device(5);
    old.initialize(&local.descriptor).unwrap();
    let invitation = koru_pairing::PairingInvitationV1::new(local.descriptor.clone(), None);
    let transcript = koru_pairing::PairingTranscriptV1::new(&invitation, paired.descriptor.clone());
    let pairing = koru_pairing::PreparedPairingV1::sign_for_test(
        transcript,
        &local.signing_key,
        &paired.signing_key,
        44,
    )
    .unwrap();
    old.prepare_pairing(&pairing, local.descriptor.device_id)
        .unwrap();
    old.commit_pairing(&pairing, local.descriptor.device_id)
        .unwrap();
    let (attestation, enrollment) = signed_enrollment(&local, &enrolled);
    old.commit_enrollment(&enrollment, &attestation, local.descriptor.device_id)
        .unwrap();
    old.connection
        .execute(
            "UPDATE trust_meta SET effective_revision = 41 WHERE singleton = 1",
            [],
        )
        .unwrap();
    old.connection
        .execute(
            "INSERT INTO enrollment_target VALUES(1, ?1)",
            [profile(1).to_storage()],
        )
        .unwrap();
    let old_schema = schema(&old.connection);
    assert_eq!(old_schema.len(), 6);
    let preserved = [
        "local_device",
        "trust_records",
        "trust_meta",
        "pairing_commits",
        "enrollment_commits",
    ];
    let before: Vec<_> = preserved
        .iter()
        .map(|table| table_rows(&old.connection, table))
        .collect();
    assert_eq!(
        before[1].len(),
        6,
        "each ceremony stores one relationship and two reciprocal grants"
    );
    let target = table_rows(&old.connection, "enrollment_target");
    let mut upgraded = SqliteTrustRepository::from_connection(old.connection).unwrap();
    for repository in [&fresh, &upgraded] {
        assert_eq!(
            repository
                .connection
                .pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            5
        );
        assert!(
            repository
                .account_directory(&"a".repeat(64))
                .unwrap()
                .records
                .is_empty()
        );
    }
    assert_eq!(upgraded.enrollment_target().unwrap(), Some(profile(1)));
    assert_eq!(schema(&fresh.connection), schema(&upgraded.connection));
    assert_eq!(schema(&upgraded.connection).len(), 9);
    for (table, rows) in preserved.iter().zip(before.iter()) {
        assert_eq!(
            &table_rows(&upgraded.connection, table),
            rows,
            "all original fields and bytes: {table}"
        );
        let before_ddl = old_schema.iter().find(|row| row.1 == *table).unwrap();
        assert_eq!(
            schema(&upgraded.connection)
                .iter()
                .find(|row| row.1 == *table),
            Some(before_ddl)
        );
    }
    assert_eq!(
        table_rows(&upgraded.connection, "enrollment_unattributed"),
        target
    );
    assert_eq!(
        upgraded.local_device().unwrap(),
        Some(local.descriptor.clone())
    );
    assert_eq!(upgraded.revision().unwrap(), 41);
    assert_eq!(
        upgraded.trust_view().unwrap().grants.len(),
        2,
        "QR all-data grants remain"
    );
    assert_eq!(upgraded.trust_view().unwrap().profile_grants.len(), 2);
    assert_eq!(
        upgraded
            .prepare_pairing(&pairing, local.descriptor.device_id)
            .unwrap(),
        super::super::PairingPersistenceState::Committed
    );
    assert_eq!(
        upgraded
            .commit_pairing(&pairing, local.descriptor.device_id)
            .unwrap(),
        super::super::PairingPersistenceState::Committed
    );
    assert!(matches!(
        upgraded
            .commit_enrollment(&enrollment, &attestation, local.descriptor.device_id)
            .unwrap(),
        ApplyOutcome::AlreadyApplied {
            effective_revision: 41
        }
    ));
    assert_eq!(
        upgraded.revision().unwrap(),
        41,
        "replay must not advance the preserved revision"
    );
    assert!(
        upgraded.import_legacy(&local.descriptor, &[], 0).is_err(),
        "the original initialization marker still prevents import"
    );
    let upgraded = SqliteTrustRepository::from_connection(upgraded.connection).unwrap();
    assert_eq!(schema(&fresh.connection), schema(&upgraded.connection));
    for (table, rows) in preserved.iter().zip(before.iter()) {
        assert_eq!(&table_rows(&upgraded.connection, table), rows);
    }
}

#[test]
fn pending_replacement_preserves_targets_and_actual_auth_identity_repairs_it() {
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    let namespace = "a".repeat(64);
    let first = ready(&mut repository, &namespace, owner(1, 1));
    repository
        .replace_account_targets(
            &namespace,
            first.owner,
            first.version,
            &BTreeSet::new(),
            &BTreeSet::from([profile(1), profile(2)]),
        )
        .unwrap();
    let replacement = repository
        .begin_account_write(&namespace, owner(1, 2))
        .unwrap();
    assert!(replacement.version > first.version);
    assert!(
        repository
            .replace_account_targets(
                &namespace,
                first.owner,
                first.version,
                &BTreeSet::from([profile(1), profile(2)]),
                &BTreeSet::new()
            )
            .is_err()
    );
    repository
        .finish_account_write(&namespace, &replacement, first.owner, true, false)
        .unwrap();
    let committed = table_rows(&repository.connection, "account_directory");
    let metadata = table_rows(&repository.connection, "account_namespace");
    // The ready transaction committed, then its acknowledgement was lost.
    // The attempted B record remains the retry token, while the actual slot
    // still authenticates A. Retrying must not require the attempted Auth ID.
    repository
        .finish_account_write(&namespace, &replacement, first.owner, true, false)
        .unwrap();
    assert_eq!(
        table_rows(&repository.connection, "account_directory"),
        committed
    );
    assert_eq!(
        table_rows(&repository.connection, "account_namespace"),
        metadata
    );
    assert!(
        repository
            .finish_account_write(&namespace, &replacement, owner(1, 3), true, false)
            .is_err()
    );
    assert!(
        repository
            .finish_account_write(&namespace, &replacement, owner(2, 1), true, false)
            .is_err()
    );
    assert_eq!(
        repository.account_directory(&namespace).unwrap().records[0].owner,
        first.owner
    );
    assert_eq!(
        repository
            .account_targets(&namespace, first.owner.account)
            .unwrap(),
        BTreeSet::from([profile(1), profile(2)])
    );
}

#[test]
fn late_ack_cannot_recreate_removed_account_and_retirement_is_independent() {
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    let namespace = "a".repeat(64);
    let record = ready(&mut repository, &namespace, owner(1, 1));
    repository
        .finish_account_write(&namespace, &record, record.owner, true, true)
        .unwrap();
    repository.remove_account(&namespace, &record).unwrap();
    assert_eq!(
        repository.account_directory(&namespace).unwrap().active,
        None
    );
    assert!(
        repository
            .finish_account_write(&namespace, &record, record.owner, true, false)
            .is_err()
    );
    let replacement = repository
        .begin_account_write(&namespace, owner(1, 1))
        .unwrap();
    assert!(replacement.version > record.version);
    assert!(
        repository
            .finish_account_write(&namespace, &record, record.owner, true, false)
            .is_err()
    );
    assert!(
        repository
            .account_directory(&namespace)
            .unwrap()
            .legacy_retired
    );
    assert!(
        !repository
            .account_directory(&"b".repeat(64))
            .unwrap()
            .legacy_retired
    );
    repository.remove_account(&namespace, &replacement).unwrap();
    let again = repository
        .begin_account_write(&namespace, owner(1, 1))
        .unwrap();
    assert!(again.version > replacement.version);
    let empty_namespace = "c".repeat(64);
    repository
        .retire_legacy_credential(&empty_namespace)
        .unwrap();
    repository
        .retire_legacy_credential(&empty_namespace)
        .unwrap();
    let empty = repository.account_directory(&empty_namespace).unwrap();
    assert!(empty.records.is_empty());
    assert!(empty.legacy_retired);
    assert_eq!(empty.active, None);
}

#[test]
fn account_sets_namespace_and_claim_are_atomic_and_independent() {
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    let namespace = "a".repeat(64);
    let other = "b".repeat(64);
    let a = ready(&mut repository, &namespace, owner(1, 1));
    let b = ready(&mut repository, &namespace, owner(2, 2));
    let c = ready(&mut repository, &other, owner(1, 3));
    for (scope, record) in [(&namespace, &a), (&namespace, &b), (&other, &c)] {
        repository
            .replace_account_targets(
                scope,
                record.owner,
                record.version,
                &BTreeSet::new(),
                &BTreeSet::from([profile(1), profile(2)]),
            )
            .unwrap();
    }
    repository
        .replace_account_targets(
            &namespace,
            a.owner,
            a.version,
            &BTreeSet::from([profile(1), profile(2)]),
            &BTreeSet::from([profile(2)]),
        )
        .unwrap();
    assert_eq!(
        repository
            .account_targets(&namespace, b.owner.account)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        repository
            .account_targets(&other, c.owner.account)
            .unwrap()
            .len(),
        2
    );
    assert!(
        repository
            .replace_account_targets(
                &namespace,
                a.owner,
                a.version,
                &BTreeSet::new(),
                &BTreeSet::new()
            )
            .is_err()
    );
    repository.set_enrollment_target(Some(profile(3))).unwrap();
    repository.connection.execute_batch("CREATE TRIGGER reject_target BEFORE INSERT ON enrollment_targets BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(
        repository
            .claim_unattributed_target(&namespace, a.owner, a.version, profile(3))
            .is_err()
    );
    assert_eq!(repository.enrollment_target().unwrap(), Some(profile(3)));
    assert!(
        repository
            .replace_account_targets(
                &namespace,
                a.owner,
                a.version,
                &BTreeSet::from([profile(2)]),
                &BTreeSet::from([profile(4)])
            )
            .is_err()
    );
    assert_eq!(
        repository
            .account_targets(&namespace, a.owner.account)
            .unwrap(),
        BTreeSet::from([profile(2)])
    );
    repository
        .connection
        .execute_batch("DROP TRIGGER reject_target;")
        .unwrap();
    // Fail after DELETE and one successful INSERT, proving whole-set rollback.
    repository.connection.execute_batch(&format!("CREATE TRIGGER reject_second_target BEFORE INSERT ON enrollment_targets WHEN NEW.profile_id = '{}' BEGIN SELECT RAISE(ABORT, 'injected'); END;", profile(4).to_storage())).unwrap();
    let all_targets = table_rows(&repository.connection, "enrollment_targets");
    assert!(
        repository
            .replace_account_targets(
                &namespace,
                a.owner,
                a.version,
                &BTreeSet::from([profile(2)]),
                &BTreeSet::from([profile(3), profile(4)])
            )
            .is_err()
    );
    assert_eq!(
        table_rows(&repository.connection, "enrollment_targets"),
        all_targets
    );
    repository
        .connection
        .execute_batch("DROP TRIGGER reject_second_target;")
        .unwrap();
    assert!(
        repository
            .claim_unattributed_target(&namespace, a.owner, a.version, profile(3))
            .unwrap()
    );
    assert_eq!(repository.enrollment_target().unwrap(), None);
    repository.remove_account(&namespace, &a).unwrap();
    assert_eq!(
        repository
            .account_targets(&namespace, b.owner.account)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        repository
            .account_targets(&other, c.owner.account)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn target_cas_and_unattributed_claim_reject_same_owner_same_set_after_recreation() {
    for selected in [BTreeSet::new(), BTreeSet::from([profile(1)])] {
        let mut repository = SqliteTrustRepository::in_memory().unwrap();
        let namespace = "a".repeat(64);
        let old = ready(&mut repository, &namespace, owner(1, 1));
        repository
            .replace_account_targets(
                &namespace,
                old.owner,
                old.version,
                &BTreeSet::new(),
                &selected,
            )
            .unwrap();
        repository.remove_account(&namespace, &old).unwrap();
        // Cleanup after removal is idempotent even if the caller captured a
        // nonempty set. It must not require recreating an account row.
        repository
            .replace_account_targets(
                &namespace,
                old.owner,
                old.version,
                &selected,
                &BTreeSet::new(),
            )
            .unwrap();
        let current = ready(&mut repository, &namespace, old.owner);
        assert!(current.version > old.version);
        repository
            .replace_account_targets(
                &namespace,
                current.owner,
                current.version,
                &BTreeSet::new(),
                &selected,
            )
            .unwrap();
        repository.set_enrollment_target(Some(profile(3))).unwrap();
        let rows = table_rows(&repository.connection, "account_directory");
        let metadata = table_rows(&repository.connection, "account_namespace");
        let targets = table_rows(&repository.connection, "enrollment_targets");
        // Identical owner and expected set cannot authorize a stale clear or
        // a stale replacement, including an empty-to-empty operation.
        for next in [BTreeSet::new(), BTreeSet::from([profile(2)])] {
            assert!(
                repository
                    .replace_account_targets(&namespace, old.owner, old.version, &selected, &next,)
                    .is_err()
            );
        }
        assert!(
            repository
                .claim_unattributed_target(&namespace, old.owner, old.version, profile(3))
                .is_err()
        );
        assert_eq!(
            table_rows(&repository.connection, "account_directory"),
            rows
        );
        assert_eq!(
            table_rows(&repository.connection, "account_namespace"),
            metadata
        );
        assert_eq!(
            table_rows(&repository.connection, "enrollment_targets"),
            targets
        );
        assert_eq!(repository.enrollment_target().unwrap(), Some(profile(3)));
        // The current generation can perform the exact same requested effects.
        repository
            .replace_account_targets(
                &namespace,
                current.owner,
                current.version,
                &selected,
                &BTreeSet::from([profile(2)]),
            )
            .unwrap();
        assert!(
            repository
                .claim_unattributed_target(&namespace, current.owner, current.version, profile(3))
                .unwrap()
        );
        assert_eq!(repository.enrollment_target().unwrap(), None);
        assert_eq!(
            repository
                .account_targets(&namespace, current.owner.account)
                .unwrap(),
            BTreeSet::from([profile(2), profile(3)])
        );
    }
}

#[test]
fn refresh_target_validation_is_read_only_and_requires_a_current_ready_row_even_for_empty_set() {
    for targets in [BTreeSet::new(), BTreeSet::from([profile(1)])] {
        let mut repository = SqliteTrustRepository::in_memory().unwrap();
        let namespace = "a".repeat(64);
        let original = ready(&mut repository, &namespace, owner(1, 1));
        repository
            .replace_account_targets(
                &namespace,
                original.owner,
                original.version,
                &BTreeSet::new(),
                &targets,
            )
            .unwrap();
        let before: Vec<_> = [
            "account_namespace",
            "account_directory",
            "enrollment_targets",
        ]
        .iter()
        .map(|table| table_rows(&repository.connection, table))
        .collect();
        repository
            .validate_account_targets(&namespace, original.owner, original.version, &targets)
            .unwrap();
        assert!(
            repository
                .validate_account_targets(
                    &namespace,
                    original.owner,
                    original.version,
                    &BTreeSet::from([profile(2)])
                )
                .is_err()
        );
        for (table, expected) in [
            "account_namespace",
            "account_directory",
            "enrollment_targets",
        ]
        .iter()
        .zip(before)
        {
            assert_eq!(table_rows(&repository.connection, table), expected);
        }
        repository.remove_account(&namespace, &original).unwrap();
        assert!(
            repository
                .validate_account_targets(
                    &namespace,
                    original.owner,
                    original.version,
                    &BTreeSet::new()
                )
                .is_err()
        );
        let replacement = ready(&mut repository, &namespace, original.owner);
        repository
            .replace_account_targets(
                &namespace,
                replacement.owner,
                replacement.version,
                &BTreeSet::new(),
                &targets,
            )
            .unwrap();
        assert!(
            repository
                .validate_account_targets(&namespace, original.owner, original.version, &targets)
                .is_err()
        );
        repository
            .validate_account_targets(&namespace, replacement.owner, replacement.version, &targets)
            .unwrap();
    }
}

#[test]
fn selection_accepts_pending_discovery_without_crossing_namespace_or_granting_authority() {
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    let namespace = "a".repeat(64);
    let other = "b".repeat(64);
    let a = ready(&mut repository, &namespace, owner(1, 1));
    let b = ready(&mut repository, &other, owner(2, 2));
    let pending = repository
        .begin_account_write(&namespace, owner(3, 3))
        .unwrap();
    assert_eq!(
        repository.account_directory(&namespace).unwrap().active,
        Some(a.owner.account)
    );
    assert_eq!(
        repository.account_directory(&other).unwrap().active,
        Some(b.owner.account)
    );
    repository
        .activate_account(&namespace, pending.owner.account)
        .unwrap();
    assert_eq!(
        repository.account_directory(&namespace).unwrap().active,
        Some(pending.owner.account)
    );
    assert!(
        repository
            .validate_account_targets(
                &namespace,
                pending.owner,
                pending.version,
                &std::collections::BTreeSet::new()
            )
            .is_err()
    );
    assert!(
        repository
            .activate_account(&namespace, b.owner.account)
            .is_err()
    );
    let namespaces = table_rows(&repository.connection, "account_namespace");
    {
        let transaction = repository.connection.transaction().unwrap();
        transaction
            .execute(
                "UPDATE account_namespace SET active_account_id = ?2 WHERE namespace = ?1",
                params![namespace, b.owner.account.to_string()],
            )
            .unwrap();
        assert!(
            transaction.commit().is_err(),
            "deferred FK still fences the exact namespace at commit"
        );
    }
    assert_eq!(
        table_rows(&repository.connection, "account_namespace"),
        namespaces
    );
    assert!(repository.connection.execute("INSERT INTO enrollment_targets(namespace, account_id, profile_id) VALUES(?1, ?2, ?3)", params![namespace, b.owner.account.to_string(), profile(1).to_storage()]).is_err());
    assert!(
        repository
            .connection
            .execute(
                "DELETE FROM account_directory WHERE namespace = ?1 AND account_id = ?2",
                params![namespace, pending.owner.account.to_string()]
            )
            .is_err(),
        "active-pointer FK forbids an uncoordinated delete"
    );
    repository.remove_account(&namespace, &pending).unwrap();
    assert_eq!(
        repository.account_directory(&namespace).unwrap().active,
        None
    );
    assert_eq!(
        repository.account_directory(&other).unwrap().active,
        Some(b.owner.account)
    );
}
