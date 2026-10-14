// SPDX-License-Identifier: MPL-2.0

//! Reddit installed-app OAuth and publishing. Call only from a worker.
//! Credentials remain in this installation's OS vault, scoped to stable actor ID.

use koru_oauth_credentials::{
    CredentialContext, CredentialEntry, CredentialStore, OsCredentialStore, SecretString,
};
use koru_oauth_loopback::{CallbackListener, WaitOptions};
use koru_socials_auth::credential_lock::LockedVault;
use koru_socials_auth::{
    callback_code, check_cancelled, credential_context, http_client, read_response,
    validate_client_id,
};
use oauth2::CsrfToken;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

pub use koru_socials_auth::{ConnectOptions, ConnectedAccount, Error, Posted, PublishError};
mod publishing;
pub use publishing::Publisher;

pub const CALLBACK_URL: &str = "http://localhost:48180/oauth/reddit/callback";
pub const SCOPES: &str = "identity submit";
const TOKEN_URL: &str = "https://www.reddit.com/api/v1/access_token";
const ME_URL: &str = "https://oauth.reddit.com/api/v1/me";
const ORIGIN: &str = "https://oauth.reddit.com";
const USER_AGENT: &str = concat!(
    "desktop:do.koru.socials:",
    env!("CARGO_PKG_VERSION"),
    " (https://www.koru.do)"
);

/// Installed apps use their public client ID and an empty Basic-auth secret.
/// A provider-approved Reddit application is required before using this flow.
pub fn connect(options: ConnectOptions) -> Result<ConnectedAccount, Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    validate_client_id(&options.client_id)?;
    check_cancelled(&options.cancelled)?;
    let context = credential_context(options.root.as_deref())?;
    let listener = CallbackListener::bind_localhost("/oauth/reddit/callback", 48180)
        .map_err(|_| Error::CallbackUnavailable)?;
    let csrf = CsrfToken::new_random();
    let url = authorize_url(&options.client_id, csrf.secret())?;
    let http = http_client()?;
    check_cancelled(&options.cancelled)?;
    (options.open_browser)(url.as_str()).map_err(|_| Error::BrowserUnavailable)?;
    let callback = listener
        .wait(
            &WaitOptions {
                expected_state: csrf.secret(),
                timeout: Duration::from_secs(300),
                poll: koru_oauth_loopback::DEFAULT_POLL,
                max_request_bytes: koru_oauth_loopback::DEFAULT_MAX_REQUEST_BYTES,
                success_html: &options.success_html,
                error_html: &options.error_html,
            },
            &|| options.cancelled.load(Ordering::Relaxed),
        )
        .map_err(|error| match error.kind() {
            koru_oauth_loopback::ErrorKind::Cancelled => Error::Cancelled,
            koru_oauth_loopback::ErrorKind::TimedOut => Error::TimedOut,
            _ => Error::InvalidCallback,
        })?;
    let code = callback_code(callback.fields())?;
    check_cancelled(&options.cancelled)?;
    let response = http
        .post(TOKEN_URL)
        .basic_auth(&options.client_id, Some(""))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(token_form(code).to_string())
        .send()
        .map_err(|_| Error::Network)?;
    let tokens = parse_tokens(
        &read_response(response, Error::AuthorizationRejected)?,
        true,
    )?;
    finish_connection(
        &context,
        &options.client_id,
        tokens,
        &LockedVault::new(options.root.as_deref())?,
        &options.cancelled,
        |access| {
            let response = http
                .get(ME_URL)
                .bearer_auth(access.expose())
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .send()
                .map_err(|_| Error::Network)?;
            parse_actor(&read_response(response, Error::IdentityRejected)?)
        },
    )
}

/// Local presence only; this does not promise the remote grant is valid.
pub fn has_credentials(root: Option<&Path>, actor: &str) -> Result<bool, Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    let entry = actor_entry(&credential_context(root)?, actor)?;
    OsCredentialStore
        .load(&entry)
        .map_err(|_| Error::VaultUnavailable)?
        .map(|secret| RefreshRecord::decode(&secret).map(|_| true))
        .unwrap_or(Ok(false))
}

/// Forget only the local credential, not the remote Reddit app authorization.
pub fn disconnect(root: Option<&Path>, actor: &str) -> Result<(), Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    LockedVault::new(root)?
        .clear(&actor_entry(&credential_context(root)?, actor)?)
        .map_err(|_| Error::VaultUnavailable)
}

fn authorize_url(client_id: &str, state: &str) -> Result<url::Url, Error> {
    validate_client_id(client_id)?;
    let mut url = url::Url::parse("https://www.reddit.com/api/v1/authorize").expect("static URL");
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id),
        ("response_type", "code"),
        ("state", state),
        ("redirect_uri", CALLBACK_URL),
        ("duration", "permanent"),
        ("scope", SCOPES),
    ]);
    Ok(url)
}

fn token_form(code: &str) -> Zeroizing<String> {
    Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", CALLBACK_URL),
            ])
            .finish(),
    )
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    expires_in: u64,
    scope: String,
}
impl Drop for Tokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(token) = self.refresh_token.as_mut() {
            token.zeroize();
        }
    }
}
fn parse_tokens(bytes: &[u8], initial: bool) -> Result<Tokens, Error> {
    let token: Tokens = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if token.access_token.is_empty()
        || !token.token_type.eq_ignore_ascii_case("bearer")
        || token.expires_in <= 30
        || token.expires_in > 7 * 24 * 3600
        || token.refresh_token.as_ref().is_some_and(String::is_empty)
        || (initial && token.refresh_token.is_none())
    {
        return Err(Error::InvalidResponse);
    }
    if SCOPES.split_whitespace().any(|required| {
        !token
            .scope
            .split_whitespace()
            .any(|actual| actual == required)
    }) {
        return Err(Error::MissingScopes);
    }
    Ok(token)
}

#[derive(Deserialize)]
struct Identity {
    id: String,
    name: String,
}
fn parse_actor(bytes: &[u8]) -> Result<ConnectedAccount, Error> {
    let actor: Identity = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    validate_actor(&actor.id)?;
    if actor.name.is_empty()
        || actor.name.len() > 64
        || !actor
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::InvalidResponse);
    }
    Ok(ConnectedAccount {
        id: actor.id,
        username: actor.name.clone(),
        display_name: actor.name,
    })
}
fn validate_actor(actor: &str) -> Result<(), Error> {
    if actor.is_empty()
        || actor.len() > 32
        || !actor
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Err(Error::InvalidActor);
    }
    Ok(())
}
fn actor_entry(context: &CredentialContext, actor: &str) -> Result<CredentialEntry, Error> {
    validate_actor(actor)?;
    context
        .entry(ORIGIN)
        .and_then(|entry| entry.for_provider_actor(actor))
        .map_err(|_| Error::VaultUnavailable)
}

#[derive(Serialize, Deserialize)]
struct RefreshRecord {
    version: u8,
    client_id: String,
    refresh_token: String,
}
impl Drop for RefreshRecord {
    fn drop(&mut self) {
        self.refresh_token.zeroize();
    }
}
impl RefreshRecord {
    fn decode(secret: &SecretString) -> Result<Self, Error> {
        let record: Self =
            serde_json::from_str(secret.expose()).map_err(|_| Error::VaultUnavailable)?;
        if record.version != 1
            || record.refresh_token.is_empty()
            || validate_client_id(&record.client_id).is_err()
        {
            return Err(Error::VaultUnavailable);
        }
        Ok(record)
    }
    fn encode(&self) -> Result<SecretString, Error> {
        SecretString::new(serde_json::to_string(self).map_err(|_| Error::VaultUnavailable)?)
            .map_err(|_| Error::VaultUnavailable)
    }
}

fn finish_connection(
    context: &CredentialContext,
    client_id: &str,
    tokens: Tokens,
    vault: &dyn CredentialStore,
    cancelled: &AtomicBool,
    verify: impl FnOnce(&SecretString) -> Result<ConnectedAccount, Error>,
) -> Result<ConnectedAccount, Error> {
    check_cancelled(cancelled)?;
    let access =
        SecretString::new(tokens.access_token.clone()).map_err(|_| Error::InvalidResponse)?;
    let actor = verify(&access)?;
    check_cancelled(cancelled)?;
    let entry = actor_entry(context, &actor.id)?;
    let record = RefreshRecord {
        version: 1,
        client_id: client_id.into(),
        refresh_token: tokens.refresh_token.clone().ok_or(Error::InvalidResponse)?,
    };
    vault
        .store(&entry, record.encode()?)
        .map_err(|_| Error::VaultUnavailable)?;
    check_cancelled(cancelled)?;
    Ok(actor)
}

#[cfg(test)]
mod tests;
