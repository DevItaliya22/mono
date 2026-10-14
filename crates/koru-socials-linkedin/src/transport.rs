// SPDX-License-Identifier: MPL-2.0

use super::API_VERSION;
use std::io::Read;
use zeroize::Zeroizing;

pub(super) enum Request<'a> {
    Identity {
        access: &'a str,
    },
    Roles {
        access: &'a str,
        start: usize,
    },
    Post {
        access: &'a str,
        actor: &'a str,
        text: &'a str,
    },
}
pub(super) struct Response {
    pub status: u16,
    pub retry_after: Option<String>,
    pub id: Option<String>,
    pub body: Zeroizing<Vec<u8>>,
}
pub(super) trait Transport: Send {
    fn request(&mut self, request: Request<'_>) -> Result<Response, ()>;
}
pub(super) struct HttpTransport(pub reqwest::blocking::Client);
impl Transport for HttpTransport {
    fn request(&mut self, request: Request<'_>) -> Result<Response, ()> {
        if cfg!(test) || cfg!(feature = "no-network") {
            return Err(());
        }
        let request = match request {
            Request::Identity { access } => self
                .0
                .get("https://api.linkedin.com/v2/userinfo")
                .bearer_auth(access),
            Request::Roles { access, start } => self
                .0
                .get("https://api.linkedin.com/rest/organizationAcls")
                .query(&[
                    ("q", "roleAssignee"),
                    ("state", "APPROVED"),
                    ("count", "100"),
                    ("start", &start.to_string()),
                ])
                .bearer_auth(access),
            Request::Post {
                access,
                actor,
                text,
            } => self
                .0
                .post("https://api.linkedin.com/rest/posts")
                .bearer_auth(access)
                .json(&post_payload(actor, text)),
        };
        let response = request
            .header("LinkedIn-Version", API_VERSION)
            .header("X-Restli-Protocol-Version", "2.0.0")
            .send()
            .map_err(|_| ())?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let id = response
            .headers()
            .get("x-restli-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        // Error bodies are neither needed nor safe to expose. A successful POST
        // carries its only receipt in a header; never require reading its body.
        let mut body = Zeroizing::new(Vec::new());
        if status == 200 {
            response
                .take(koru_socials_auth::MAX_RESPONSE + 1)
                .read_to_end(&mut body)
                .map_err(|_| ())?;
            if body.len() as u64 > koru_socials_auth::MAX_RESPONSE {
                return Err(());
            }
        }
        Ok(Response {
            status,
            retry_after,
            id,
            body,
        })
    }
}
pub(super) fn post_payload(actor: &str, text: &str) -> serde_json::Value {
    serde_json::json!({ "author": actor, "commentary": plain_text(text), "visibility": "PUBLIC",
        "distribution": { "feedDistribution": "MAIN_FEED", "targetEntities": [], "thirdPartyDistributionChannels": [] },
        "lifecycleState": "PUBLISHED", "isReshareDisabledByAuthor": false })
}
fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "|{}@[]()<>#\\*_~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
pub(super) fn retry_delay(value: Option<&str>) -> u64 {
    let Some(value) = value else {
        return 0;
    };
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = value.bytes().fold(0_u64, |n, byte| {
            n.saturating_mul(10).saturating_add(u64::from(byte - b'0'))
        });
        return seconds.saturating_mul(1000);
    }
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .and_then(|date| date.timestamp_millis().checked_sub(super::now_ms()))
        .and_then(|ms| u64::try_from(ms).ok())
        .unwrap_or(0)
}
