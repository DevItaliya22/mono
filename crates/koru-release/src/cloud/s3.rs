// SPDX-License-Identifier: MPL-2.0

//! **The S3-compatible half of R2, because the REST API refuses a large body.**
//!
//! Everything Koru has published so far fits in one `PUT` to
//! `api.cloudflare.com/client/v4/accounts/…/r2/buckets/…/objects/<key>`, which
//! is what [`crate::cloud::Cloud::put_file`] does. A speech model does not:
//! **that endpoint's documented maximum upload is 300 MB**, and the smallest
//! artifact this publisher carries is a 475 MB Nemotron export. The endpoint
//! has no multipart companion either — the Cloudflare REST API exposes
//! `Upload Object` and `Delete Object` and nothing that creates, uploads a part
//! to, or completes a multipart upload. Multipart exists only in R2's
//! S3-compatible API and in the Workers binding.
//!
//! So a model archive can only be published through the S3 API, and the S3 API
//! is signed with AWS Signature Version 4. That is what this module is, and it
//! is the whole of the new mechanism the voice publisher needed.
//!
//! ## The S3 credential is explicit
//!
//! Cloudflare's R2 token screen prints an **Access Key ID** and **Secret Access
//! Key** for the S3-compatible API. Production publication supplies those as
//! `CLOUDFLARE_R2_ACCESS_KEY_ID` and `CLOUDFLARE_R2_SECRET_ACCESS_KEY`; they are
//! separate inputs from the `CLOUDFLARE_API_TOKEN` used by Cloudflare's REST API
//! for bucket, DNS, cache, and small-object operations.
//!
//! Cloudflare also documents a derivation for an R2 API token created through
//! its API: the access key is the token id and the secret is the lowercase hex
//! SHA-256 of the token value. [`S3Credentials::derive`] preserves that fallback
//! for such tokens, obtaining the id from `GET /user/tokens/verify`. A general
//! custom Cloudflare API token is not assumed to be an R2 S3 credential: it may
//! authenticate the REST API and still produce `SignatureDoesNotMatch` at the
//! R2 endpoint.
//!
//! Both explicit values must be supplied together. Mixing one explicit half
//! with one derived half is invalid and is refused before a request is sent.
//!
//! ## What is tested, and what cannot be
//!
//! The signing arithmetic is deterministic and is tested. The HMAC underneath
//! it is pinned against **RFC 4231's** published vectors rather than against a
//! value this file produced, because a test that asserts a function's own
//! output is a test that a typo passes. The canonical request and the
//! string-to-sign are asserted as literal text, so a reviewer reads the bytes
//! being signed rather than a hash of them.
//!
//! What is **not** tested is that Cloudflare accepts them: that needs the live
//! origin, a real token and a real upload, and this tool's whole safety rule is
//! that uploading takes an explicit flag. Treat the first real
//! `voice-models publish --upload` as the acceptance test it is, and read
//! [`docs/features/voice/archive/QUESTIONS.md`](../../../../docs/features/voice/archive/QUESTIONS.md)
//! Q-V80 before doing it.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// R2's S3 endpoints are region-less; `auto` is the literal Cloudflare
/// documents, and a real AWS region name here is signed into the credential
/// scope and rejected.
pub const REGION: &str = "auto";

const SERVICE: &str = "s3";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// **The documented maximum body for the Cloudflare REST object upload.**
///
/// Stated by the endpoint's own reference page. It is the number that decides
/// this module exists, so it is a named constant rather than a comment.
pub const REST_UPLOAD_MAX_BYTES: u64 = 300 * 1000 * 1000;

/// Where [`crate::cloud::Cloud::put_file_any_size`] switches routes.
///
/// **Below the documented cap on purpose.** "300 MB" does not say whether
/// Cloudflare measures it in millions or in mebibytes, whether the limit is on
/// the body or on the whole request, or whether a proxy in front of it is
/// stricter. An object near the boundary is exactly the one whose failure would
/// be intermittent, so nothing near the boundary goes that way.
pub const MULTIPART_ABOVE_BYTES: u64 = 200 * 1024 * 1024;

/// How much of a file one `UploadPart` carries.
///
/// **All parts but the last must be the same size** in R2, and the minimum is
/// 5 MiB, so this is one number rather than a strategy. 64 MiB puts a 475 MB
/// export in eight parts and a 574 MB one in nine — comfortably under the
/// 10,000-part ceiling, and small enough that a part is read into memory
/// without thinking about it.
pub const PART_SIZE: u64 = 64 * 1024 * 1024;

/// An R2 S3 credential pair, either supplied explicitly or derived from an R2
/// API token.
#[derive(Clone)]
pub struct S3Credentials {
    pub access_key_id: String,
    secret_access_key: String,
}

impl S3Credentials {
    pub fn explicit(access_key_id: String, secret_access_key: String) -> S3Credentials {
        S3Credentials {
            access_key_id,
            secret_access_key,
        }
    }

    /// Both explicit values must travel together; mixing one with a derived
    /// half produces only an opaque SignatureDoesNotMatch response.
    pub fn from_explicit_pair(
        access_key_id: Option<String>,
        secret_access_key: Option<String>,
    ) -> Result<Option<S3Credentials>, String> {
        match (access_key_id, secret_access_key) {
            (None, None) => Ok(None),
            (Some(access), Some(secret)) if !access.is_empty() && !secret.is_empty() => {
                Ok(Some(S3Credentials::explicit(access, secret)))
            }
            _ => Err(
                "set both CLOUDFLARE_R2_ACCESS_KEY_ID and CLOUDFLARE_R2_SECRET_ACCESS_KEY, or neither"
                    .into(),
            ),
        }
    }

    /// `id` verbatim, and the SHA-256 of `value` as lowercase hex.
    ///
    /// Not a hash of the *token id*, and not a base64 — both are easy to write
    /// and would fail only against the live origin, which is the one place this
    /// tool refuses to rehearse.
    pub fn derive(token_id: &str, token_value: &str) -> S3Credentials {
        S3Credentials {
            access_key_id: token_id.to_string(),
            secret_access_key: hex(&Sha256::digest(token_value.as_bytes())),
        }
    }
}

impl std::fmt::Debug for S3Credentials {
    /// The secret is never printed, including by a `{:?}` somebody adds to a
    /// log line in a hurry.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .finish()
    }
}

/// One request, in the form SigV4 has an opinion about.
///
/// Headers are `(name, value)` with the name already lowercase: canonicalising
/// them here rather than at the call site would hide the one rule a caller has
/// to obey, which is that **every header sent must be listed**. A header sent
/// and not signed is not an error; a header signed and not sent is a 403 with
/// no useful body.
pub struct CanonicalRequest<'a> {
    pub method: &'a str,
    /// Already absolute and un-encoded — `/koru-updates/models/x/1.0/ab.zip`.
    pub path: &'a str,
    /// `(name, value)`, un-encoded, in any order.
    pub query: &'a [(&'a str, &'a str)],
    /// `(lowercase name, value)`, in any order. Must include `host`.
    pub headers: &'a [(String, String)],
    /// Lowercase hex SHA-256 of the body. R2 accepts `UNSIGNED-PAYLOAD`; this
    /// publisher never sends it, because the bytes are already in hand and a
    /// signed payload is the difference between authenticating a request and
    /// authenticating a request's envelope.
    pub payload_sha256: &'a str,
}

impl CanonicalRequest<'_> {
    /// The canonical request text, exactly as it is hashed.
    pub fn render(&self) -> String {
        let mut query: Vec<(String, String)> = self
            .query
            .iter()
            .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
            .collect();
        query.sort();
        let query = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let mut headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        headers.sort();
        let signed = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_headers = headers
            .iter()
            .map(|(name, value)| format!("{name}:{value}\n"))
            .collect::<String>();

        format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            self.method,
            uri_encode(self.path, true),
            query,
            canonical_headers,
            signed,
            self.payload_sha256
        )
    }

    /// The `SignedHeaders=` list, which has to be the same set the text above
    /// used or the two disagree in a way only the server sees.
    pub fn signed_headers(&self) -> String {
        let mut names: Vec<String> = self
            .headers
            .iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect();
        names.sort();
        names.join(";")
    }
}

/// `AWS4-HMAC-SHA256\n<amz date>\n<scope>\n<hash of the canonical request>`.
pub fn string_to_sign(amz_date: &str, date_stamp: &str, canonical: &str) -> String {
    format!(
        "{ALGORITHM}\n{amz_date}\n{}\n{}",
        scope(date_stamp),
        hex(&Sha256::digest(canonical.as_bytes()))
    )
}

/// `20260830/auto/s3/aws4_request`.
pub fn scope(date_stamp: &str) -> String {
    format!("{date_stamp}/{REGION}/{SERVICE}/aws4_request")
}

/// The four-step key derivation. Each step keys the next, which is what makes a
/// leaked daily key useless tomorrow and in another region.
fn signing_key(secret: &str, date_stamp: &str) -> Vec<u8> {
    let date = hmac(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
    let region = hmac(&date, REGION.as_bytes());
    let service = hmac(&region, SERVICE.as_bytes());
    hmac(&service, b"aws4_request")
}

/// The finished `Authorization` header value.
pub fn authorization(
    credentials: &S3Credentials,
    amz_date: &str,
    date_stamp: &str,
    request: &CanonicalRequest<'_>,
) -> String {
    let to_sign = string_to_sign(amz_date, date_stamp, &request.render());
    let key = signing_key(&credentials.secret_access_key, date_stamp);
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    format!(
        "{ALGORITHM} Credential={}/{}, SignedHeaders={}, Signature={signature}",
        credentials.access_key_id,
        scope(date_stamp),
        request.signed_headers()
    )
}

/// HMAC-SHA256. Pinned against RFC 4231 in this module's tests, because every
/// other assertion here is downstream of it being right.
pub fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        // `Hmac` accepts a key of any length, so this is unreachable rather
        // than survivable, and pretending otherwise would put a `Result` on
        // every call site for a branch that cannot be taken.
        .expect("HMAC accepts a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Lowercase hex, which is what every field of SigV4 wants and what
/// `Sha256Digest` in `koru-voice-models` also wants.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The SHA-256 of a slice, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// **RFC 3986 unreserved, and everything else percent-encoded** — with `/`
/// spared when this is a path.
///
/// S3's own rule, and the one place a hand-rolled signer usually goes wrong:
/// `+` and `*` and `~` are *not* what `application/x-www-form-urlencoded`
/// would do with them, so a general-purpose form encoder produces a signature
/// that fails on exactly the keys with an unusual character in them.
pub fn uri_encode(value: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else if c == '/' && keep_slash {
            out.push('/');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The one tag we read out of an S3 XML response.
///
/// **Not an XML parser, and it must not become one.** Two responses in this
/// module carry a body at all, and between them they have exactly one field
/// this code needs; adding a parser would be a dependency, an attack surface
/// and a second thing to keep true, for one substring of an already
/// authenticated reply. If a third field is ever needed, that is the moment to
/// reach for a real parser rather than a third scan.
pub fn xml_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// How a file of this size splits into parts: `(part number, offset, length)`.
///
/// Separated from the upload so the arithmetic is testable without a socket —
/// the off-by-one that leaves a final zero-length part, or one part too many,
/// is the failure that costs a whole re-upload to discover.
pub fn parts(size: u64, part_size: u64) -> Vec<(u32, u64, u64)> {
    let mut out = Vec::new();
    let mut offset = 0u64;
    let mut number = 1u32;
    while offset < size {
        let length = part_size.min(size - offset);
        out.push((number, offset, length));
        offset += length;
        number += 1;
    }
    out
}

#[cfg(test)]
mod tests;
