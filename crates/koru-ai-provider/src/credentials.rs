// SPDX-License-Identifier: MPL-2.0

//! Secret resolution stays behind the provider boundary.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use koru_ai_core::{AiError, AiErrorClass};
use zeroize::Zeroize;

pub const OPENAI_VAULT_SERVICE: &str = "do.koru.ai.openai";

/// A secret whose debug representation and destructor never expose its value.
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Result<Self, AiError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AiError::new(
                AiErrorClass::Configuration,
                "The OpenAI project API key is empty.",
                false,
            ));
        }
        Ok(Self(value))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_exposed(mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl Clone for SecretString {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(REDACTED)")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Provider credentials are resolved immediately before provider I/O.
pub trait CredentialStore: Send + Sync {
    fn save_openai(&self, account: &str, secret: SecretString) -> Result<(), AiError>;
    fn resolve_openai(&self, account: &str) -> Result<SecretString, AiError>;
    fn remove_openai(&self, account: &str) -> Result<(), AiError>;
}

/// Normal desktop credential storage backed by the platform's native vault.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsCredentialStore;

impl OsCredentialStore {
    pub fn entry_name(account: &str) -> String {
        format!("{OPENAI_VAULT_SERVICE}/{account}")
    }

    fn entry(account: &str) -> Result<keyring::Entry, AiError> {
        validate_account(account)?;
        keyring::Entry::new(OPENAI_VAULT_SERVICE, account).map_err(vault_unavailable)
    }
}

impl CredentialStore for OsCredentialStore {
    fn save_openai(&self, account: &str, secret: SecretString) -> Result<(), AiError> {
        Self::entry(account)?
            .set_password(secret.expose())
            .map_err(vault_unavailable)
    }

    fn resolve_openai(&self, account: &str) -> Result<SecretString, AiError> {
        let value = Self::entry(account)?
            .get_password()
            .map_err(|error| match error {
                keyring::Error::NoEntry => AiError::new(
                    AiErrorClass::Configuration,
                    "No OpenAI project API key is saved for this account.",
                    false,
                ),
                error => vault_unavailable(error),
            })?;
        SecretString::new(value)
    }

    fn remove_openai(&self, account: &str) -> Result<(), AiError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(vault_unavailable(error)),
        }
    }
}

/// Explicit-root development can keep a key in this process only.
#[derive(Default)]
pub struct SessionCredentialStore {
    openai: Mutex<HashMap<String, SecretString>>,
}

impl fmt::Debug for SessionCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCredentialStore")
            .field("openai", &"REDACTED")
            .finish()
    }
}

impl CredentialStore for SessionCredentialStore {
    fn save_openai(&self, account: &str, secret: SecretString) -> Result<(), AiError> {
        validate_account(account)?;
        self.openai
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(account.to_owned(), secret);
        Ok(())
    }

    fn resolve_openai(&self, account: &str) -> Result<SecretString, AiError> {
        validate_account(account)?;
        self.openai
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(account)
            .cloned()
            .ok_or_else(|| {
                AiError::new(
                    AiErrorClass::Configuration,
                    "No session-only OpenAI project API key is configured.",
                    false,
                )
            })
    }

    fn remove_openai(&self, account: &str) -> Result<(), AiError> {
        validate_account(account)?;
        self.openai
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(account);
        Ok(())
    }
}

fn validate_account(account: &str) -> Result<(), AiError> {
    if account.trim().is_empty() || account.len() > 200 || account.chars().any(char::is_control) {
        return Err(AiError::new(
            AiErrorClass::Configuration,
            "The OpenAI credential account label is invalid.",
            false,
        ));
    }
    Ok(())
}

fn vault_unavailable(_: keyring::Error) -> AiError {
    AiError::new(
        AiErrorClass::Configuration,
        "The OS credential vault could not be accessed.",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted_and_session_storage_is_explicit() {
        let secret = SecretString::new("sk-test-never-log").unwrap();
        assert_eq!(format!("{secret:?}"), "SecretString(REDACTED)");

        let store = SessionCredentialStore::default();
        store.save_openai("project-a", secret).unwrap();
        let resolved = store.resolve_openai("project-a").unwrap();
        assert_eq!(resolved.expose(), "sk-test-never-log");
        store.remove_openai("project-a").unwrap();
        assert_eq!(
            store.resolve_openai("project-a").unwrap_err().class,
            AiErrorClass::Configuration
        );
    }

    #[test]
    fn vault_entry_name_contains_no_secret() {
        assert_eq!(
            OsCredentialStore::entry_name("project-a"),
            "do.koru.ai.openai/project-a"
        );
    }
}
