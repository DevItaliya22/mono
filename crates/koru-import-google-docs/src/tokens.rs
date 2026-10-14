// SPDX-License-Identifier: MPL-2.0

//! Google refresh-token storage.
//!
//! The shared credential crate owns OS keyring access and opaque entry names;
//! this adapter owns Google's policy that only refresh tokens are durable.
//! Access tokens never enter this module.

use std::sync::{Arc, Mutex};

use koru_oauth_credentials::{CredentialEntry, CredentialStore, OsCredentialStore, SecretString};

pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<SecretString>, String>;
    fn store(&self, refresh: SecretString) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
}

pub struct ScopedTokenStore {
    store: Arc<dyn CredentialStore>,
    entry: CredentialEntry,
}

impl ScopedTokenStore {
    pub fn os(entry: CredentialEntry) -> Self {
        Self {
            store: Arc::new(OsCredentialStore),
            entry,
        }
    }

    #[cfg(test)]
    pub fn injected(store: Arc<dyn CredentialStore>, entry: CredentialEntry) -> Self {
        Self { store, entry }
    }
}

impl TokenStore for ScopedTokenStore {
    fn load(&self) -> Result<Option<SecretString>, String> {
        self.store
            .load(&self.entry)
            .map_err(|error| format!("could not read Google's refresh token: {error}"))
    }

    fn store(&self, refresh: SecretString) -> Result<(), String> {
        self.store
            .store(&self.entry, refresh)
            .map_err(|error| format!("could not save Google's refresh token: {error}"))
    }

    fn clear(&self) -> Result<(), String> {
        self.store
            .clear(&self.entry)
            .map_err(|error| format!("could not remove Google's refresh token: {error}"))
    }
}

/// Degraded sessions without an Agent-acknowledged Device ID keep a refresh
/// token until the process exits. Importer registries share one instance;
/// there is deliberately no file or less-specific keychain fallback.
#[derive(Default)]
pub struct SessionTokenStore {
    refresh: Mutex<Option<SecretString>>,
}

impl TokenStore for SessionTokenStore {
    fn load(&self) -> Result<Option<SecretString>, String> {
        Ok(self
            .refresh
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone())
    }

    fn store(&self, refresh: SecretString) -> Result<(), String> {
        *self
            .refresh
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(refresh);
        Ok(())
    }

    fn clear(&self) -> Result<(), String> {
        *self
            .refresh
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_device::DeviceId;

    #[test]
    fn session_store_is_memory_only_and_redacted() {
        let store = SessionTokenStore::default();
        assert!(store.load().unwrap().is_none());
        store
            .store(SecretString::new("refresh-never-log").unwrap())
            .unwrap();
        let refresh = store.load().unwrap().unwrap();
        assert_eq!(refresh.expose(), "refresh-never-log");
        assert_eq!(format!("{refresh:?}"), "SecretString(REDACTED)");
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn scoped_store_is_injected_and_persists_only_the_refresh_secret() {
        let context = koru_oauth_credentials::CredentialContext::new(
            "internal-debug",
            std::path::Path::new("synthetic-root"),
            DeviceId::parse("a6258102-2534-4fab-bd4d-40717b066f29").unwrap(),
        )
        .unwrap();
        let entry = context.entry("https://accounts.google.com").unwrap();
        let credentials = Arc::new(koru_oauth_credentials::SessionCredentialStore::default());
        let store = ScopedTokenStore::injected(credentials, entry);
        store
            .store(SecretString::new("refresh-only").unwrap())
            .unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "refresh-only");
    }
}
