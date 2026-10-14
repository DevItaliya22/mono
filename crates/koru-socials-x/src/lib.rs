// SPDX-License-Identifier: MPL-2.0

//! Blocking X authorization and publishing for Socials workers.
//! The caller owns scheduling, durable attempts and connection cancellation.
//! Browser opening uses the host's existing, injectable OS opener.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use koru_oauth_credentials::{
    CredentialContext, CredentialEntry, CredentialStore, OsCredentialStore, SecretString,
};
use koru_oauth_loopback::{CallbackListener, WaitOptions};
use oauth2::{CsrfToken, PkceCodeChallenge};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use koru_socials_auth::credential_lock;
pub use koru_socials_auth::{ConnectOptions, ConnectedAccount, Error};
use koru_socials_auth::{
    callback_code, check_cancelled, credential_context, read_response, validate_client_id,
};
mod publishing;
pub use publishing::{Posted, PublishError, Publisher};

pub const CALLBACK_URL: &str = "http://localhost:48179/oauth/x/callback";
pub const SCOPES: &str = "tweet.read users.read tweet.write offline.access";
const CALLBACK_PATH: &str = "/oauth/x/callback";
const CALLBACK_PORT: u16 = 48179;
const ORIGIN: &str = "https://api.x.com";
const TOKEN_URL: &str = "https://api.x.com/2/oauth2/token";
const ME_URL: &str = "https://api.x.com/2/users/me";
const MAX_RESPONSE: u64 = koru_socials_auth::MAX_RESPONSE;

/// Run only on a worker after the user presses Connect. Cancellation is polled
/// every 100 ms during the browser wait and between bounded HTTP requests.
pub fn connect(options: ConnectOptions) -> Result<ConnectedAccount, Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    validate_client_id(&options.client_id)?;
    check_cancelled(&options.cancelled)?;
    let context = credential_context(options.root.as_deref())?;
    let listener = CallbackListener::bind_localhost(CALLBACK_PATH, CALLBACK_PORT)
        .map_err(|_| Error::CallbackUnavailable)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let csrf = CsrfToken::new_random();
    let auth_url = authorize_url(&options.client_id, csrf.secret(), challenge.as_str())?;
    let http = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| Error::Network)?;
    check_cancelled(&options.cancelled)?;
    (options.open_browser)(auth_url.as_str()).map_err(|_| Error::BrowserUnavailable)?;
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
    let body = token_form(&options.client_id, code, verifier.secret());
    let response = http
        .post(TOKEN_URL)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body.to_string())
        .send()
        .map_err(|_| Error::Network)?;
    let bytes = read_response(response, Error::AuthorizationRejected)?;
    let tokens = parse_tokens(&bytes)?;
    finish_connection(
        &context,
        &options.client_id,
        tokens,
        &credential_lock::LockedVault::new(options.root.as_deref())?,
        &options.cancelled,
        |access| {
            let response = http
                .get(ME_URL)
                .bearer_auth(access.expose())
                .send()
                .map_err(|_| Error::Network)?;
            let bytes = read_response(response, Error::IdentityRejected)?;
            parse_actor(&bytes)
        },
    )
}

/// Local vault presence only. Does not refresh or promise the remote grant is
/// still valid. Call off the UI thread when loading a connected channel.
pub fn has_credentials(root: Option<&Path>, actor: &str) -> Result<bool, Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    let entry = actor_entry(&credential_context(root)?, actor)?;
    let record = OsCredentialStore
        .load(&entry)
        .map_err(|_| Error::VaultUnavailable)?;
    record
        .map(|secret| RefreshRecord::decode(&secret).map(|_| true))
        .unwrap_or(Ok(false))
}

/// Forget this installation's refresh credential. This does not revoke the
/// remote X app grant or remove a channel document.
pub fn disconnect(root: Option<&Path>, actor: &str) -> Result<(), Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    credential_lock::LockedVault::new(root)?
        .clear(&actor_entry(&credential_context(root)?, actor)?)
        .map_err(|_| Error::VaultUnavailable)
}

fn authorize_url(client_id: &str, state: &str, challenge: &str) -> Result<url::Url, Error> {
    validate_client_id(client_id)?;
    let mut url = url::Url::parse("https://x.com/i/oauth2/authorize").expect("static URL");
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", CALLBACK_URL),
        ("scope", SCOPES),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ]);
    Ok(url)
}

fn token_form(client_id: &str, code: &str, verifier: &str) -> Zeroizing<String> {
    Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code),
                ("redirect_uri", CALLBACK_URL),
                ("code_verifier", verifier),
            ])
            .finish(),
    )
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    token_type: String,
    scope: String,
}
impl Drop for Tokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
fn parse_tokens(bytes: &[u8]) -> Result<Tokens, Error> {
    let token: Tokens = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if token.access_token.is_empty()
        || token.refresh_token.is_empty()
        || !token.token_type.eq_ignore_ascii_case("bearer")
    {
        return Err(Error::InvalidResponse);
    }
    if SCOPES.split_whitespace().any(|required| {
        !token
            .scope
            .split_whitespace()
            .any(|scope| scope == required)
    }) {
        return Err(Error::MissingScopes);
    }
    Ok(token)
}

#[derive(Deserialize)]
struct IdentityResponse {
    data: Identity,
}
#[derive(Deserialize)]
struct Identity {
    id: String,
    username: String,
    name: String,
}
fn parse_actor(bytes: &[u8]) -> Result<ConnectedAccount, Error> {
    let response: IdentityResponse =
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    let actor = response.data;
    validate_actor(&actor.id)?;
    if actor.username.is_empty()
        || actor.username.len() > 15
        || !actor
            .username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || actor.name.trim().is_empty()
        || actor.name.chars().count() > 100
        || actor.name.chars().any(char::is_control)
    {
        return Err(Error::InvalidResponse);
    }
    Ok(ConnectedAccount {
        id: actor.id,
        username: actor.username,
        display_name: actor.name,
    })
}
fn validate_actor(actor: &str) -> Result<(), Error> {
    if actor.is_empty()
        || actor.len() > 20
        || actor.starts_with('0')
        || !actor.bytes().all(|b| b.is_ascii_digit())
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
        client_id: client_id.to_string(),
        refresh_token: tokens.refresh_token.clone(),
    };
    let encoded = serde_json::to_string(&record).map_err(|_| Error::VaultUnavailable)?;
    let secret = SecretString::new(encoded).map_err(|_| Error::VaultUnavailable)?;
    vault
        .store(&entry, secret)
        .map_err(|_| Error::VaultUnavailable)?;
    // A vault prompt can outlive Cancel. Keep the stored credential (a
    // reconnect may have rotated it), but do not publish a connected result.
    check_cancelled(cancelled)?;
    Ok(actor)
}

#[cfg(test)]
mod tests;
