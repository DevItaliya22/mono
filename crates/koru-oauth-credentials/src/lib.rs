// SPDX-License-Identifier: MPL-2.0

//! The narrow credential boundary shared by native OAuth owners.
//!
//! OAuth owners normally persist refresh tokens and keep access tokens in
//! memory. LinkedIn native Socials grants have no refresh token: their access
//! token and expiry are vault-only until browser reconnection (ADR-455).
//! Desktop production code enables the `desktop`
//! feature for `OsCredentialStore`; the default API has no OS keyring dependency.
//! Tests inject [`CredentialStore`] and therefore never open a real keychain.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::Mutex;

use koru_device::DeviceId;
use serde::{Deserialize, Serialize};
use url::Host;
use zeroize::Zeroize;

pub const VAULT_SERVICE: &str = "do.koru.oauth.refresh";

#[cfg(test)]
mod scope_tests;

/// Non-secret coordinates for one installation's OAuth credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialContext {
    channel: String,
    root_fingerprint: String,
    device: DeviceId,
}

impl fmt::Debug for CredentialContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialContext(REDACTED)")
    }
}

impl CredentialContext {
    /// Build a context without reading the root. The caller supplies the root
    /// already selected for this process and the Agent-acknowledged Device ID.
    /// Windows slash direction, case, and verbatim prefixes are normalized in
    /// the same way as Koru's rooted IPC endpoints; Unix spellings remain
    /// case-sensitive. No canonicalization or other filesystem access occurs.
    pub fn new(channel: &str, root: &Path, device: DeviceId) -> Result<Self, Error> {
        if channel.is_empty()
            || channel.len() > 40
            || !channel
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(Error::InvalidScope);
        }
        let root = root_identity(root);
        if root.is_empty() {
            return Err(Error::InvalidScope);
        }
        Ok(Self {
            channel: channel.to_owned(),
            root_fingerprint: blake3::hash(root.as_bytes()).to_hex().to_string(),
            device,
        })
    }

    /// Derive the opaque keychain account for one OAuth service origin.
    pub fn entry(&self, service_origin: &str) -> Result<CredentialEntry, Error> {
        let origin = canonical_origin(service_origin)?;
        let origin_fingerprint = blake3::hash(origin.as_bytes()).to_hex().to_string();
        Ok(CredentialEntry {
            account: format!(
                "v1:{}:{}:{}:{}",
                self.channel,
                origin_fingerprint,
                self.root_fingerprint,
                self.device.to_storage()
            ),
        })
    }
}

fn root_identity(root: &Path) -> String {
    #[cfg(windows)]
    {
        let value = root.to_string_lossy().replace('/', "\\");
        let ordinary = if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else {
            value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
        };
        ordinary.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        root.to_string_lossy().into_owned()
    }
}

fn canonical_origin(value: &str) -> Result<String, Error> {
    let url = url::Url::parse(value).map_err(|_| Error::InvalidOrigin)?;
    if !is_https_or_loopback_http(&url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidOrigin);
    }
    let mut origin = format!(
        "{}://{}",
        url.scheme(),
        url.host().ok_or(Error::InvalidOrigin)?
    );
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Ok(origin)
}

/// Match the public API's development exception from ADR-307. Credential
/// scoping must accept every origin the transport can use, without widening
/// cleartext access beyond a literal loopback host.
fn is_https_or_loopback_http(url: &url::Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialEntry {
    account: String,
}

impl fmt::Debug for CredentialEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialEntry(REDACTED)")
    }
}

impl CredentialEntry {
    /// Scope a provider credential to its authenticated, stable actor ID.
    /// The provider adapter validates the ID's format and obtains it from its
    /// authenticated identity endpoint. This is separate from Koru account
    /// UUIDs; actor spelling never appears in the keychain account name.
    pub fn for_provider_actor(&self, actor: &str) -> Result<Self, Error> {
        if !self.account.starts_with("v1:")
            || actor.is_empty()
            || actor.len() > 256
            || actor.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(Error::InvalidScope);
        }
        Ok(Self {
            account: format!(
                "provider-v1:{}:{}",
                self.namespace(),
                blake3::hash(actor.as_bytes()).to_hex()
            ),
        })
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// Non-secret partition for the directory in a root shared by more than
    /// one channel or hosted origin. The legacy entry is the namespace authority.
    pub fn namespace(&self) -> String {
        blake3::hash(self.account.as_bytes()).to_hex().to_string()
    }

    /// Add an authenticated canonical account UUID without changing the legacy
    /// slot. Identity must come from the authenticated session, never UI text.
    pub fn for_account(&self, account_id: &str) -> Result<Self, Error> {
        if !self.account.starts_with("v1:")
            || account_id.len() != 36
            || account_id.bytes().enumerate().any(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    byte != b'-'
                } else {
                    !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte)
                }
            })
            || account_id.as_bytes()[14] != b'4'
            || !b"89ab".contains(&account_id.as_bytes()[19])
        {
            return Err(Error::InvalidScope);
        }
        Ok(Self {
            account: format!("v2:{}:{account_id}", &self.account[3..]),
        })
    }
}

/// A secret with a redacted debug representation and zeroing destructor.
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty() {
            return Err(Error::EmptySecret);
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The only durable native account credential. Its JSON representation lives
/// as one OS-vault value so a refresh rotation cannot leave the session id and
/// token from different generations.
#[derive(Serialize, Deserialize)]
pub struct SessionCredential {
    pub version: u8,
    pub session_id: String,
    pub refresh_token: String,
}

impl SessionCredential {
    pub const VERSION: u8 = 1;

    pub fn new(session_id: String, refresh_token: String) -> Result<Self, Error> {
        if session_id.is_empty() || refresh_token.is_empty() {
            return Err(Error::EmptySecret);
        }
        Ok(Self {
            version: Self::VERSION,
            session_id,
            refresh_token,
        })
    }

    pub fn encode(&self) -> Result<SecretString, Error> {
        SecretString::new(serde_json::to_string(self).map_err(|_| Error::InvalidRecord)?)
    }

    pub fn decode(secret: &SecretString) -> Result<Self, Error> {
        let record: Self =
            serde_json::from_str(secret.expose()).map_err(|_| Error::InvalidRecord)?;
        if record.version != Self::VERSION
            || record.session_id.is_empty()
            || record.refresh_token.is_empty()
        {
            return Err(Error::InvalidRecord);
        }
        Ok(record)
    }
}

impl fmt::Debug for SessionCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionCredential(REDACTED)")
    }
}

impl Drop for SessionCredential {
    fn drop(&mut self) {
        self.session_id.zeroize();
        self.refresh_token.zeroize();
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidScope,
    InvalidOrigin,
    EmptySecret,
    InvalidRecord,
    Unavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidScope => "the OAuth credential scope is invalid",
            Self::InvalidOrigin => "the OAuth service must be an HTTPS origin",
            Self::EmptySecret => "the OAuth refresh token is empty",
            Self::InvalidRecord => "the OAuth credential record is invalid",
            Self::Unavailable => "the OS credential store is unavailable",
        })
    }
}

impl std::error::Error for Error {}

pub trait CredentialStore: Send + Sync {
    fn load(&self, entry: &CredentialEntry) -> Result<Option<SecretString>, Error>;
    fn store(&self, entry: &CredentialEntry, secret: SecretString) -> Result<(), Error>;
    fn clear(&self, entry: &CredentialEntry) -> Result<(), Error>;
}

#[derive(Clone, Copy, Debug, Default)]
#[cfg(feature = "desktop")]
pub struct OsCredentialStore;

#[cfg(feature = "desktop")]
impl OsCredentialStore {
    fn entry(entry: &CredentialEntry) -> Result<keyring::Entry, Error> {
        keyring::Entry::new(VAULT_SERVICE, entry.account()).map_err(|_| Error::Unavailable)
    }
}

#[cfg(feature = "desktop")]
impl CredentialStore for OsCredentialStore {
    fn load(&self, entry: &CredentialEntry) -> Result<Option<SecretString>, Error> {
        match Self::entry(entry)?.get_password() {
            Ok(value) => SecretString::new(value).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Error::Unavailable),
        }
    }

    fn store(&self, entry: &CredentialEntry, secret: SecretString) -> Result<(), Error> {
        Self::entry(entry)?
            .set_password(secret.expose())
            .map_err(|_| Error::Unavailable)
    }

    fn clear(&self, entry: &CredentialEntry) -> Result<(), Error> {
        match Self::entry(entry)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Error::Unavailable),
        }
    }
}

/// Process-only storage for degraded callers and deterministic tests.
#[derive(Default)]
pub struct SessionCredentialStore {
    entries: Mutex<HashMap<String, SecretString>>,
}

impl CredentialStore for SessionCredentialStore {
    fn load(&self, entry: &CredentialEntry) -> Result<Option<SecretString>, Error> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(entry.account())
            .cloned())
    }

    fn store(&self, entry: &CredentialEntry, secret: SecretString) -> Result<(), Error> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(entry.account().to_owned(), secret);
        Ok(())
    }

    fn clear(&self, entry: &CredentialEntry) -> Result<(), Error> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(entry.account());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(root: &str, device: &str) -> CredentialContext {
        CredentialContext::new("nightly", Path::new(root), DeviceId::parse(device).unwrap())
            .unwrap()
    }

    #[test]
    fn entry_is_stable_scoped_and_contains_no_path_origin_or_email() {
        let device = "a6258102-2534-4fab-bd4d-40717b066f29";
        let scope = context(r"C:\Users\alice@example.com\Koru", device);
        let first = scope.entry("https://accounts.google.com").unwrap();
        let second = scope.entry("https://accounts.google.com/").unwrap();
        assert_eq!(first, second);
        assert_eq!(format!("{scope:?}"), "CredentialContext(REDACTED)");
        assert_eq!(format!("{first:?}"), "CredentialEntry(REDACTED)");
        assert!(first.account().starts_with("v1:nightly:"));
        assert!(first.account().ends_with(device));
        for private in ["alice", "example.com", "Users", "accounts.google.com"] {
            assert!(!first.account().contains(private), "{private} leaked");
        }

        assert_ne!(
            first,
            context(r"D:\Koru", device)
                .entry("https://accounts.google.com")
                .unwrap()
        );
        if cfg!(windows) {
            assert_eq!(
                first,
                context("c:/users/alice@example.com/koru", device)
                    .entry("https://accounts.google.com")
                    .unwrap()
            );
        }
        assert_ne!(
            first,
            context(
                r"C:\Users\alice@example.com\Koru",
                "b6258102-2534-4fab-bd4d-40717b066f29"
            )
            .entry("https://accounts.google.com")
            .unwrap()
        );
        assert_ne!(first, scope.entry("https://login.example.net").unwrap());
        assert_ne!(
            first,
            CredentialContext::new(
                "stable",
                Path::new(r"C:\Users\alice@example.com\Koru"),
                DeviceId::parse(device).unwrap(),
            )
            .unwrap()
            .entry("https://accounts.google.com")
            .unwrap()
        );
    }

    #[test]
    fn two_root_overrides_never_share_account_credentials() {
        let device = "a6258102-2534-4fab-bd4d-40717b066f29";
        let origin = "https://account.example.test";
        let first = context(r"C:\isolated\first", device).entry(origin).unwrap();
        let second = context(r"C:\isolated\second", device)
            .entry(origin)
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn only_https_or_a_literal_loopback_http_origin_is_accepted() {
        let context = context("root", "a6258102-2534-4fab-bd4d-40717b066f29");
        let localhost = context.entry("http://localhost:8080").unwrap();
        assert_eq!(localhost, context.entry("http://LOCALHOST:8080/").unwrap());
        assert!(context.entry("http://127.0.0.1:8080").is_ok());
        assert!(context.entry("http://[::1]:8080").is_ok());
        for origin in [
            "http://accounts.google.com",
            "http://localhost.example.com",
            "https://me@example.com",
            "https://example.com/path",
            "https://example.com?query",
        ] {
            assert_eq!(context.entry(origin), Err(Error::InvalidOrigin), "{origin}");
        }
    }

    #[test]
    fn injected_store_round_trips_without_a_keychain() {
        let entry = context("root", "a6258102-2534-4fab-bd4d-40717b066f29")
            .entry("https://accounts.google.com")
            .unwrap();
        let store = SessionCredentialStore::default();
        let secret = SecretString::new("refresh-never-log").unwrap();
        assert_eq!(format!("{secret:?}"), "SecretString(REDACTED)");
        store.store(&entry, secret).unwrap();
        assert_eq!(
            store.load(&entry).unwrap().unwrap().expose(),
            "refresh-never-log"
        );
        store.clear(&entry).unwrap();
        assert!(store.load(&entry).unwrap().is_none());
    }

    #[test]
    fn session_record_round_trips_and_debug_is_redacted() {
        let record =
            SessionCredential::new("session-secret".into(), "refresh-secret".into()).unwrap();
        assert_eq!(format!("{record:?}"), "SessionCredential(REDACTED)");
        let encoded = record.encode().unwrap();
        assert!(!format!("{encoded:?}").contains("refresh-secret"));
        let decoded = SessionCredential::decode(&encoded).unwrap();
        assert_eq!(decoded.session_id, "session-secret");
        assert_eq!(decoded.refresh_token, "refresh-secret");
    }
}
