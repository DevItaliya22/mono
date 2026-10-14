// SPDX-License-Identifier: MPL-2.0

//! The upload, as the plan's §10 orders it.
//!
//! 1. the local object is **verified before any network request**;
//! 2. status answers durable, an existing session, or missing;
//! 3. missing allocates — idempotently by `(workspace, incarnation, hash)`, so
//!    two devices racing on one picture converge on one session (S08);
//! 4. the client streams bounded chunks from the local file, renewing the
//!    capability as needed;
//! 5. completion verifies and publishes, and only then is status durable.
//!
//! **The resume authority is the worker's `HEAD`, never the allocation's
//! `committed_offset`.** That field is advisory and the contract says so: the
//! worker holds the bytes and the API holds a projection of what it was last
//! told. A client that trusted the projection would re-send a chunk the worker
//! already has, or — worse — skip one it does not.
//!
//! **The source file is immutable by address**, so a resume needs no local
//! staging copy: the bytes at `hash` are the bytes at `hash`.

use std::io::Read as _;

use koru_cloud_blob::{
    BlobCode,
    wire::{AllocateRequestV1, AllocationV1, BlobStatusV1},
};
use koru_device::cloud::CloudId;
use koru_store_core::{WorkspaceId, errors::Code, model::BlobHash};

use crate::{
    allowlist::AdmittedUrl,
    engine::BlobEngine,
    ports::{CloudAccessFacts, LocalBlob, TransferError},
    retry::Disposition,
};

/// Where an upload ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadOutcome {
    /// Cloud already held it. Nothing was sent, which is the ordinary answer
    /// once two devices agree and is indistinguishable from a broken pass
    /// without being said.
    AlreadyDurable,
    /// The bytes are published and the mapping is committed.
    Durable { sent: u64 },
    /// Every byte is on the worker's disk and verification has not finished
    /// inside one capability's life. Not a failure: the session is terminal in
    /// neither direction and the next pass asks again.
    Verifying { sent: u64 },
}

/// How often the client asks whether verification has finished.
///
/// A second, because the answer is a single indexed read on the control plane
/// and the wait it is measuring is a disk read on another machine. Shorter
/// would be a poll loop; longer would leave a finished upload looking unfinished
/// for no reason.
pub const DURABILITY_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// **Derived**: one capability's life. Verification is a whole-file read on
/// the worker, and a client that waited longer than a capability lives would
/// be waiting past the point where it could do anything about the answer — its
/// session is still resumable, and the next pass is the retry.
pub const DURABILITY_CEILING: std::time::Duration =
    std::time::Duration::from_secs(koru_cloud_blob::CAPABILITY_LIFETIME_SECS as u64);

/// What one live session is, once a byte URL has been admitted.
struct Session {
    upload: CloudId,
    url: AdmittedUrl,
    capability: String,
    chunk: u64,
}

pub async fn run(
    engine: &BlobEngine,
    access: &CloudAccessFacts,
    workspace: WorkspaceId,
    blob: &LocalBlob,
) -> Result<UploadOutcome, TransferError> {
    // **§6's fourth point, before the first network call of this transfer.**
    // Asked again rather than trusted from the pass: a person who turned Cloud
    // off while the file before this one was uploading is entitled to have it
    // off for this one, and this is where a status read would otherwise be the
    // first packet to leave.
    engine.admit_cloud(workspace).await?;
    // **Before any network request** (§10 step 3). Uploading bytes that no
    // longer hash to their own name would publish something else entirely
    // under an address a document believes it knows, and the Store has already
    // copied the damaged file aside by the time this returns.
    engine
        .store()
        .verify_object(workspace, blob.hash.clone())
        .await?;

    if let BlobStatusV1::Durable { .. } = engine
        .control()
        .status(access.clone(), workspace, blob.hash.clone())
        .await?
    {
        return Ok(UploadOutcome::AlreadyDurable);
    }

    let mut session = match allocate(engine, access, workspace, blob).await? {
        Allocated::Durable => return Ok(UploadOutcome::AlreadyDurable),
        Allocated::Session(session) => session,
    };

    // The authority. The allocation's own `committed_offset` is a projection
    // of what the API was last told, and the worker is what holds the bytes.
    let mut committed = engine
        .objects()
        .committed_offset(session.url.clone(), session.capability.clone())
        .await?;
    let mut sent = 0u64;
    let mut reader = open_at(engine, workspace, &blob.hash, committed).await?;

    while committed < blob.size {
        let want = session.chunk.min(blob.size - committed);
        let (returned, chunk) = read_chunk(reader, want).await?;
        reader = returned;
        if chunk.len() as u64 != want {
            // The local file is shorter than the Store said. About this blob:
            // no retry can lengthen it, and the declared size is what the
            // capability is bound to.
            return Err(TransferError::Refused(BlobCode::LengthMismatch));
        }
        let length = chunk.len() as u64;
        match engine
            .objects()
            .patch(
                session.url.clone(),
                session.capability.clone(),
                committed,
                blob.size,
                chunk,
            )
            .await
        {
            Ok(offset) => {
                sent += length;
                committed = offset;
            }
            Err(error) => {
                match crate::retry::classify(&error) {
                    // Nothing is discarded: a fresh capability for the same
                    // session, and the same offset.
                    Disposition::Renew => {
                        session = renew(engine, access, workspace, session.upload).await?;
                    }
                    // The client and the worker disagree about the disk. The
                    // worker is right.
                    Disposition::Resync => {
                        committed = engine
                            .objects()
                            .committed_offset(session.url.clone(), session.capability.clone())
                            .await?;
                    }
                    // The session is gone. The local object has not moved, so
                    // allocate a new one and ask its own `HEAD`.
                    Disposition::Restart => {
                        session = match allocate(engine, access, workspace, blob).await? {
                            Allocated::Durable => return Ok(UploadOutcome::AlreadyDurable),
                            Allocated::Session(session) => session,
                        };
                        committed = engine
                            .objects()
                            .committed_offset(session.url.clone(), session.capability.clone())
                            .await?;
                    }
                    // Everything else is the engine's to back off or park —
                    // but a session whose *declared length* is wrong can
                    // never succeed, and allocation is idempotent by the
                    // triple, so leaving it would hand the same wrong session
                    // back for the seven days of the resume window. Abort it
                    // so the next pass declares the size the file has now.
                    Disposition::ParkUnit => {
                        if matches!(
                            error,
                            TransferError::Refused(BlobCode::LengthMismatch)
                                | TransferError::Refused(BlobCode::MalformedRange)
                        ) {
                            let _ = engine
                                .control()
                                .abort(access.clone(), workspace, session.upload)
                                .await;
                        }
                        return Err(error);
                    }
                    _ => return Err(error),
                }
                // **The reader is repositioned on every recovery path**, and
                // that is not tidiness: the chunk just read was consumed, so
                // carrying on without this would send the *next* chunk's bytes
                // at the failed chunk's offset. The file is immutable by
                // address, so re-reading is always the same bytes.
                reader = open_at(engine, workspace, &blob.hash, committed).await?;
            }
        }
    }

    match engine
        .objects()
        .complete(session.url.clone(), session.capability.clone())
        .await
    {
        Ok(()) => {}
        Err(error) if crate::retry::classify(&error) == Disposition::Renew => {
            let renewed = renew(engine, access, workspace, session.upload).await?;
            engine
                .objects()
                .complete(renewed.url.clone(), renewed.capability.clone())
                .await?;
        }
        Err(error) => return Err(error),
    }

    poll_to_durable(engine, access, workspace, &blob.hash, sent).await
}

enum Allocated {
    Durable,
    Session(Session),
}

async fn allocate(
    engine: &BlobEngine,
    access: &CloudAccessFacts,
    workspace: WorkspaceId,
    blob: &LocalBlob,
) -> Result<Allocated, TransferError> {
    let allocation = engine
        .control()
        .allocate(
            access.clone(),
            workspace,
            blob.hash.clone(),
            AllocateRequestV1 {
                size: blob.size,
                mime: mime_for(blob),
                filename: blob.filename.clone(),
            },
        )
        .await?;
    match allocation {
        AllocationV1::Durable { .. } => Ok(Allocated::Durable),
        AllocationV1::Uploading {
            upload_id,
            chunk_size,
            byte_url,
            capability,
            ..
        } => Ok(Allocated::Session(Session {
            upload: upload_id,
            // **The clause this line is.** A control response is not authority
            // to send a person's file anywhere.
            url: access
                .byte_origins
                .admit(&byte_url)
                .map_err(TransferError::Origin)?,
            capability,
            // **A client never hard-codes the chunk size**, so a later
            // provider can raise it without touching an installed app — and
            // within *this* contract version §4.2 refuses a chunk above
            // `CHUNK` outright, so a larger answer is a service disagreeing
            // with its own contract and the smaller of the two is the only
            // one that could be accepted. It also means a service cannot
            // choose this device's memory.
            chunk: chunk_size.clamp(1, koru_cloud_blob::CHUNK),
        })),
    }
}

async fn renew(
    engine: &BlobEngine,
    access: &CloudAccessFacts,
    workspace: WorkspaceId,
    upload: CloudId,
) -> Result<Session, TransferError> {
    match engine
        .control()
        .renew(access.clone(), workspace, upload)
        .await?
    {
        // Renewal never creates a second session, so a `durable` answer here
        // means the object was published while this client was mid-chunk —
        // which is S08's other device finishing first.
        AllocationV1::Durable { .. } => Err(TransferError::Refused(BlobCode::UploadExpired)),
        AllocationV1::Uploading {
            upload_id,
            chunk_size,
            byte_url,
            capability,
            ..
        } => Ok(Session {
            upload: upload_id,
            url: access
                .byte_origins
                .admit(&byte_url)
                .map_err(TransferError::Origin)?,
            capability,
            chunk: chunk_size.clamp(1, koru_cloud_blob::CHUNK),
        }),
    }
}

/// Wait for the worker's verification job, bounded.
///
/// A terminal `missing` with a `failure` is the answer §4.3 promises is
/// visible on status — `hash_mismatch`, most of all, which means the bytes
/// this device sent are not the bytes it claimed.
async fn poll_to_durable(
    engine: &BlobEngine,
    access: &CloudAccessFacts,
    workspace: WorkspaceId,
    hash: &BlobHash,
    sent: u64,
) -> Result<UploadOutcome, TransferError> {
    let deadline = std::time::Instant::now() + DURABILITY_CEILING;
    loop {
        match engine
            .control()
            .status(access.clone(), workspace, hash.clone())
            .await?
        {
            BlobStatusV1::Durable { .. } => return Ok(UploadOutcome::Durable { sent }),
            BlobStatusV1::Missing {
                failure: Some(code),
            } => return Err(TransferError::Refused(code)),
            // Still `uploading` on the API's projection, or `missing` with no
            // reason yet: the worker has the bytes and has not reported.
            BlobStatusV1::Missing { failure: None } | BlobStatusV1::Uploading { .. } => {}
        }
        if std::time::Instant::now() >= deadline {
            return Ok(UploadOutcome::Verifying { sent });
        }
        tokio::time::sleep(DURABILITY_POLL).await;
    }
}

/// A reader positioned at `offset`.
///
/// §9 froze `open_blob` without `Seek`, so a resume reads and discards the
/// prefix once. That is one local pass on the rare path — a resume — rather
/// than a wider port, and it is bounded: the discard runs through a fixed
/// buffer and never materialises what it skips.
async fn open_at(
    engine: &BlobEngine,
    workspace: WorkspaceId,
    hash: &BlobHash,
    offset: u64,
) -> Result<Box<dyn std::io::Read + Send>, TransferError> {
    let reader = engine.store().open_blob(workspace, hash.clone()).await?;
    if offset == 0 {
        return Ok(reader);
    }
    blocking(move || {
        let mut reader = reader;
        let skipped =
            std::io::copy(&mut (&mut reader).take(offset), &mut std::io::sink()).map_err(io)?;
        // A local file shorter than the offset the worker already holds is not
        // the object the capability is bound to.
        if skipped != offset {
            return Err(TransferError::Refused(BlobCode::LengthMismatch));
        }
        Ok(reader)
    })
    .await
}

/// Read exactly `want` bytes, or as many as there are.
///
/// **The one allocation that scales with anything**, and it scales with the
/// chunk rather than with the file: `want` is at most `CHUNK`.
async fn read_chunk(
    reader: Box<dyn std::io::Read + Send>,
    want: u64,
) -> Result<(Box<dyn std::io::Read + Send>, Vec<u8>), TransferError> {
    blocking(move || {
        let mut reader = reader;
        let mut buffer = Vec::with_capacity(want as usize);
        (&mut reader)
            .take(want)
            .read_to_end(&mut buffer)
            .map_err(io)?;
        Ok((reader, buffer))
    })
    .await
}

fn io(_error: std::io::Error) -> TransferError {
    TransferError::Local(Code::StorageFailed)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, TransferError> + Send + 'static,
) -> Result<T, TransferError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or(Err(TransferError::Local(Code::Internal)))
}

/// The media type an allocation declares. A blank one is the generic type
/// rather than an invented specific one: the Store recorded what it was given,
/// and guessing `image/png` because a node looked like a picture is how a
/// document ends up describing bytes it has not read.
fn mime_for(blob: &LocalBlob) -> String {
    let mime = blob.mime.trim();
    if mime.is_empty() {
        "application/octet-stream".to_string()
    } else {
        mime.to_string()
    }
}
