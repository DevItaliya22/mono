// SPDX-License-Identifier: MPL-2.0

//! One prepared grant permits one submission. The owner persists Sending first.
use super::{
    ME_URL, Posted, PublishError, RefreshRecord, TOKEN_URL, USER_AGENT, actor_entry,
    credential_context, parse_actor, parse_tokens, validate_actor,
};
use koru_oauth_credentials::{
    CredentialContext, CredentialEntry, CredentialStore, OsCredentialStore, SecretString,
};
use koru_socials_auth::credential_lock::{CredentialLocks, LockError, RootLocks};
use koru_socials_auth::{MAX_RESPONSE, http_client};
use serde::Deserialize;
use std::{
    io::Read,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const SUBMIT_URL: &str = "https://oauth.reddit.com/api/submit";
const DEFAULT_RETRY_MS: u64 = 60_000;

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

    /// Refresh under the actor's lock, persist rotation, then verify identity.
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
        let token =
            parse_tokens(&response.body, false).map_err(|_| PublishError::NeedsConnection)?;
        let rotated = RefreshRecord {
            version: 1,
            client_id: record.client_id.clone(),
            refresh_token: token
                .refresh_token
                .as_deref()
                .unwrap_or(&record.refresh_token)
                .into(),
        };
        let encoded = rotated.encode().map_err(|_| PublishError::Unavailable)?;
        self.vault
            .store(&entry, encoded.clone())
            .map_err(|_| PublishError::Unavailable)?;
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

    /// Text/self submission only. A lost or ambiguous response must never be
    /// retried automatically; this consumes preparation even on refusal.
    pub fn send(
        &mut self,
        actor: &str,
        text: &str,
        subreddit: &str,
        title: &str,
        flair_id: Option<&str>,
    ) -> Result<Posted, PublishError> {
        let grant = self.prepared.take().ok_or(PublishError::NeedsConnection)?;
        if grant.actor != actor || grant.expires <= Instant::now() {
            return Err(PublishError::NeedsConnection);
        }
        validate_submission(text, subreddit, title, flair_id)?;
        let _guard = self.locks.acquire(&grant.entry).map_err(lock_error)?;
        let current = self
            .vault
            .load(&grant.entry)
            .map_err(|_| PublishError::Unavailable)?
            .ok_or(PublishError::NeedsConnection)?;
        if current.expose() != grant.stored.expose() || grant.expires <= Instant::now() {
            return Err(PublishError::NeedsConnection);
        }
        let response = self
            .transport
            .request(Request::Submit {
                access: grant.access.expose(),
                text,
                subreddit,
                title,
                flair_id,
            })
            .map_err(|_| PublishError::Unconfirmed)?;
        classify_submit(&response)?;
        parse_submitted(&response.body)
    }
}

fn validate_submission(
    text: &str,
    subreddit: &str,
    title: &str,
    flair_id: Option<&str>,
) -> Result<(), PublishError> {
    if text.trim().is_empty()
        || text.chars().count() > 40_000
        || title.trim().is_empty()
        || title.chars().count() > 300
        || title.chars().any(char::is_control)
        || subreddit.is_empty()
        || subreddit.len() > 21
        || !subreddit
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || flair_id.is_some_and(|flair| {
            flair.is_empty()
                || flair.len() > 36
                || !flair
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(PublishError::Rejected);
    }
    Ok(())
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
    Submit {
        access: &'a str,
        text: &'a str,
        subreddit: &'a str,
        title: &'a str,
        flair_id: Option<&'a str>,
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
            Request::Refresh { client_id, refresh } => self
                .0
                .post(TOKEN_URL)
                .basic_auth(client_id, Some(""))
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(refresh_form(refresh).to_string()),
            Request::Identity { access } => self.0.get(ME_URL).bearer_auth(access),
            Request::Submit {
                access,
                text,
                subreddit,
                title,
                flair_id,
            } => self
                .0
                .post(SUBMIT_URL)
                .bearer_auth(access)
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(submit_form(text, subreddit, title, flair_id).to_string()),
        }
        .header(reqwest::header::USER_AGENT, USER_AGENT);
        let response = request.send().map_err(|_| ())?;
        let status = response.status().as_u16();
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .filter(|v| v.len() <= 128)
                .map(str::to_owned)
        };
        let retry_after = header("retry-after");
        let reset = header("x-ratelimit-reset");
        let mut body = Zeroizing::new(Vec::new());
        // Preserve definite refusal status even when its body is broken.
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
fn refresh_form(refresh: &str) -> Zeroizing<String> {
    Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([("grant_type", "refresh_token"), ("refresh_token", refresh)])
            .finish(),
    )
}
fn submit_form(
    text: &str,
    subreddit: &str,
    title: &str,
    flair_id: Option<&str>,
) -> Zeroizing<String> {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.extend_pairs([
        ("api_type", "json"),
        ("raw_json", "1"),
        ("kind", "self"),
        ("sr", subreddit),
        ("title", title),
        ("text", text),
    ]);
    if let Some(flair) = flair_id {
        form.append_pair("flair_id", flair);
    }
    Zeroizing::new(form.finish())
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
fn classify_submit(response: &Response) -> Result<(), PublishError> {
    match response.status {
        200..=299 => Ok(()),
        429 => Err(PublishError::Retryable {
            retry_after_ms: retry_delay(response, unix_ms()),
        }),
        401 => Err(PublishError::NeedsConnection),
        400..=499 if response.status != 408 => Err(PublishError::Rejected),
        _ => Err(PublishError::Unconfirmed),
    }
}
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
fn retry_delay(response: &Response, now_ms: u64) -> u64 {
    let after = response.retry_after.as_deref().and_then(|value| {
        seconds_ms(value).or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value)
                .ok()
                .and_then(|date| u64::try_from(date.timestamp_millis()).ok())
                .map(|date| date.saturating_sub(now_ms))
        })
    });
    // Reddit's reset header is seconds remaining, not X's Unix timestamp.
    let reset = response.reset.as_deref().and_then(seconds_ms);
    after
        .into_iter()
        .chain(reset)
        .max()
        .unwrap_or(DEFAULT_RETRY_MS)
        .max(1000)
}
fn seconds_ms(value: &str) -> Option<u64> {
    let (whole, fraction) = value.trim().split_once('.').unwrap_or((value.trim(), ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let seconds = whole.bytes().fold(0_u64, |n, b| {
        n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
    });
    // Round fractional seconds up, never retry before the provider allows it.
    Some(
        seconds
            .saturating_add(u64::from(fraction.bytes().any(|b| b != b'0')))
            .saturating_mul(1000),
    )
}

#[derive(Deserialize)]
struct SubmitResponse {
    json: SubmitResult,
}
#[derive(Deserialize)]
struct SubmitResult {
    errors: Vec<Vec<serde_json::Value>>,
    data: Option<SubmitIdentity>,
}
#[derive(Deserialize)]
struct SubmitIdentity {
    name: Option<String>,
    id: Option<String>,
    url: Option<String>,
}
fn parse_submitted(bytes: &[u8]) -> Result<Posted, PublishError> {
    let response: SubmitResponse =
        serde_json::from_slice(bytes).map_err(|_| PublishError::Unconfirmed)?;
    if !response.json.errors.is_empty() {
        // Reddit reports validation failures in HTTP 200 JSON. Conflicting
        // evidence is uncertain; never interpret errors alongside an ID as safe.
        if response
            .json
            .data
            .as_ref()
            .is_some_and(|d| d.name.is_some() || d.id.is_some() || d.url.is_some())
        {
            return Err(PublishError::Unconfirmed);
        }
        if response.json.errors.iter().any(|error| {
            error.len() != 3
                || error[0].as_str().is_none_or(str::is_empty)
                || !error.iter().all(serde_json::Value::is_string)
        }) {
            return Err(PublishError::Unconfirmed);
        }
        return Err(PublishError::Rejected);
    }
    let data = response.json.data.ok_or(PublishError::Unconfirmed)?;
    let named_id = data
        .name
        .as_deref()
        .map(|name| name.strip_prefix("t3_").ok_or(PublishError::Unconfirmed))
        .transpose()?;
    let id = data
        .id
        .as_deref()
        .or(named_id)
        .ok_or(PublishError::Unconfirmed)?;
    validate_actor(id).map_err(|_| PublishError::Unconfirmed)?;
    if named_id.is_some_and(|named| named != id) {
        return Err(PublishError::Unconfirmed);
    }
    // Construct from a verified base36 ID. Never open a provider-supplied URL.
    Ok(Posted {
        id: format!("t3_{id}"),
        url: format!("https://www.reddit.com/comments/{id}/"),
    })
}

#[cfg(test)]
mod tests;
