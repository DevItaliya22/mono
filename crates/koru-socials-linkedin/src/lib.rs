// SPDX-License-Identifier: MPL-2.0

//! LinkedIn native PKCE for approved applications. No client secret or broker.
//! Expired native grants require browser authorization again.
use koru_oauth_credentials::{
    CredentialContext, CredentialEntry, CredentialStore, OsCredentialStore, SecretString,
};
use koru_oauth_loopback::{CallbackListener, WaitOptions};
use koru_socials_auth::credential_lock::{CredentialLocks, LockError, LockedVault, RootLocks};
pub use koru_socials_auth::{ConnectOptions, ConnectedAccount, Error, Posted, PublishError};
use koru_socials_auth::{
    callback_code, check_cancelled, credential_context, http_client, read_response,
    validate_client_id,
};
use oauth2::{CsrfToken, PkceCodeChallenge};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

mod transport;
use transport::{HttpTransport, Request, Response, Transport};
pub const CALLBACK_URL: &str = "http://127.0.0.1:48181/oauth/linkedin/callback";
pub const API_VERSION: &str = "202609";
const PERSON_SCOPES: &str = "openid profile w_member_social";
const PAGE_SCOPES: &str = "openid profile w_organization_social r_organization_admin";
const TOKEN_URL: &str = "https://www.linkedin.com/oauth/v2/accessToken";

pub fn connect(options: ConnectOptions, page: Option<&str>) -> Result<ConnectedAccount, Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    validate_client_id(&options.client_id)?;
    if let Some(id) = page {
        validate_page(id)?;
    }
    check_cancelled(&options.cancelled)?;
    let context = credential_context(options.root.as_deref())?;
    let listener = CallbackListener::bind_port("/oauth/linkedin/callback", 48181)
        .map_err(|_| Error::CallbackUnavailable)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let csrf = CsrfToken::new_random();
    let url = authorize_url(
        &options.client_id,
        csrf.secret(),
        challenge.as_str(),
        page.is_some(),
    )?;
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
        .map_err(|e| match e.kind() {
            koru_oauth_loopback::ErrorKind::Cancelled => Error::Cancelled,
            koru_oauth_loopback::ErrorKind::TimedOut => Error::TimedOut,
            _ => Error::InvalidCallback,
        })?;
    check_cancelled(&options.cancelled)?;
    let began = now_ms();
    let response = http
        .post(TOKEN_URL)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(
            token_form(
                &options.client_id,
                callback_code(callback.fields())?,
                verifier.secret(),
            )
            .to_string(),
        )
        .send()
        .map_err(|_| Error::Network)?;
    let bytes = read_response(response, Error::AuthorizationRejected)?;
    let token = parse_token(&bytes, began, page.is_some())?;
    finish_connection(
        &context,
        token,
        page,
        &LockedVault::new(options.root.as_deref())?,
        &options.cancelled,
        &mut HttpTransport(http),
    )
}

pub fn has_credentials(root: Option<&Path>, actor: &str) -> Result<bool, Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    let entry = actor_entry(&credential_context(root)?, actor)?;
    let secret = OsCredentialStore
        .load(&entry)
        .map_err(|_| Error::VaultUnavailable)?;
    secret
        .map(|s| {
            Record::decode(&s)
                .map(|r| r.actor == actor && r.expires_ms > now_ms().saturating_add(60_000))
        })
        .unwrap_or(Ok(false))
}
pub fn disconnect(root: Option<&Path>, actor: &str) -> Result<(), Error> {
    if cfg!(test) || cfg!(feature = "no-network") {
        return Err(Error::Disabled);
    }
    LockedVault::new(root)?
        .clear(&actor_entry(&credential_context(root)?, actor)?)
        .map_err(|_| Error::VaultUnavailable)
}

fn authorize_url(
    client_id: &str,
    state: &str,
    challenge: &str,
    page: bool,
) -> Result<url::Url, Error> {
    validate_client_id(client_id)?;
    let mut url = url::Url::parse("https://www.linkedin.com/oauth/native-pkce/authorization")
        .expect("static URL");
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", CALLBACK_URL),
        ("state", state),
        ("scope", if page { PAGE_SCOPES } else { PERSON_SCOPES }),
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
struct Token {
    access_token: String,
    expires_in: u64,
    scope: Option<String>,
    token_type: Option<String>,
}
impl Drop for Token {
    fn drop(&mut self) {
        self.access_token.zeroize();
    }
}
fn parse_token(bytes: &[u8], began: i64, page: bool) -> Result<(SecretString, i64), Error> {
    let token: Token = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if token.access_token.is_empty()
        || token.expires_in <= 60
        || token
            .token_type
            .as_deref()
            .is_some_and(|v| !v.eq_ignore_ascii_case("bearer"))
    {
        return Err(Error::InvalidResponse);
    }
    // Native responses document no scope field; consent is all-or-nothing. If
    // supplied, reject a reduced grant instead of assuming requested scopes.
    if let Some(scope) = &token.scope {
        if (if page { PAGE_SCOPES } else { PERSON_SCOPES })
            .split_whitespace()
            .any(|s| !scope.split_whitespace().any(|got| got == s))
        {
            return Err(Error::MissingScopes);
        }
    }
    let expires = i64::try_from(token.expires_in)
        .ok()
        .and_then(|s| s.checked_mul(1000))
        .and_then(|ms| began.checked_add(ms))
        .ok_or(Error::InvalidResponse)?;
    Ok((
        SecretString::new(token.access_token.clone()).map_err(|_| Error::InvalidResponse)?,
        expires,
    ))
}
#[derive(Serialize, Deserialize)]
struct Record {
    version: u8,
    actor: String,
    member: String,
    access_token: String,
    expires_ms: i64,
}
impl Drop for Record {
    fn drop(&mut self) {
        self.access_token.zeroize();
    }
}
impl Record {
    fn decode(secret: &SecretString) -> Result<Self, Error> {
        let r: Self = serde_json::from_str(secret.expose()).map_err(|_| Error::VaultUnavailable)?;
        if r.version != 1
            || validate_actor(&r.actor).is_err()
            || !r.member.starts_with("urn:li:person:")
            || validate_actor(&r.member).is_err()
            || (r.actor.starts_with("urn:li:person:") && r.actor != r.member)
            || r.access_token.is_empty()
        {
            return Err(Error::VaultUnavailable);
        }
        Ok(r)
    }
}
fn validate_page(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || id.len() > 30
        || id.starts_with('0')
        || !id.bytes().all(|b| b.is_ascii_digit())
    {
        Err(Error::InvalidActor)
    } else {
        Ok(())
    }
}
fn validate_actor(actor: &str) -> Result<(), Error> {
    if let Some(id) = actor.strip_prefix("urn:li:organization:") {
        return validate_page(id);
    }
    let id = actor
        .strip_prefix("urn:li:person:")
        .ok_or(Error::InvalidActor)?;
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        Err(Error::InvalidActor)
    } else {
        Ok(())
    }
}
fn actor_entry(context: &CredentialContext, actor: &str) -> Result<CredentialEntry, Error> {
    validate_actor(actor)?;
    context
        .entry("https://api.linkedin.com")
        .and_then(|e| e.for_provider_actor(actor))
        .map_err(|_| Error::VaultUnavailable)
}
fn parse_identity(bytes: &[u8]) -> Result<ConnectedAccount, Error> {
    #[derive(Deserialize)]
    struct Identity {
        sub: String,
        name: String,
    }
    let i: Identity = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    let id = format!("urn:li:person:{}", i.sub);
    validate_actor(&id)?;
    if i.name.trim().is_empty()
        || i.name.chars().count() > 300
        || i.name.chars().any(char::is_control)
    {
        return Err(Error::InvalidResponse);
    }
    Ok(ConnectedAccount {
        id,
        username: i.sub,
        display_name: i.name,
    })
}
fn finish_connection(
    context: &CredentialContext,
    token: (SecretString, i64),
    page: Option<&str>,
    vault: &dyn CredentialStore,
    cancelled: &AtomicBool,
    transport: &mut dyn Transport,
) -> Result<ConnectedAccount, Error> {
    check_cancelled(cancelled)?;
    let response = transport
        .request(Request::Identity {
            access: token.0.expose(),
        })
        .map_err(|_| Error::Network)?;
    if response.status != 200 {
        return Err(Error::IdentityRejected);
    }
    let member = parse_identity(&response.body)?;
    let mut account = member.clone();
    if let Some(id) = page {
        validate_page(id)?;
        let actor = format!("urn:li:organization:{id}");
        let verified = verify_page(
            transport,
            token.0.expose(),
            &member.id,
            &actor,
            Some(cancelled),
        );
        check_cancelled(cancelled)?;
        verified.map_err(|e| match e {
            PublishError::Retryable { .. } => Error::Network,
            _ => Error::IdentityRejected,
        })?;
        // Organization name lookup requires a broader write-admin permission.
        // The verified numeric Page ID identifies this channel without it.
        account = ConnectedAccount {
            id: actor,
            username: id.into(),
            display_name: id.into(),
        };
    }
    check_cancelled(cancelled)?;
    if token.1 <= now_ms().saturating_add(60_000) {
        return Err(Error::AuthorizationRejected);
    }
    let r = Record {
        version: 1,
        actor: account.id.clone(),
        member: member.id,
        access_token: token.0.expose().into(),
        expires_ms: token.1,
    };
    let secret = SecretString::new(serde_json::to_string(&r).map_err(|_| Error::VaultUnavailable)?)
        .map_err(|_| Error::VaultUnavailable)?;
    vault
        .store(&actor_entry(context, &account.id)?, secret)
        .map_err(|_| Error::VaultUnavailable)?;
    check_cancelled(cancelled)?;
    Ok(account)
}

fn verify_page(
    transport: &mut dyn Transport,
    access: &str,
    member: &str,
    actor: &str,
    cancelled: Option<&AtomicBool>,
) -> Result<(), PublishError> {
    // Bounded pagination of the authenticated member's own ACLs. Never follow
    // remote pagination URLs, which could leak the Authorization header.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut start = 0;
    for _ in 0..5 {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(PublishError::Unavailable);
        }
        if Instant::now() >= deadline {
            return Err(retry());
        }
        let response = transport
            .request(Request::Roles { access, start })
            .map_err(|_| retry())?;
        classify(&response, false)?;
        let v: serde_json::Value =
            serde_json::from_slice(&response.body).map_err(|_| PublishError::NeedsConnection)?;
        let rows = v
            .get("elements")
            .and_then(|v| v.as_array())
            .ok_or(PublishError::NeedsConnection)?;
        if rows.iter().any(|v| approved_role(v, member, actor)) {
            return Ok(());
        }
        let next = v
            .get("paging")
            .and_then(|v| v.get("links"))
            .and_then(|v| v.as_array())
            .is_some_and(|links| {
                links
                    .iter()
                    .any(|link| link.get("rel").and_then(|v| v.as_str()) == Some("next"))
            });
        if rows.is_empty() || (!next && rows.len() < 100) {
            break;
        }
        start += rows.len();
    }
    Err(PublishError::NeedsConnection)
}
fn approved_role(v: &serde_json::Value, member: &str, actor: &str) -> bool {
    v.get("roleAssignee").and_then(|v| v.as_str()) == Some(member)
        && v.get("organization")
            .or_else(|| v.get("organizationTarget"))
            .and_then(|v| v.as_str())
            == Some(actor)
        && v.get("state").and_then(|v| v.as_str()) == Some("APPROVED")
        && matches!(
            v.get("role").and_then(|v| v.as_str()),
            Some("ADMINISTRATOR" | "CONTENT_ADMINISTRATOR")
        )
}

struct Prepared {
    actor: String,
    access: SecretString,
    stored: SecretString,
    entry: CredentialEntry,
    expires: Instant,
    expires_ms: i64,
}
pub struct Publisher {
    context: CredentialContext,
    transport: Box<dyn Transport>,
    vault: Arc<dyn CredentialStore>,
    locks: Box<dyn CredentialLocks>,
    prepared: Option<Prepared>,
}
impl Publisher {
    pub fn new(root: Option<PathBuf>) -> Result<Self, PublishError> {
        if cfg!(test) || cfg!(feature = "no-network") {
            return Err(PublishError::Unavailable);
        }
        Ok(Self {
            context: credential_context(root.as_deref()).map_err(|_| PublishError::Unavailable)?,
            transport: Box::new(HttpTransport(
                http_client().map_err(|_| PublishError::Unavailable)?,
            )),
            vault: Arc::new(OsCredentialStore),
            locks: Box::new(
                RootLocks::new(root.as_deref()).map_err(|_| PublishError::Unavailable)?,
            ),
            prepared: None,
        })
    }
    pub fn prepare(&mut self, actor: &str) -> Result<(), PublishError> {
        self.prepared = None;
        let entry = actor_entry(&self.context, actor).map_err(|_| PublishError::NeedsConnection)?;
        let _guard = self.locks.acquire(&entry).map_err(lock_error)?;
        let stored = self
            .vault
            .load(&entry)
            .map_err(|_| PublishError::Unavailable)?
            .ok_or(PublishError::NeedsConnection)?;
        let r = Record::decode(&stored).map_err(|_| PublishError::NeedsConnection)?;
        if r.actor != actor || r.expires_ms <= now_ms().saturating_add(60_000) {
            return Err(PublishError::NeedsConnection);
        }
        let began = Instant::now();
        let response = self
            .transport
            .request(Request::Identity {
                access: &r.access_token,
            })
            .map_err(|_| retry())?;
        classify(&response, false)?;
        let identity = parse_identity(&response.body).map_err(|_| PublishError::NeedsConnection)?;
        if identity.id != r.member {
            return Err(PublishError::NeedsConnection);
        }
        if actor.starts_with("urn:li:organization:") {
            verify_page(
                self.transport.as_mut(),
                &r.access_token,
                &r.member,
                actor,
                None,
            )?;
        }
        let expires = began + Duration::from_secs(60);
        if Instant::now() >= expires || r.expires_ms <= now_ms().saturating_add(60_000) {
            return Err(PublishError::NeedsConnection);
        }
        self.prepared = Some(Prepared {
            actor: actor.into(),
            access: SecretString::new(r.access_token.clone())
                .map_err(|_| PublishError::NeedsConnection)?,
            stored,
            entry,
            expires,
            expires_ms: r.expires_ms,
        });
        Ok(())
    }
    pub fn send(&mut self, actor: &str, text: &str) -> Result<Posted, PublishError> {
        let p = self.prepared.take().ok_or(PublishError::NeedsConnection)?;
        if p.actor != actor
            || p.expires <= Instant::now()
            || p.expires_ms <= now_ms().saturating_add(30_000)
        {
            return Err(PublishError::NeedsConnection);
        }
        if text.trim().is_empty() || text.chars().count() > 3000 {
            return Err(PublishError::Rejected);
        }
        let _guard = self.locks.acquire(&p.entry).map_err(lock_error)?;
        let current = self
            .vault
            .load(&p.entry)
            .map_err(|_| PublishError::Unavailable)?
            .ok_or(PublishError::NeedsConnection)?;
        if current.expose() != p.stored.expose()
            || p.expires <= Instant::now()
            || p.expires_ms <= now_ms().saturating_add(30_000)
        {
            return Err(PublishError::NeedsConnection);
        }
        let response = self
            .transport
            .request(Request::Post {
                access: p.access.expose(),
                actor,
                text,
            })
            .map_err(|_| PublishError::Unconfirmed)?;
        classify(&response, true)?;
        parse_posted(response.id.as_deref())
    }
}
fn parse_posted(id: Option<&str>) -> Result<Posted, PublishError> {
    let id = id.ok_or(PublishError::Unconfirmed)?;
    let suffix = id
        .strip_prefix("urn:li:share:")
        .or_else(|| id.strip_prefix("urn:li:ugcPost:"))
        .ok_or(PublishError::Unconfirmed)?;
    validate_page(suffix).map_err(|_| PublishError::Unconfirmed)?;
    Ok(Posted {
        id: id.into(),
        url: format!("https://www.linkedin.com/feed/update/{id}/"),
    })
}
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn retry() -> PublishError {
    PublishError::Retryable {
        retry_after_ms: 60_000,
    }
}
fn lock_error(e: LockError) -> PublishError {
    match e {
        LockError::Busy => PublishError::Retryable {
            retry_after_ms: 1000,
        },
        LockError::Unavailable => PublishError::Unavailable,
    }
}
fn classify(r: &Response, posting: bool) -> Result<(), PublishError> {
    match r.status {
        201 if posting => Ok(()),
        200 if !posting => Ok(()),
        401 => Err(PublishError::NeedsConnection),
        429 => Err(PublishError::Retryable {
            retry_after_ms: transport::retry_delay(r.retry_after.as_deref()).max(60_000),
        }),
        408 | 500..=599 if posting => Err(PublishError::Unconfirmed),
        408 | 500..=599 => Err(retry()),
        400..=499 => Err(PublishError::Rejected),
        _ if posting => Err(PublishError::Unconfirmed),
        _ => Err(PublishError::NeedsConnection),
    }
}
#[cfg(test)]
mod tests;
