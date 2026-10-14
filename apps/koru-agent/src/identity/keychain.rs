// SPDX-License-Identifier: MPL-2.0

//! The device's two secrets in the macOS Keychain, on signed builds (M17).
//!
//! **Which builds.** Stable, beta and nightly are Developer ID signed, so the
//! Keychain knows them as one application across updates. An internal build
//! (`./koru.sh run`) is re-signed ad hoc on every compile and would be a
//! stranger to its own items, so it keeps the owner-only file
//! ([`super::ProtectedSecretStore`]) — ADR-133's reasoning, now scoped to the
//! builds it was true of.
//!
//! **The cutover copies, then deletes on the next launch.** A signed build
//! that finds the old plaintext files copies both into the Keychain, reads
//! them back, and only then writes a marker naming *this build*. The files
//! stay one launch longer, because an update that fails its health check is
//! rolled back to the previous build, which knows only the files. At the next
//! start of the same build the update can no longer roll back, so the files go.
//! A build that was rolled back and then updated past rewrites the marker, and
//! deletes one launch later. (The updater records a failed build and does not
//! reinstall it, so "the same failed build twice" cannot delete early.)
//!
//! **No file fall-back after that.** With the marker present, a missing item
//! is a missing key: the identity fails, and the Link Device window offers a
//! new start. A plaintext file restored from an old backup is never read
//! again — one secret home per build, as the account and Google tokens have.

use std::path::{Path, PathBuf};

use koru_device::{DeviceError, SecretBytes, SecretName, SecretStore};

use super::{ProtectedSecretStore, io_error, set_aside};

/// The Keychain service every device secret is filed under. Accounts are
/// scoped by channel and by the secrets folder, so a `--root` sandbox and
/// another channel never read each other's identity.
const SERVICE: &str = "do.koru.device";
/// Present once the secrets live in the Keychain; holds the build that moved
/// them (or minted them there), which decides when the plaintext may go.
const MARKER: &str = "keychain.v2";

/// Where the bytes live. The system Keychain in production; a map in tests,
/// because a test that touched the real Keychain would prompt, or pass only
/// on the machine that ran it first.
pub(crate) trait KeychainItems: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceError>;
    fn set(&self, account: &str, bytes: &[u8]) -> Result<(), DeviceError>;
    fn delete(&self, account: &str) -> Result<(), DeviceError>;
}

/// The system Keychain, under [`SERVICE`].
#[cfg(target_os = "macos")]
pub(crate) struct SystemKeychain;

#[cfg(target_os = "macos")]
impl SystemKeychain {
    fn entry(&self, account: &str) -> Result<keyring::Entry, DeviceError> {
        keyring::Entry::new(SERVICE, account).map_err(keychain_error)
    }
}

#[cfg(target_os = "macos")]
impl KeychainItems for SystemKeychain {
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceError> {
        match self.entry(account)?.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_error(error)),
        }
    }
    fn set(&self, account: &str, bytes: &[u8]) -> Result<(), DeviceError> {
        self.entry(account)?
            .set_secret(bytes)
            .map_err(keychain_error)
    }
    fn delete(&self, account: &str) -> Result<(), DeviceError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_error(error)),
        }
    }
}

/// Never the bytes, never the account name: the kind of refusal only.
#[cfg(target_os = "macos")]
fn keychain_error(error: keyring::Error) -> DeviceError {
    let kind = match error {
        keyring::Error::NoStorageAccess(_) => "no access",
        keyring::Error::PlatformFailure(_) => "platform failure",
        _ => "other",
    };
    DeviceError::new(
        "keychain_unavailable",
        format!("the Keychain refused ({kind})"),
    )
}

pub(crate) struct KeychainSecrets {
    directory: PathBuf,
    scope: String,
    build: String,
    items: Box<dyn KeychainItems>,
}

impl KeychainSecrets {
    /// The system Keychain, for this build.
    #[cfg(target_os = "macos")]
    pub(crate) fn system(directory: PathBuf) -> Self {
        let info = koru_channel::build_info();
        Self::with_build(
            directory,
            Box::new(SystemKeychain),
            format!("{}+{}", info.version, info.build_number),
        )
    }

    pub(crate) fn with_build(
        directory: PathBuf,
        items: Box<dyn KeychainItems>,
        build: String,
    ) -> Self {
        let location = blake3::hash(directory.to_string_lossy().as_bytes()).to_hex();
        let scope = format!("{}:{}", koru_channel::segment(), &location[..32]);
        Self {
            directory,
            scope,
            build,
            items,
        }
    }

    fn account(&self, name: SecretName) -> String {
        let secret = match name {
            SecretName::DeviceSigning => "device-signing",
            SecretName::ReplicationEndpoint => "replication-endpoint",
        };
        format!("{}:{secret}", self.scope)
    }

    fn files(&self) -> ProtectedSecretStore {
        ProtectedSecretStore::new(self.directory.clone())
    }

    fn marker_path(&self) -> PathBuf {
        self.directory.join(MARKER)
    }

    fn marker(&self) -> Option<String> {
        std::fs::read_to_string(self.marker_path())
            .ok()
            .map(|text| text.trim().to_owned())
    }

    fn write_marker(&self) -> Result<(), DeviceError> {
        self.files().ensure_directory()?;
        std::fs::write(self.marker_path(), &self.build).map_err(io_error)
    }

    fn plaintext(&self) -> [PathBuf; 2] {
        let files = self.files();
        [
            files.path(SecretName::DeviceSigning),
            files.path(SecretName::ReplicationEndpoint),
        ]
    }

    /// Put the secrets where this build keeps them, before anything reads
    /// them. See the module note for the three cases.
    pub(crate) fn prepare(&self) -> Result<(), DeviceError> {
        let plaintext_present = self.plaintext().iter().any(|path| path.exists());
        match self.marker() {
            None => self.migrate(),
            Some(build) if build == self.build => {
                if plaintext_present {
                    for path in self.plaintext() {
                        match std::fs::remove_file(&path) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(io_error(error)),
                        }
                    }
                    tracing::info!("device secrets: the plaintext copies were removed");
                }
                Ok(())
            }
            // Moved by another build, and the files are still here: this one
            // may yet be rolled back, so it owns the next launch's delete.
            Some(_) if plaintext_present => self.write_marker(),
            Some(_) => Ok(()),
        }
    }

    /// Copy both plaintext secrets in, read both back, then mark. Only a
    /// complete pair moves: one file without the other is left for the
    /// identity to report, not half-migrated.
    fn migrate(&self) -> Result<(), DeviceError> {
        let files = self.files();
        let signing = files.load(SecretName::DeviceSigning)?;
        let endpoint = files.load(SecretName::ReplicationEndpoint)?;
        let (Some(signing), Some(endpoint)) = (signing, endpoint) else {
            return Ok(());
        };
        for (name, value) in [
            (SecretName::DeviceSigning, &signing),
            (SecretName::ReplicationEndpoint, &endpoint),
        ] {
            self.put_and_verify(name, value)?;
        }
        self.write_marker()?;
        tracing::info!("device secrets: copied into the Keychain; the files go at the next launch");
        Ok(())
    }

    fn put_and_verify(&self, name: SecretName, value: &SecretBytes) -> Result<(), DeviceError> {
        let account = self.account(name);
        self.items.set(&account, &value.0)?;
        match self.items.get(&account)? {
            Some(back) if back == value.0 => Ok(()),
            _ => Err(DeviceError::new(
                "keychain_unavailable",
                "the Keychain did not return what was written",
            )),
        }
    }

    /// Whether this store holds `name` at all — the fresh-install check.
    pub(crate) fn holds(&self, name: SecretName) -> bool {
        self.items
            .get(&self.account(name))
            .map(|value| value.is_some())
            .unwrap_or(true)
    }

    /// For a reset whose keys cannot be kept: the plaintext and the marker go
    /// aside with the folder, and the Keychain items are deleted — an item
    /// cannot be set aside, and these no longer produce the identity.
    pub(crate) fn retire_into(&self, aside: &Path) -> Result<(), DeviceError> {
        set_aside(&self.directory, aside)?;
        for name in [SecretName::DeviceSigning, SecretName::ReplicationEndpoint] {
            self.items.delete(&self.account(name))?;
        }
        Ok(())
    }
}

impl SecretStore for KeychainSecrets {
    fn load(&self, name: SecretName) -> Result<Option<SecretBytes>, DeviceError> {
        let Some(bytes) = self.items.get(&self.account(name))? else {
            return Ok(None);
        };
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
            DeviceError::new("secret_corrupt", "protected secret has the wrong length")
        })?;
        Ok(Some(SecretBytes(bytes)))
    }

    fn store_new(&self, name: SecretName, value: &SecretBytes) -> Result<(), DeviceError> {
        if self.items.get(&self.account(name))?.is_some() {
            return Err(DeviceError::new(
                "secret_exists",
                "refusing to replace protected key material",
            ));
        }
        self.put_and_verify(name, value)?;
        if self.marker().is_none() {
            self.write_marker()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
