// SPDX-License-Identifier: MPL-2.0

//! The four byte routes, from the client's side.
//!
//! ```text
//! HEAD   /v1/objects/uploads/{upload}
//! PATCH  /v1/objects/uploads/{upload}
//! POST   /v1/objects/uploads/{upload}/complete
//! GET    /v1/objects/{object-locator}
//! ```
//!
//! Three properties, each of which is a clause somewhere:
//!
//! - **Every URL here is an [`AdmittedUrl`]**, which only
//!   [`crate::ByteOrigins::admit`] can make. A client that could pass a string
//!   could pass a string a control plane chose.
//! - **`Authorization: Capability`, never `Bearer`.** The account token and
//!   the object capability are different authorities with different lifetimes,
//!   and at rung 7 one of them travels to a third party. A misrouted
//!   credential must fail loudly rather than be tried as the other kind.
//! - **Nothing here allocates in proportion to the transfer.** One bounded
//!   chunk is in flight at a time, in both directions, which is what
//!   `tests/bounded_memory.rs` measures rather than asserts.
//!
//! A download is a sequence of **bounded ranged GETs** rather than one long
//! stream, and that is deliberate: the worker verifies the capability per
//! request, so a chunk-sized request bounds the post-revocation read window to
//! one chunk instead of to however long a 2 GiB stream takes. It is the same
//! trade the upload already makes, read from the other end.

use koru_cloud_blob::{
    BlobCode, CAPABILITY_SCHEME, COMMITTED_OFFSET_HEADER,
    wire::{BlobErrorV1, CommittedOffsetV1},
};
use koru_store_core::errors::Code;

use crate::{
    allowlist::AdmittedUrl,
    ports::{PortFuture, TransferError},
};

/// How long one byte request may take.
///
/// **Derived, not chosen**: one chunk must fit inside one capability, so a
/// request that has not finished within a capability's life cannot finish at
/// all — its capability has expired underneath it. `CAPABILITY_LIFETIME` is
/// therefore the ceiling, and a client that waited longer would be waiting to
/// be told `capability_expired`.
pub const BYTE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(koru_cloud_blob::CAPABILITY_LIFETIME_SECS as u64);

/// The largest error body this client reads from a byte route. A refusal is a
/// small JSON object; a success carries bytes and is streamed.
const MAX_ERROR_BODY: usize = 8 * 1024;

/// The object plane, as a transfer needs it.
pub trait BlobObjectPlane: Send + Sync {
    /// §4.1 — the synced length of the session's staging file, which **is**
    /// the contiguous durable prefix. The authority for a resume.
    fn committed_offset(
        &self,
        url: AdmittedUrl,
        capability: String,
    ) -> PortFuture<'_, Result<u64, TransferError>>;

    /// §4.2 — one bounded chunk. `total` is the object's exact length and
    /// must equal the capability's bound length.
    fn patch(
        &self,
        url: AdmittedUrl,
        capability: String,
        start: u64,
        total: u64,
        body: Vec<u8>,
    ) -> PortFuture<'_, Result<u64, TransferError>>;

    /// §4.3 — `202`, and the hash runs as the worker's own job.
    ///
    /// `url` is the **session's** URL, as allocation returned it; the
    /// `/complete` segment is this client's to append, because the contract
    /// returns one byte URL per session and not one per route.
    fn complete(
        &self,
        url: AdmittedUrl,
        capability: String,
    ) -> PortFuture<'_, Result<(), TransferError>>;

    /// §4.4 — `[start, start + length)` of the object, as bytes.
    ///
    /// `length` is one chunk at most, which is what keeps this a bounded
    /// allocation rather than a copy of the object.
    fn read_range(
        &self,
        url: AdmittedUrl,
        capability: String,
        start: u64,
        length: u64,
    ) -> PortFuture<'_, Result<Vec<u8>, TransferError>>;
}

/// The real one.
pub struct HttpObjectPlane {
    client: reqwest::Client,
}

impl HttpObjectPlane {
    pub fn new() -> Result<Self, TransferError> {
        let client = reqwest::Client::builder()
            // A byte route that answered `302` would be redirecting a person's
            // file, with its capability attached, to whatever it named.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(BYTE_TIMEOUT)
            .build()
            .map_err(|_| TransferError::Unreachable)?;
        Ok(Self { client })
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    fn authorization(capability: &str) -> String {
        format!("{CAPABILITY_SCHEME} {capability}")
    }
}

/// What a refusal actually said, asked in the order a service is able to say
/// it.
///
/// **The header comes first, because one route cannot answer with a body.**
/// §4.1's resume authority is a `HEAD`, and HTTP forbids a body on the answer
/// to one — so the `BlobErrorV1` the worker writes is stripped before it
/// arrives and there is nothing to parse. That is not an edge case: the resume
/// read is the *first* byte request of every upload, so a refusal that stops a
/// transfer before it starts is exactly the one that reaches here bodiless.
///
/// The status is the last resort and it is a guess, so it now guesses in the
/// direction that claims no more than a status can support. `401` is two
/// codes, and answering `capability_expired` sent a person to check the clocks
/// on two machines for hours while the real answer was an issuer that did not
/// match. `capability_invalid` is the registry's "was not admitted", which is
/// precisely as much as a bare status says; it also parks the session for a
/// trigger instead of spinning on renewals that a control plane would keep
/// signing identically.
async fn refusal(response: reqwest::Response) -> TransferError {
    let status = response.status().as_u16();
    if let Some(code) = response
        .headers()
        .get(koru_cloud_blob::BLOB_CODE_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(BlobCode::parse)
    {
        return TransferError::Refused(code);
    }
    let body = response.bytes().await.unwrap_or_default();
    if body.len() <= MAX_ERROR_BODY
        && let Ok(error) = serde_json::from_slice::<BlobErrorV1>(&body)
    {
        return TransferError::Refused(error.code);
    }
    match status {
        401 => TransferError::Refused(BlobCode::CapabilityInvalid),
        403 => TransferError::Refused(BlobCode::WrongObject),
        404 => TransferError::Refused(BlobCode::ObjectMissing),
        409 => TransferError::Refused(BlobCode::RangeConflict),
        410 => TransferError::Refused(BlobCode::UploadExpired),
        429 | 500..=599 => TransferError::Refused(BlobCode::StorageUnavailable),
        _ => TransferError::Local(Code::InvalidRequest),
    }
}

impl BlobObjectPlane for HttpObjectPlane {
    fn committed_offset(
        &self,
        url: AdmittedUrl,
        capability: String,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        Box::pin(async move {
            let response = self
                .client
                .head(url.into_url())
                .header(
                    reqwest::header::AUTHORIZATION,
                    Self::authorization(&capability),
                )
                .send()
                .await
                .map_err(|_| TransferError::Unreachable)?;
            if !response.status().is_success() {
                return Err(refusal(response).await);
            }
            // The header is the answer — axum runs the `GET` handler for a
            // `HEAD` and discards the body, so the body is not there to read.
            let offset = response
                .headers()
                .get(COMMITTED_OFFSET_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            match offset {
                Some(offset) => Ok(offset),
                // Fall back to the body for a caller that sent `GET` rather
                // than `HEAD`; a resume authority that answered neither is a
                // service this client will not guess for.
                None => {
                    let body = response
                        .bytes()
                        .await
                        .map_err(|_| TransferError::Unreachable)?;
                    serde_json::from_slice::<CommittedOffsetV1>(&body)
                        .map(|value| value.committed_offset)
                        .map_err(|_| TransferError::Unreachable)
                }
            }
        })
    }

    fn patch(
        &self,
        url: AdmittedUrl,
        capability: String,
        start: u64,
        total: u64,
        body: Vec<u8>,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        Box::pin(async move {
            if body.is_empty() {
                // R15: a zero-length chunk is not a piece of anything. Refused
                // here rather than sent, because the answer is knowable.
                return Err(TransferError::Refused(BlobCode::MalformedRange));
            }
            let end = start + body.len() as u64 - 1;
            let response = self
                .client
                .patch(url.into_url())
                .header(
                    reqwest::header::AUTHORIZATION,
                    Self::authorization(&capability),
                )
                .header(
                    reqwest::header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total}"),
                )
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(body)
                .send()
                .await
                .map_err(|_| TransferError::Unreachable)?;
            if !response.status().is_success() {
                return Err(refusal(response).await);
            }
            let body = response
                .bytes()
                .await
                .map_err(|_| TransferError::Unreachable)?;
            serde_json::from_slice::<CommittedOffsetV1>(&body)
                .map(|value| value.committed_offset)
                .map_err(|_| TransferError::Unreachable)
        })
    }

    fn complete(
        &self,
        url: AdmittedUrl,
        capability: String,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        Box::pin(async move {
            let response = self
                .client
                .post(url.join_segment("complete").into_url())
                .header(
                    reqwest::header::AUTHORIZATION,
                    Self::authorization(&capability),
                )
                .header(reqwest::header::CONTENT_LENGTH, "0")
                .send()
                .await
                .map_err(|_| TransferError::Unreachable)?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(refusal(response).await)
            }
        })
    }

    fn read_range(
        &self,
        url: AdmittedUrl,
        capability: String,
        start: u64,
        length: u64,
    ) -> PortFuture<'_, Result<Vec<u8>, TransferError>> {
        Box::pin(async move {
            if length == 0 || length > koru_cloud_blob::CHUNK {
                return Err(TransferError::Refused(BlobCode::MalformedRange));
            }
            let end = start + length - 1;
            let mut response = self
                .client
                .get(url.into_url())
                .header(
                    reqwest::header::AUTHORIZATION,
                    Self::authorization(&capability),
                )
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send()
                .await
                .map_err(|_| TransferError::Unreachable)?;
            if !response.status().is_success() {
                return Err(refusal(response).await);
            }
            // Read with a reserved buffer of exactly the chunk asked for, and
            // refuse anything longer as it arrives. A service that answered a
            // ranged request with the whole object would otherwise be allowed
            // to decide this client's memory.
            let mut bytes = Vec::with_capacity(length as usize);
            while let Some(piece) = response
                .chunk()
                .await
                .map_err(|_| TransferError::Unreachable)?
            {
                if bytes.len() + piece.len() > length as usize {
                    return Err(TransferError::Refused(BlobCode::LengthMismatch));
                }
                bytes.extend_from_slice(&piece);
            }
            if bytes.len() as u64 != length {
                return Err(TransferError::Refused(BlobCode::LengthMismatch));
            }
            Ok(bytes)
        })
    }
}
