// SPDX-License-Identifier: MPL-2.0

use super::*;

fn identity() -> DeviceDescriptorV1 {
    let key = koru_device::DeviceSigningKey::from_secret_bytes([7; 32]);
    let endpoint = koru_device::ReplicationEndpointSecret::from_secret_bytes([8; 32]);
    DeviceDescriptorV1::new(
        DeviceId::new(),
        "Phone".into(),
        &key,
        endpoint.endpoint_id(),
        1,
    )
}

#[test]
fn failed_transaction_leaves_no_marker_or_records_and_can_retry() {
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    let local = identity();
    let record = TrustRecord::DeviceName(koru_device::DeviceNameV1::sign(
        koru_device::RecordId::new(),
        local.device_id,
        "Renamed".into(),
        2,
        &koru_device::DeviceSigningKey::from_secret_bytes([7; 32]),
    ));
    repository
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER interrupt_import BEFORE UPDATE ON trust_meta
         BEGIN SELECT RAISE(ABORT, 'injected interruption'); END;",
        )
        .unwrap();
    assert!(
        repository
            .import_legacy(&local, &[record.clone()], 42)
            .is_err()
    );
    assert!(repository.local_device().unwrap().is_none());
    assert!(repository.records().unwrap().is_empty());
    assert_eq!(repository.revision().unwrap(), 0);
    repository
        .connection
        .execute_batch("DROP TRIGGER interrupt_import")
        .unwrap();
    repository
        .import_legacy(&local, &[record.clone()], 42)
        .unwrap();
    assert_eq!(repository.local_device().unwrap(), Some(local));
    assert_eq!(repository.records().unwrap(), vec![record]);
    assert_eq!(repository.revision().unwrap(), 42);
    assert_eq!(
        repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
}

#[test]
fn initialized_or_unmarked_nonempty_repository_is_never_overwritten() {
    let local = identity();
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    repository.import_legacy(&local, &[], 20).unwrap();
    assert!(repository.import_legacy(&local, &[], 0).is_err());
    assert_eq!(repository.revision().unwrap(), 20);
    let mut partial = SqliteTrustRepository::in_memory().unwrap();
    partial
        .connection
        .execute("UPDATE trust_meta SET effective_revision = 1", [])
        .unwrap();
    assert!(partial.import_legacy(&local, &[], 20).is_err());
    assert!(partial.local_device().unwrap().is_none());
    assert_eq!(partial.revision().unwrap(), 1);
}

#[test]
fn unrepresentable_revision_and_unknown_records_never_initialize() {
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    assert!(
        repository
            .import_legacy(&identity(), &[], u64::MAX)
            .is_err()
    );
    assert!(
        repository
            .import_legacy(&identity(), &[TrustRecord::Unknown], 1)
            .is_err()
    );
    assert!(repository.local_device().unwrap().is_none());
    assert!(repository.records().unwrap().is_empty());
    assert_eq!(repository.revision().unwrap(), 0);
}
