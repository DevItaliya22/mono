// SPDX-License-Identifier: MPL-2.0

//! Provider effects only. The caller durably records its Sending attempt before
//! calling send and owns all scheduling, retries and uncertain-result recovery.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use koru_oauth_credentials::{
    CredentialContext, CredentialEntry, CredentialStore, OsCredentialStore, SecretString,
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::credential_lock::{CredentialLocks, LockError, RootLocks};
use super::{
    MAX_RESPONSE, ME_URL, RefreshRecord, SCOPES, TOKEN_URL, actor_entry, credential_context,
    parse_actor, validate_actor,
};

const POSTS_URL: &str = "https://api.x.com/2/tweets";
const DEFAULT_RETRY_MS: u64 = 60_000;

pub use koru_socials_auth::{Posted, PublishError};

struct Prepared {
    actor: String,
    access: SecretString,
    expires: Instant,
    entry: CredentialEntry,
    stored: SecretString,
}

pub struct Publisher {
    context: CredentialContext,
    transport: Box<dyn Transport>,
    vault: Arc<dyn CredentialStore>,
    locks: Box<dyn CredentialLocks>,
    prepared: Option<Prepared>,
}

impl Publisher {
    /// Blocking worker API. Requires the selected root's running Agent; never
    /// starts a process or reads a different installation's device identity.
    pub fn new(root: Option<PathBuf>) -> Result<Self, PublishError> {
        if cfg!(test) || cfg!(feature = "no-network") {
            return Err(PublishError::Unavailable);
        }
        let context = credential_context(root.as_deref()).map_err(|_| PublishError::Unavailable)?;
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| PublishError::Unavailable)?;
        Ok(Self {
            context,
            transport: Box::new(HttpTransport(http)),
            vault: Arc::new(OsCredentialStore),
            locks: Box::new(
                RootLocks::new(root.as_deref()).map_err(|_| PublishError::Unavailable)?,
            ),
            prepared: None,
        })
    }

    /// Refresh and authenticate before the caller commits Sending. Each prepare
    /// replaces any previous grant and arms exactly one subsequent send call.
    /// Rotation is durable even if the identity request subsequently fails.
    pub fn prepare(&mut self, actor: &str) -> Result<(), PublishError> {
        self.prepared = None;
        validate_actor(actor).map_err(|_| PublishError::NeedsConnection)?;
        let entry = actor_entry(&self.context, actor).map_err(|_| PublishError::Unavailable)?;
        let _guard = self.locks.acquire(&entry).map_err(lock_error)?;
        let stored = self
            .vault
            .load(&entry)
            .map_err(|_| PublishError::Unavailable)?
            .ok_or(PublishError::NeedsConnection)?;
        let record = RefreshRecord::decode(&stored).map_err(|_| PublishError::NeedsConnection)?;
        let began = Instant::now();
        let response = self
            .transport
            .request(Request::Refresh {
                client_id: &record.client_id,
                refresh: &record.refresh_token,
            })
            .map_err(|_| retry())?;
        classify_prepare(&response, true)?;
        let token = parse_refresh(&response.body)?;
        let refresh = token
            .refresh_token
            .as_deref()
            .unwrap_or(&record.refresh_token);
        let rotated = RefreshRecord {
            version: 1,
            client_id: record.client_id.clone(),
            refresh_token: refresh.to_owned(),
        };
        let encoded = SecretString::new(
            serde_json::to_string(&rotated).map_err(|_| PublishError::Unavailable)?,
        )
        .map_err(|_| PublishError::Unavailable)?;
        self.vault
            .store(&entry, encoded.clone())
            .map_err(|_| PublishError::Unavailable)?;
        // Persist rotation before any further remote call: a transient /me
        // failure must not leave the old, potentially spent refresh token.
        let response = self
            .transport
            .request(Request::Identity {
                access: &token.access_token,
            })
            .map_err(|_| retry())?;
        classify_prepare(&response, false)?;
        let identity = parse_actor(&response.body).map_err(|_| PublishError::NeedsConnection)?;
        if identity.id != actor {
            return Err(PublishError::NeedsConnection);
        }
        let expires = began
            .checked_add(Duration::from_secs(token.expires_in.saturating_sub(30)))
            .ok_or(PublishError::NeedsConnection)?;
        if expires <= Instant::now() {
            return Err(PublishError::NeedsConnection);
        }
        self.prepared = Some(Prepared {
            actor: actor.into(),
            access: SecretString::new(token.access_token.clone())
                .map_err(|_| PublishError::NeedsConnection)?,
            expires,
            entry,
            stored: encoded,
        });
        Ok(())
    }

    /// One POST attempt, with all automatic HTTP retries and redirects disabled.
    /// A prepared grant is consumed even when the attempt fails. The caller must
    /// not infer safe retry from a missing ID; only the typed error decides it.
    pub fn send(
        &mut self,
        actor: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Posted, PublishError> {
        let grant = self.prepared.take().ok_or(PublishError::NeedsConnection)?;
        if grant.actor != actor || grant.expires <= Instant::now() {
            return Err(PublishError::NeedsConnection);
        }
        if text.trim().is_empty()
            || text.len() > 64 * 1024
            || reply_to.is_some_and(|id| validate_actor(id).is_err())
        {
            return Err(PublishError::Rejected);
        }
        let _guard = self.locks.acquire(&grant.entry).map_err(lock_error)?;
        let current = self
            .vault
            .load(&grant.entry)
            .map_err(|_| PublishError::Unavailable)?
            .ok_or(PublishError::NeedsConnection)?;
        // Reconnect, disconnect or another publisher's rotation invalidates the
        // prepared grant. Hold the same lock through POST to close that race.
        if current.expose() != grant.stored.expose() || grant.expires <= Instant::now() {
            return Err(PublishError::NeedsConnection);
        }
        let response = self
            .transport
            .request(Request::Post {
                access: grant.access.expose(),
                text,
                reply_to,
            })
            .map_err(|_| PublishError::Unconfirmed)?;
        classify_post(&response)?;
        parse_posted(&response.body)
    }
}

fn lock_error(error: LockError) -> PublishError {
    match error {
        LockError::Busy => PublishError::Retryable {
            retry_after_ms: 1000,
        },
        LockError::Unavailable => PublishError::Unavailable,
    }
}
fn retry() -> PublishError {
    PublishError::Retryable {
        retry_after_ms: DEFAULT_RETRY_MS,
    }
}

enum Request<'a> {
    Refresh {
        client_id: &'a str,
        refresh: &'a str,
    },
    Identity {
        access: &'a str,
    },
    Post {
        access: &'a str,
        text: &'a str,
        reply_to: Option<&'a str>,
    },
}
struct Response {
    status: u16,
    retry_after: Option<String>,
    reset: Option<String>,
    body: Zeroizing<Vec<u8>>,
}
trait Transport: Send {
    fn request(&mut self, request: Request<'_>) -> Result<Response, ()>;
}
struct HttpTransport(reqwest::blocking::Client);
impl Transport for HttpTransport {
    fn request(&mut self, request: Request<'_>) -> Result<Response, ()> {
        if cfg!(test) || cfg!(feature = "no-network") {
            return Err(());
        }
        let request = match request {
            Request::Refresh { client_id, refresh } => {
                let form = refresh_form(client_id, refresh);
                self.0
                    .post(TOKEN_URL)
                    .header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(form.to_string())
            }
            Request::Identity { access } => self.0.get(ME_URL).bearer_auth(access),
            Request::Post {
                access,
                text,
                reply_to,
            } => {
                let body = post_body(text, reply_to)?;
                self.0
                    .post(POSTS_URL)
                    .bearer_auth(access)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body.to_string())
            }
        };
        let response = request.send().map_err(|_| ())?;
        let status = response.status().as_u16();
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .filter(|value| value.len() <= 128)
                .map(str::to_owned)
        };
        let retry_after = header("retry-after");
        let reset = header("x-rate-limit-reset");
        let mut body = Zeroizing::new(Vec::new());
        // Error bodies are not needed to classify a definite refusal. In
        // particular, a broken 429 body must not erase its known status.
        if (200..300).contains(&status) {
            response
                .take(MAX_RESPONSE + 1)
                .read_to_end(&mut body)
                .map_err(|_| ())?;
            if body.len() as u64 > MAX_RESPONSE {
                return Err(());
            }
        }
        Ok(Response {
            status,
            retry_after,
            reset,
            body,
        })
    }
}

fn refresh_form(client_id: &str, refresh: &str) -> Zeroizing<String> {
    Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("refresh_token", refresh),
            ])
            .finish(),
    )
}
#[derive(Serialize)]
struct PostBody<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<Reply<'a>>,
}
#[derive(Serialize)]
struct Reply<'a> {
    in_reply_to_tweet_id: &'a str,
}
fn post_body(text: &str, reply_to: Option<&str>) -> Result<Zeroizing<String>, ()> {
    serde_json::to_string(&PostBody {
        text,
        reply: reply_to.map(|id| Reply {
            in_reply_to_tweet_id: id,
        }),
    })
    .map(Zeroizing::new)
    .map_err(|_| ())
}

#[derive(Deserialize)]
struct RefreshedToken {
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    scope: Option<String>,
    expires_in: u64,
}
impl Drop for RefreshedToken {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(refresh) = self.refresh_token.as_mut() {
            refresh.zeroize();
        }
    }
}
fn parse_refresh(bytes: &[u8]) -> Result<RefreshedToken, PublishError> {
    let token: RefreshedToken =
        serde_json::from_slice(bytes).map_err(|_| PublishError::NeedsConnection)?;
    if token.access_token.is_empty()
        || token.refresh_token.as_ref().is_some_and(String::is_empty)
        || !token.token_type.eq_ignore_ascii_case("bearer")
        || token.expires_in <= 30
        || token.expires_in > 7 * 24 * 3600
        || token.scope.as_ref().is_some_and(|scope| {
            SCOPES
                .split_whitespace()
                .any(|required| !scope.split_whitespace().any(|actual| actual == required))
        })
    {
        return Err(PublishError::NeedsConnection);
    }
    Ok(token)
}

fn classify_prepare(response: &Response, refresh: bool) -> Result<(), PublishError> {
    match response.status {
        200..=299 => Ok(()),
        429 => Err(PublishError::Retryable {
            retry_after_ms: retry_delay(response, unix_ms()),
        }),
        401 => Err(PublishError::NeedsConnection),
        400 if refresh => Err(PublishError::NeedsConnection),
        400..=499 if response.status != 408 => Err(PublishError::Rejected),
        _ => Err(retry()),
    }
}
fn classify_post(response: &Response) -> Result<(), PublishError> {
    match response.status {
        200..=299 => Ok(()),
        429 => Err(PublishError::Retryable {
            retry_after_ms: retry_delay(response, unix_ms()),
        }),
        401 => Err(PublishError::NeedsConnection),
        // 408 can follow a partially received request and is treated as
        // uncertain. Other explicit client refusals are safe not-posted facts.
        400..=499 if response.status != 408 => Err(PublishError::Rejected),
        _ => Err(PublishError::Unconfirmed),
    }
}
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
fn retry_delay(response: &Response, now_ms: u64) -> u64 {
    let after = response.retry_after.as_deref().and_then(|value| {
        parse_seconds(value)
            .map(|seconds| seconds.saturating_mul(1000))
            .or_else(|| {
                chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .and_then(|date| u64::try_from(date.timestamp_millis()).ok())
                    .map(|date| date.saturating_sub(now_ms))
            })
    });
    let reset = response
        .reset
        .as_deref()
        .and_then(parse_seconds)
        .map(|seconds| seconds.saturating_mul(1000).saturating_sub(now_ms));
    after
        .into_iter()
        .chain(reset)
        .max()
        .unwrap_or(DEFAULT_RETRY_MS)
        .max(1000)
}

// A provider's minimum delay must not become a short retry merely because it
// exceeds an integer or product backoff limit. The caller must saturate its
// final timestamp calculation too, rather than casting this u64 unchecked.
fn parse_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(value.bytes().fold(0_u64, |total, byte| {
        total
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'))
    }))
}

#[derive(Deserialize)]
struct PostResponse {
    data: PostIdentity,
}
#[derive(Deserialize)]
struct PostIdentity {
    id: String,
}
fn parse_posted(bytes: &[u8]) -> Result<Posted, PublishError> {
    let response: PostResponse =
        serde_json::from_slice(bytes).map_err(|_| PublishError::Unconfirmed)?;
    validate_actor(&response.data.id).map_err(|_| PublishError::Unconfirmed)?;
    Ok(Posted {
        url: format!("https://x.com/i/status/{}", response.data.id),
        id: response.data.id,
    })
}

#[cfg(test)]
mod tests;
