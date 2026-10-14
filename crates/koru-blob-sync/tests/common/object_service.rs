// SPDX-License-Identifier: MPL-2.0

//! **An in-process object service: the four byte routes, and nothing else.**
//!
//! ```text
//! HEAD   /v1/objects/uploads/{upload}
//! PATCH  /v1/objects/uploads/{upload}
//! POST   /v1/objects/uploads/{upload}/complete
//! GET    /v1/objects/{object-locator}
//! ```
//!
//! The engine's integration suites used to run against the real object worker,
//! which made this public crate dev-depend on the hosted service. This is what
//! they run against instead; the hosted service, which is not in this
//! repository, still runs the same suites against its real worker.
//!
//! **What keeps a fake honest is that it decides nothing of its own.** Every
//! admission here is the shared contract's: the `Capability` scheme
//! ([`binding::credential`]), the signature, purpose, audience, region and
//! window ([`verify_blob_capability`]), the route binding ([`check_bindings`]),
//! the `Content-Range` grammar and its admission ([`parse_content_range`],
//! [`admit_range`]), the stable codes, their statuses and the
//! [`BLOB_CODE_HEADER`] a bodiless `HEAD` refusal is read from. What is this
//! file's own is storage: a temporary directory, a staging file per session,
//! a published file per object. And `tests/object_service.rs` holds it to the
//! `cloud-blob-object-v1` vectors the real worker is held to — every row the
//! worker owns that a volume without a database can decide — so a fake that
//! drifted from the contract fails there rather than passing the engine.
//!
//! What it leaves out is everything the client cannot observe: the journal,
//! the report to the control plane, crash recovery, and reclamation.
//!
//! **Bounded, because `bounded_memory.rs` measures it too.** It shares a
//! process with the client it serves, so a fake that buffered an object would
//! be counted against the client. A chunk is the most a request holds, the
//! hash runs over fixed buffers, and a download streams through a bounded
//! channel — the same shape as the worker, for the same reason.

use std::{
    collections::HashMap,
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path as Route, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use koru_account_core::workspace_authorization::Region;
use koru_account_session::{
    TokenCapabilityVerifier,
    cloud::blob::{BlobCapabilityClaimsV1, BlobPurpose, verify_blob_capability},
};
use koru_cloud_blob::{
    BLOB_CODE_HEADER, BindingWorld, BlobCode, CAPABILITY_SCHEME, CHUNK, COMMITTED_OFFSET_HEADER,
    ObjectLocator, admit_range, binding, check_bindings, object_key, parse_content_range,
    range::RangeOutcome,
    staging_name,
    wire::{BlobErrorV1, CommittedOffsetV1, VerifyingV1},
};
use koru_device::cloud::CloudId;

/// Twice the chunk plus slack, as the worker sets it: above the contract's
/// bound, so a body longer than its declared range reaches the handler and is
/// answered `length_mismatch` (R12) rather than refused without a code.
const BODY_LIMIT: usize = (CHUNK as usize) * 2 + 1024;
/// How far a download may read ahead of the socket.
const READAHEAD: usize = 4;
const BUFFER: usize = 64 * 1024;

/// One deployment's byte plane: a region, the audience its capabilities must
/// name, and the issuer and key it trusts.
struct Service {
    region: Region,
    audience: String,
    verifier: TokenCapabilityVerifier,
    root: PathBuf,
    /// Sessions whose verification failed, so a later `PATCH` or `complete`
    /// answers what the worker answers instead of appending to nothing.
    failures: Mutex<HashMap<CloudId, BlobCode>>,
}

/// The fake on a real loopback socket, for as long as this value lives.
pub struct ObjectService {
    pub base: String,
    root: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    served: Option<tokio::task::JoinHandle<()>>,
    _volume: tempfile::TempDir,
}

impl ObjectService {
    /// The US deployment the suites use, trusting [`super::ISSUER`]'s key.
    pub async fn start() -> Self {
        Self::start_in(Region::Us, super::AUDIENCE).await
    }

    /// A deployment in `region` answering for `audience`.
    pub async fn start_in(region: Region, audience: &str) -> Self {
        let volume = tempfile::tempdir().expect("a private volume");
        let root = volume.path().to_path_buf();
        for directory in ["staging", "objects", "quarantine"] {
            std::fs::create_dir_all(root.join(directory)).expect("a volume directory");
        }
        let verifier = TokenCapabilityVerifier::new(
            region,
            super::ISSUER.to_owned(),
            [(
                super::KEY_ID.to_owned(),
                ed25519_dalek::SigningKey::from_bytes(&super::SEED).verifying_key(),
            )],
        )
        .expect("a verifier");
        let service = Arc::new(Service {
            region,
            audience: audience.to_owned(),
            verifier,
            root: root.clone(),
            failures: Mutex::new(HashMap::new()),
        });
        let router = Router::new()
            .route(
                "/v1/objects/uploads/{upload}",
                get(head_upload).patch(patch_upload),
            )
            .route("/v1/objects/uploads/{upload}/complete", post(complete))
            .route("/v1/objects/{locator}", get(read_object))
            .layer(DefaultBodyLimit::max(BODY_LIMIT))
            .with_state(service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port");
        let port = listener.local_addr().expect("bound").port();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let served = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            root,
            stop: Some(stop),
            served: Some(served),
            _volume: volume,
        }
    }

    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(served) = self.served.take() {
            let _ = served.await;
        }
    }

    /// The published object at `key`, if verification has published it — the
    /// volume, looked at directly, for the vector tests that assert on it.
    pub fn object_length(&self, key: &str) -> Option<u64> {
        std::fs::metadata(self.root.join("objects").join(key))
            .ok()
            .map(|meta| meta.len())
    }

    /// The synced length of a session's staging file.
    pub fn committed_offset(&self, upload: CloudId) -> u64 {
        staged_length(&staging_path(&self.root, upload))
    }

    /// Whether a session's staging file was set aside rather than destroyed.
    pub fn quarantined(&self, upload: CloudId) -> bool {
        self.root
            .join("quarantine")
            .join(staging_name(upload))
            .is_file()
    }

    /// Remove a published object's bytes, leaving everything that names it —
    /// G04's world, which a real volume reaches only by losing a file.
    pub fn lose_object(&self, key: &str) {
        let _ = std::fs::remove_file(self.root.join("objects").join(key));
    }

    /// Whether the object at `locator` is published yet — asked the way a
    /// client asks, over the byte route.
    pub async fn published(
        &self,
        locator: ObjectLocator,
        dataset: (CloudId, CloudId),
        size: u64,
        hash: &str,
    ) -> bool {
        let subject = CloudId::from_bytes(*locator.as_uuid().as_bytes());
        let read = super::sign(&super::claims(
            BlobPurpose::Read,
            dataset,
            subject,
            subject,
            size,
            hash,
        ));
        reqwest::Client::new()
            .get(format!("{}/v1/objects/{locator}", self.base))
            .header("authorization", format!("{CAPABILITY_SCHEME} {read}"))
            .header("range", "bytes=0-0")
            .send()
            .await
            .map(|response| response.status() == 206)
            .unwrap_or(false)
    }
}

impl Drop for ObjectService {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

// ----------------------------------------------------------------- storage

fn staging_path(root: &Path, upload: CloudId) -> PathBuf {
    root.join("staging").join(staging_name(upload))
}

fn staged_length(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// A storage fault the contract has a code for.
fn unavailable(_: std::io::Error) -> BlobCode {
    BlobCode::StorageUnavailable
}

/// Append `body` at `start`, which admission has already proved is the
/// committed offset.
fn append(path: &Path, start: u64, body: &[u8]) -> Result<u64, BlobCode> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(unavailable)?;
    if file.metadata().map_err(unavailable)?.len() != start {
        return Err(BlobCode::RangeConflict);
    }
    file.write_all(body).map_err(unavailable)?;
    file.sync_data().map_err(unavailable)?;
    Ok(start + body.len() as u64)
}

/// Whether a replayed range carries the bytes already on the disk — the only
/// thing that separates a lost acknowledgement from a client that changed its
/// mind.
fn staged_matches(path: &Path, start: u64, body: &[u8]) -> Result<bool, BlobCode> {
    let mut file = std::fs::File::open(path).map_err(unavailable)?;
    file.seek(SeekFrom::Start(start)).map_err(unavailable)?;
    let mut buffer = vec![0u8; BUFFER];
    let mut compared = 0;
    while compared < body.len() {
        let take = (body.len() - compared).min(BUFFER);
        file.read_exact(&mut buffer[..take]).map_err(unavailable)?;
        if buffer[..take] != body[compared..compared + take] {
            return Ok(false);
        }
        compared += take;
    }
    Ok(true)
}

/// Hash the staging file in fixed buffers, then publish it or set it aside.
fn verify(service: &Service, upload: CloudId, claims: &BlobCapabilityClaimsV1) {
    let staged = staging_path(&service.root, upload);
    let digest = (|| -> std::io::Result<String> {
        let mut file = std::fs::File::open(&staged)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; BUFFER];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(format!("b3:{}", hasher.finalize().to_hex()))
    })();
    if digest.as_deref().ok() == Some(claims.bh.as_str()) {
        let locator = ObjectLocator::from_uuid(claims.loc.as_uuid());
        let object = service
            .root
            .join("objects")
            .join(object_key(claims.rgn, claims.wsp, claims.inc, locator));
        let published = object
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|()| std::fs::rename(&staged, &object));
        if published.is_ok() {
            return;
        }
    }
    // The failure first, so a session is refused from the moment its
    // staging file is no longer there to append to.
    if let Ok(mut failures) = service.failures.lock() {
        failures.insert(upload, BlobCode::HashMismatch);
    }
    let _ = std::fs::rename(
        &staged,
        service.root.join("quarantine").join(staging_name(upload)),
    );
}

fn failure(service: &Service, upload: CloudId) -> Option<BlobCode> {
    service
        .failures
        .lock()
        .ok()
        .and_then(|failures| failures.get(&upload).copied())
}

// ------------------------------------------------------------------ routes

/// The refusal body, and the code again in a header because a `HEAD` is
/// answered without a body.
fn refuse(code: BlobCode) -> Response {
    let status = StatusCode::from_u16(code.status().unwrap_or(500))
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::HeaderName::from_static(BLOB_CODE_HEADER),
                HeaderValue::from_static(code.as_str()),
            ),
        ],
        serde_json::to_vec(&BlobErrorV1::from(code)).unwrap_or_default(),
    )
        .into_response()
}

/// Verify a capability for exactly the purpose the route implies, and bind it
/// to the id in the URL. Like the worker, this holds no account database, so
/// the workspace, incarnation and epoch bindings are not its to decide.
fn admit(
    service: &Service,
    headers: &HeaderMap,
    purpose: BlobPurpose,
    subject: CloudId,
) -> Result<BlobCapabilityClaimsV1, BlobCode> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let token = binding::credential(value, CAPABILITY_SCHEME)?;
    let verified = verify_blob_capability(
        &service.verifier,
        token.as_bytes(),
        purpose,
        &service.audience,
        service.region,
        now_millis(),
    )
    .map_err(koru_cloud_blob::code::from_verification_error)?;
    let claims = verified.claims().clone();
    check_bindings(
        &claims,
        &BindingWorld {
            route_subject: Some(subject),
            ..BindingWorld::default()
        },
    )?;
    Ok(claims)
}

/// An id in the path that does not parse names no object, and a capability
/// can therefore never name it.
fn upload_id(raw: &str) -> Result<CloudId, BlobCode> {
    uuid::Uuid::parse_str(raw)
        .ok()
        .filter(|id| id.hyphenated().to_string() == raw)
        .map(|id| CloudId::from_bytes(*id.as_bytes()))
        .ok_or(BlobCode::WrongObject)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, BlobCode> + Send + 'static,
) -> Result<T, BlobCode> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or(Err(BlobCode::StorageUnavailable))
}

fn offset_answer(offset: u64) -> Response {
    let mut response = (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        serde_json::to_vec(&CommittedOffsetV1 {
            committed_offset: offset,
        })
        .unwrap_or_default(),
    )
        .into_response();
    response.headers_mut().insert(
        header::HeaderName::from_static(COMMITTED_OFFSET_HEADER),
        HeaderValue::from(offset),
    );
    response
}

/// §4.1 — the synced length of the staging file is the resume authority.
/// Registered as `get`: axum answers a `HEAD` with the `GET` handler and drops
/// the body, exactly as it does for the worker.
async fn head_upload(
    State(service): State<Arc<Service>>,
    Route(upload): Route<String>,
    headers: HeaderMap,
) -> Response {
    let answer = async {
        let upload = upload_id(&upload)?;
        admit(&service, &headers, BlobPurpose::Write, upload)?;
        Ok(staged_length(&staging_path(&service.root, upload)))
    };
    match answer.await {
        Ok(offset) => offset_answer(offset),
        Err(code) => refuse(code),
    }
}

/// §4.2 — one bounded chunk, appended and synced before it is acknowledged.
async fn patch_upload(
    State(service): State<Arc<Service>>,
    Route(upload): Route<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let answer = async {
        let upload = upload_id(&upload)?;
        let claims = admit(&service, &headers, BlobPurpose::Write, upload)?;
        if let Some(code) = failure(&service, upload) {
            return Err(code);
        }
        let raw = headers
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .ok_or(BlobCode::MalformedRange)?;
        let range = parse_content_range(raw)?;
        let path = staging_path(&service.root, upload);
        let committed = staged_length(&path);
        match admit_range(range, committed, claims.len, body.len() as u64)? {
            RangeOutcome::Append { start, .. } => {
                blocking(move || append(&path, start, &body)).await
            }
            RangeOutcome::Replay {
                start,
                committed_after,
                ..
            } => match blocking(move || staged_matches(&path, start, &body)).await? {
                true => Ok(committed_after),
                false => Err(BlobCode::RangeConflict),
            },
        }
    };
    match answer.await {
        Ok(offset) => offset_answer(offset),
        Err(code) => refuse(code),
    }
}

/// §4.3 — the exact length, then `verifying` at once; the hash runs as its own
/// job, off the request.
async fn complete(
    State(service): State<Arc<Service>>,
    Route(upload): Route<String>,
    headers: HeaderMap,
) -> Response {
    let answer = async {
        let upload = upload_id(&upload)?;
        let claims = admit(&service, &headers, BlobPurpose::Write, upload)?;
        if let Some(code) = failure(&service, upload) {
            return Err(code);
        }
        // V03 — completing before the declared length arrived.
        if staged_length(&staging_path(&service.root, upload)) != claims.len {
            return Err(BlobCode::LengthMismatch);
        }
        let verifying = service.clone();
        tokio::task::spawn_blocking(move || verify(&verifying, upload, &claims));
        Ok(())
    };
    match answer.await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            [(header::CACHE_CONTROL, "no-store")],
            serde_json::to_vec(&VerifyingV1::default()).unwrap_or_default(),
        )
            .into_response(),
        Err(code) => refuse(code),
    }
}

/// §4.4 — the whole object or one range of it, with the headers a resuming
/// download reads. "Not yours" and "not there" are decided before anything
/// touches the volume, so they answer the same code (G02, G03).
async fn read_object(
    State(service): State<Arc<Service>>,
    Route(locator): Route<String>,
    headers: HeaderMap,
) -> Response {
    let answer = async {
        let locator = ObjectLocator::parse(&locator).ok_or(BlobCode::WrongObject)?;
        let subject = CloudId::from_bytes(*locator.as_uuid().as_bytes());
        let claims = admit(&service, &headers, BlobPurpose::Read, subject)?;
        let path = service
            .root
            .join("objects")
            .join(object_key(claims.rgn, claims.wsp, claims.inc, locator));
        // G04 — named, authorised, and the bytes are not there.
        let length = std::fs::metadata(&path)
            .map(|meta| meta.len())
            .map_err(|_| BlobCode::ObjectMissing)?;
        let (start, end) = requested_range(&headers, length)?;
        let mut file = std::fs::File::open(&path).map_err(|_| BlobCode::ObjectMissing)?;
        file.seek(SeekFrom::Start(start)).map_err(unavailable)?;
        Ok((locator, length, start, end, file))
    };
    let (locator, length, start, end, file) = match answer.await {
        Ok(found) => found,
        Err(code) => return refuse(code),
    };
    let span = end - start + 1;
    let partial = span != length;
    let mut response = Response::new(stream(file, span));
    if partial {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
    }
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(span));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{locator}-{length}\"")) {
        headers.insert(header::ETAG, etag);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0, no-store"),
    );
    if partial && let Ok(range) = HeaderValue::from_str(&format!("bytes {start}-{end}/{length}")) {
        headers.insert(header::CONTENT_RANGE, range);
    }
    response
}

/// `bytes=<start>-` or `bytes=<start>-<end>`, inclusive. A multi-range request
/// is answered whole, as the worker answers it.
fn requested_range(headers: &HeaderMap, length: u64) -> Result<(u64, u64), BlobCode> {
    let Some(raw) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok((0, length.saturating_sub(1)));
    };
    let span = raw.strip_prefix("bytes=").ok_or(BlobCode::MalformedRange)?;
    if span.contains(',') {
        return Ok((0, length.saturating_sub(1)));
    }
    let (start, end) = span.split_once('-').ok_or(BlobCode::MalformedRange)?;
    let number = |value: &str| -> Option<u64> {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
    };
    let start = number(start).ok_or(BlobCode::MalformedRange)?;
    let end = if end.is_empty() {
        length.saturating_sub(1)
    } else {
        number(end).ok_or(BlobCode::MalformedRange)?
    };
    if length == 0 || start > end || end >= length {
        return Err(BlobCode::MalformedRange);
    }
    Ok((start, end))
}

/// Fixed buffers on a blocking thread, handed to the socket through a bounded
/// channel: a slow reader costs `READAHEAD` buffers and never the object.
fn stream(mut file: std::fs::File, mut remaining: u64) -> Body {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(READAHEAD);
    tokio::task::spawn_blocking(move || {
        let mut buffer = vec![0u8; BUFFER];
        while remaining > 0 {
            let take = remaining.min(BUFFER as u64) as usize;
            let outcome = match file.read(&mut buffer[..take]) {
                Ok(0) => Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
                Ok(read) => {
                    remaining -= read as u64;
                    Ok(Bytes::copy_from_slice(&buffer[..read]))
                }
                Err(error) => Err(error),
            };
            let failed = outcome.is_err();
            if sender.blocking_send(outcome).is_err() || failed {
                return;
            }
        }
    });
    Body::from_stream(futures_util::stream::unfold(
        receiver,
        |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) },
    ))
}
