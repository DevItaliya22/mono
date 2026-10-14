// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn explicit_root_contains_database_and_secrets() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let identity = IdentityManager::open(&paths, 42).unwrap();
    let secrets = ProtectedSecretStore::new(paths.secrets_dir());
    assert!(paths.identity_db().exists());
    assert!(secrets.path(SecretName::DeviceSigning).exists());
    assert!(secrets.path(SecretName::ReplicationEndpoint).exists());
    assert_eq!(identity.descriptor.created_at_millis, 42);
    assert_eq!(
        Device::open(&paths.device_file(), 99).unwrap().id,
        identity.descriptor.device_id.to_storage()
    );
}

#[test]
fn identity_is_stable_and_missing_secret_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let first = IdentityManager::open(&paths, 42).unwrap();
    let first_id = first.descriptor.device_id;
    let first_endpoint = first.descriptor.endpoint_id;
    let second = IdentityManager::open(&paths, 99).unwrap();
    assert_eq!(second.descriptor.device_id, first_id);
    assert_eq!(second.descriptor.endpoint_id, first_endpoint);
    let secrets = ProtectedSecretStore::new(paths.secrets_dir());
    std::fs::remove_file(secrets.path(SecretName::DeviceSigning)).unwrap();
    let error = IdentityManager::open(&paths, 100).unwrap_err();
    assert_eq!(error.code, "identity_unavailable");
    assert_eq!(
        SqliteTrustRepository::open(&paths.identity_db())
            .unwrap()
            .local_device()
            .unwrap()
            .unwrap()
            .device_id,
        first_id
    );
}

#[test]
fn corrupt_ciphertext_is_never_replaced() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    IdentityManager::open(&paths, 42).unwrap();
    let path = ProtectedSecretStore::new(paths.secrets_dir()).path(SecretName::DeviceSigning);
    std::fs::write(&path, b"not dpapi").unwrap();
    assert!(IdentityManager::open(&paths, 43).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"not dpapi");
}

#[test]
fn protected_round_trip_and_refuses_replacement() {
    let root = tempfile::tempdir().unwrap();
    let store = ProtectedSecretStore::new(root.path().to_owned());
    store
        .store_new(SecretName::DeviceSigning, &SecretBytes([9; 32]))
        .unwrap();
    assert_eq!(
        store.load(SecretName::DeviceSigning).unwrap().unwrap().0,
        [9; 32]
    );
    assert_eq!(
        store
            .store_new(SecretName::DeviceSigning, &SecretBytes([8; 32]))
            .unwrap_err()
            .code,
        "secret_exists"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_stored_secret_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("secrets");
    let store = ProtectedSecretStore::new(directory.clone());
    store
        .store_new(SecretName::DeviceSigning, &SecretBytes([9; 32]))
        .unwrap();
    let file_mode = std::fs::metadata(store.path(SecretName::DeviceSigning))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(file_mode & 0o777, 0o600, "the file is owner-only");
    let dir_mode = std::fs::metadata(&directory).unwrap().permissions().mode();
    assert_eq!(dir_mode & 0o777, 0o700, "the directory is the guarantee");
}

/// A working identity is what every linked device trusts, so a reset of one
/// is refused and nothing moves.
#[test]
fn an_intact_identity_is_not_reset() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let before = IdentityManager::open(&paths, 42).unwrap();
    let error = IdentityManager::reset(&paths, 43).unwrap_err();
    assert_eq!(error.code, "identity_intact");
    assert_eq!(
        IdentityManager::open(&paths, 44)
            .unwrap()
            .descriptor
            .device_id,
        before.descriptor.device_id
    );
}

/// **Keys that no longer match their identity** — here, another install's
/// keys dropped in beside this one's database. The reset keeps the keys the
/// device holds, signs a new identity with them under a new Device ID and the
/// old name, and sets the old database and `device.json` aside.
#[test]
fn mismatched_keys_start_over_as_a_new_device_and_keep_the_keys() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let original = IdentityManager::open(&paths, 42).unwrap();
    let other_root = tempfile::tempdir().unwrap();
    let other = Paths::for_root(other_root.path().to_owned());
    let foreign = IdentityManager::open(&other, 42).unwrap();
    std::fs::remove_dir_all(paths.secrets_dir()).unwrap();
    copy_dir(&other.secrets_dir(), &paths.secrets_dir());
    assert_eq!(
        IdentityManager::open(&paths, 43).unwrap_err().code,
        "identity_unavailable"
    );

    let aside = IdentityManager::reset(&paths, 50).unwrap();

    let reset = IdentityManager::open(&paths, 51).unwrap();
    assert_ne!(reset.descriptor.device_id, original.descriptor.device_id);
    assert_eq!(reset.descriptor.endpoint_id, foreign.descriptor.endpoint_id);
    assert_eq!(
        reset.descriptor.display_name,
        original.descriptor.display_name
    );
    assert_eq!(
        Device::open(&paths.device_file(), 52).unwrap().id,
        reset.descriptor.device_id.to_storage()
    );
    assert!(aside.join("agent.db").exists());
    assert!(aside.join("device.json").exists());
    assert!(!aside.join("secrets").exists(), "held keys were kept");
}

/// **A key the device no longer holds** cannot be kept, so the secrets are set
/// aside with everything else and new ones are minted.
#[test]
fn a_missing_key_starts_over_with_new_keys() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let original = IdentityManager::open(&paths, 42).unwrap();
    let secrets = ProtectedSecretStore::new(paths.secrets_dir());
    std::fs::remove_file(secrets.path(SecretName::DeviceSigning)).unwrap();

    let aside = IdentityManager::reset(&paths, 50).unwrap();

    let reset = IdentityManager::open(&paths, 51).unwrap();
    assert_ne!(reset.descriptor.device_id, original.descriptor.device_id);
    assert_ne!(
        reset.descriptor.endpoint_id,
        original.descriptor.endpoint_id
    );
    assert!(aside.join("secrets").exists());
}

/// The request only leaves a marker; the next start does the work and removes
/// it, and a marker beside an intact identity is removed without a reset.
#[test]
fn a_requested_reset_runs_once_at_the_next_start() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let original = IdentityManager::open(&paths, 42).unwrap();
    IdentityManager::request_reset(&paths).unwrap();
    assert!(IdentityManager::reset_if_requested(&paths, 43).is_none());
    assert!(!paths.agent_dir().join(RESET_MARKER).exists());
    assert_eq!(
        IdentityManager::open(&paths, 44)
            .unwrap()
            .descriptor
            .device_id,
        original.descriptor.device_id
    );

    let secrets = ProtectedSecretStore::new(paths.secrets_dir());
    std::fs::remove_file(secrets.path(SecretName::ReplicationEndpoint)).unwrap();
    IdentityManager::request_reset(&paths).unwrap();
    assert!(IdentityManager::reset_if_requested(&paths, 45).is_some());
    assert!(!paths.agent_dir().join(RESET_MARKER).exists());
    assert!(IdentityManager::reset_if_requested(&paths, 46).is_none());
    assert_ne!(
        IdentityManager::open(&paths, 47)
            .unwrap()
            .descriptor
            .device_id,
        original.descriptor.device_id
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}
