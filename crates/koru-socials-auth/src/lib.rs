// SPDX-License-Identifier: MPL-2.0

//! Shared worker contracts and credential coordination. Provider protocols stay
//! in their adapters; secrets never enter a document or a diagnostic.
use koru_oauth_credentials::CredentialContext;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zeroize::Zeroizing;

pub mod credential_lock;
pub const MAX_RESPONSE: u64 = 256 * 1024;

pub struct ConnectOptions {
    pub root: Option<PathBuf>,
    pub client_id: String,
    pub cancelled: Arc<AtomicBool>,
    pub open_browser: fn(&str) -> std::io::Result<()>,
    pub success_html: String,
    pub error_html: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedAccount {
    pub id: String,
    pub username: String,
    pub display_name: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidClientId,
    AgentUnavailable,
    CallbackUnavailable,
    BrowserUnavailable,
    Cancelled,
    TimedOut,
    InvalidCallback,
    Denied,
    Network,
    AuthorizationRejected,
    IdentityRejected,
    MissingScopes,
    InvalidResponse,
    VaultUnavailable,
    InvalidActor,
    Disabled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Social account connection: {self:?}")
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError {
    NeedsConnection,
    Retryable {
        retry_after_ms: u64,
    },
    Rejected,
    /// An irreversible request may have succeeded. Never automatically resend.
    Unconfirmed,
    /// Local failure before any publication request was sent.
    Unavailable,
}
impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Social publishing: {self:?}")
    }
}
impl std::error::Error for PublishError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posted {
    pub id: String,
    pub url: String,
}

pub fn credential_context(root: Option<&Path>) -> Result<CredentialContext, Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    let segment = koru_channel::segment();
    let endpoint = koru_api_client::grants::agent_endpoint(segment, root)
        .map_err(|_| Error::AgentUnavailable)?;
    let welcome =
        koru_api_client::grants::agent_welcome(&endpoint).map_err(|_| Error::AgentUnavailable)?;
    let device = koru_device::DeviceId::parse(&welcome.device_id).ok_or(Error::AgentUnavailable)?;
    let root = root
        .map(Path::to_path_buf)
        .or_else(koru_channel::channel_root)
        .ok_or(Error::AgentUnavailable)?;
    CredentialContext::new(segment, &root, device).map_err(|_| Error::AgentUnavailable)
}
pub fn validate_client_id(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 512
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    {
        return Err(Error::InvalidClientId);
    }
    Ok(())
}
pub fn check_cancelled(cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
pub fn callback_code<'a>(
    fields: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<&'a str, Error> {
    let mut code = None;
    let mut denied = false;
    for (key, value) in fields {
        match key {
            "code" if code.is_some() || value.is_empty() || value.len() > 4096 => {
                return Err(Error::InvalidCallback);
            }
            "code" => code = Some(value),
            "error" => denied = true,
            _ => {}
        }
    }
    if denied {
        Err(Error::Denied)
    } else {
        code.ok_or(Error::InvalidCallback)
    }
}
pub fn http_client() -> Result<reqwest::blocking::Client, Error> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| Error::Network)
}
pub fn read_response(
    response: reqwest::blocking::Response,
    rejected: Error,
) -> Result<Zeroizing<Vec<u8>>, Error> {
    if !response.status().is_success() {
        return Err(rejected);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Network)?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(Error::InvalidResponse);
    }
    Ok(bytes)
}
