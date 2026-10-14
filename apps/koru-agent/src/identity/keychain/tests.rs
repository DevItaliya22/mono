// SPDX-License-Identifier: MPL-2.0

//! The cutover and its three rules, against a Keychain made of a map.
//!
//! These run on Windows as well as on a Mac (DPAPI files stand in for the
//! Mac's plaintext). Only `SystemKeychain`, the real Keychain, is Mac-only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;
use crate::Paths;
use crate::identity::{DeviceSecrets, IdentityManager};

#[derive(Clone, Default)]
struct Memory(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl Memory {
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

impl KeychainItems for Memory {
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceError> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }
    fn set(&self, account: &str, bytes: &[u8]) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap()
            .insert(account.to_owned(), bytes.to_vec());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), DeviceError> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}

fn keychain(paths: &Paths, memory: &Memory, build: &str) -> DeviceSecrets {
    DeviceSecrets::Keychain(KeychainSecrets::with_build(
        paths.secrets_dir(),
        Box::new(memory.clone()),
        build.to_owned(),
    ))
}

fn file(paths: &Paths) -> DeviceSecrets {
    DeviceSecrets::File(ProtectedSecretStore::new(paths.secrets_dir()))
}

fn plaintext_present(paths: &Paths) -> bool {
    let files = ProtectedSecretStore::new(paths.secrets_dir());
    files.path(SecretName::DeviceSigning).exists()
        || files.path(SecretName::ReplicationEndpoint).exists()
}

fn marker(paths: &Paths) -> String {
    std::fs::read_to_string(paths.secrets_dir().join(MARKER)).unwrap()
}

/// A new signed install never writes a plaintext secret.
#[test]
fn a_fresh_signed_install_keeps_its_secrets_only_in_the_keychain() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let memory = Memory::default();
    let first = IdentityManager::open_with(&paths, keychain(&paths, &memory, "1.0+1"), 42).unwrap();
    assert_eq!(memory.len(), 2);
    assert!(!plaintext_present(&paths));
    assert_eq!(marker(&paths), "1.0+1");
    let again = IdentityManager::open_with(&paths, keychain(&paths, &memory, "1.0+1"), 43).unwrap();
    assert_eq!(again.descriptor.device_id, first.descriptor.device_id);
}

/// **Copy, then delete on the next launch of the same build.** The first
/// signed launch copies and keeps the files — an update can still roll back
/// to a build that knows only them — and the second removes them.
#[test]
fn the_plaintext_outlives_the_migration_by_exactly_one_launch() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let before = IdentityManager::open_with(&paths, file(&paths), 42).unwrap();
    let memory = Memory::default();

    let first = IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 43).unwrap();
    assert_eq!(first.descriptor.device_id, before.descriptor.device_id);
    assert_eq!(memory.len(), 2);
    assert!(
        plaintext_present(&paths),
        "a rollback still needs the files"
    );
    assert_eq!(marker(&paths), "2.0+7");

    // The rolled-back build would read the files, and they are intact.
    assert_eq!(
        IdentityManager::open_with(&paths, file(&paths), 44)
            .unwrap()
            .descriptor
            .device_id,
        before.descriptor.device_id
    );

    let second =
        IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 45).unwrap();
    assert_eq!(second.descriptor.device_id, before.descriptor.device_id);
    assert!(!plaintext_present(&paths));
}

/// A build that was rolled back and then updated past takes over the delete:
/// it rewrites the marker and removes the files one launch later.
#[test]
fn a_later_build_after_a_rollback_owns_the_delete() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    IdentityManager::open_with(&paths, file(&paths), 42).unwrap();
    let memory = Memory::default();
    IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 43).unwrap();
    // …rolled back to the file build, then updated to a newer one.
    IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.1+9"), 44).unwrap();
    assert!(plaintext_present(&paths));
    assert_eq!(marker(&paths), "2.1+9");
    IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.1+9"), 45).unwrap();
    assert!(!plaintext_present(&paths));
}

/// **No file fall-back once moved.** A lost Keychain item is a missing key,
/// even with a plaintext copy restored beside it — the identity fails, and
/// the recovery is the way out.
#[test]
fn a_restored_plaintext_file_is_never_read_after_the_move() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    IdentityManager::open_with(&paths, file(&paths), 42).unwrap();
    let files = ProtectedSecretStore::new(paths.secrets_dir());
    let backup: Vec<_> = [SecretName::DeviceSigning, SecretName::ReplicationEndpoint]
        .into_iter()
        .map(|name| std::fs::read(files.path(name)).unwrap())
        .collect();
    let memory = Memory::default();
    IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 43).unwrap();
    IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 44).unwrap();

    memory.clear();
    for (name, bytes) in [SecretName::DeviceSigning, SecretName::ReplicationEndpoint]
        .into_iter()
        .zip(backup)
    {
        std::fs::write(files.path(name), bytes).unwrap();
    }
    let error =
        IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 45).unwrap_err();
    assert_eq!(error.code, "identity_unavailable");
    assert_eq!(memory.len(), 0, "the file was not copied back in");
}

/// Starting over on a signed Mac whose Keychain lost the keys mints new ones
/// into the Keychain, never into a file.
#[test]
fn starting_over_after_a_lost_keychain_mints_into_the_keychain() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let memory = Memory::default();
    let before =
        IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 42).unwrap();
    memory.clear();

    IdentityManager::reset_with(&paths, || keychain(&paths, &memory, "2.0+7"), 50).unwrap();

    let after = IdentityManager::open_with(&paths, keychain(&paths, &memory, "2.0+7"), 51).unwrap();
    assert_ne!(after.descriptor.device_id, before.descriptor.device_id);
    assert_eq!(memory.len(), 2);
    assert!(!plaintext_present(&paths));
}

/// Two roots on one Mac — a `--root` sandbox beside the real install — never
/// share an item.
#[test]
fn two_roots_are_two_identities_in_one_keychain() {
    let memory = Memory::default();
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    let one = Paths::for_root(one.path().to_owned());
    let two = Paths::for_root(two.path().to_owned());
    let a = IdentityManager::open_with(&one, keychain(&one, &memory, "2.0+7"), 42).unwrap();
    let b = IdentityManager::open_with(&two, keychain(&two, &memory, "2.0+7"), 42).unwrap();
    assert_ne!(a.descriptor.endpoint_id, b.descriptor.endpoint_id);
    assert_eq!(memory.len(), 4);
}

#[test]
fn the_service_and_marker_are_koru_names() {
    assert_eq!(SERVICE, "do.koru.device");
    assert_eq!(MARKER, "keychain.v2");
}
