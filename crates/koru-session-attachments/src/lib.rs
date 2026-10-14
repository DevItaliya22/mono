// SPDX-License-Identifier: MPL-2.0

//! Root-confined, provider-neutral attachment ingestion.
//!
//! This crate owns byte/type validation, bounded CPU decoding, atomic files,
//! and preview caching. It owns no IPC, PTY, provider policy, UI, root
//! discovery, or logging. Callers hand it a resolved [`SessionPaths`] and run
//! every method that can decode or touch disk on a worker.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Cursor, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use image::{DynamicImage, ImageFormat, ImageReader};
use koru_session_paths::{AtomicPath, SessionPaths, VerifiedAttachmentExtension};
use koru_session_protocol::{
    Attachment, AttachmentId, AttachmentKind, AttachmentSource, AttachmentStatus, DeliveryMode,
    DeliveryState, ErrorCode, ImageDimensions, SessionId,
};

pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const DEFAULT_MAX_DIMENSION: u32 = 8_192;
pub const DEFAULT_MAX_DECODED_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_PER_SESSION: usize = 16;
pub const DEFAULT_PREVIEW_EDGE: u32 = 512;

/// Resource policy applied before a pending attachment can exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachmentLimits {
    pub max_bytes: u64,
    pub max_dimension: u32,
    pub max_decoded_bytes: u64,
    pub max_per_session: usize,
    pub preview_edge: u32,
}

impl Default for AttachmentLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            max_dimension: DEFAULT_MAX_DIMENSION,
            max_decoded_bytes: DEFAULT_MAX_DECODED_BYTES,
            max_per_session: DEFAULT_MAX_PER_SESSION,
            preview_edge: DEFAULT_PREVIEW_EDGE,
        }
    }
}

impl AttachmentLimits {
    /// Read bounded process configuration once at the host boundary.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            max_bytes: env_number("KORU_ATTACHMENT_MAX_BYTES", defaults.max_bytes),
            max_dimension: env_number("KORU_ATTACHMENT_MAX_DIMENSION", defaults.max_dimension),
            max_decoded_bytes: env_number(
                "KORU_ATTACHMENT_MAX_DECODED_BYTES",
                defaults.max_decoded_bytes,
            ),
            max_per_session: env_number(
                "KORU_ATTACHMENT_MAX_PER_SESSION",
                defaults.max_per_session,
            ),
            preview_edge: env_number("KORU_ATTACHMENT_PREVIEW_EDGE", defaults.preview_edge),
        }
        .validated_or(defaults)
    }

    fn valid(self) -> bool {
        self.max_bytes > 0
            && self.max_dimension > 0
            && self.max_decoded_bytes >= 4
            && self.max_per_session > 0
            && self.preview_edge > 0
            && self.preview_edge <= self.max_dimension
    }

    fn validated_or(self, fallback: Self) -> Self {
        if self.valid() { self } else { fallback }
    }
}

fn env_number<T>(name: &str, fallback: T) -> T
where
    T: std::str::FromStr + Copy,
{
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

#[derive(Clone, Debug)]
pub struct IngestRequest {
    pub session_id: SessionId,
    pub attachment_id: AttachmentId,
    pub source: AttachmentSource,
    pub bytes: Vec<u8>,
    pub created_at_ms: i64,
}

/// A bounded PNG returned only in response to an explicit preview request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentPreview {
    pub attachment_id: AttachmentId,
    pub dimensions: ImageDimensions,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentStoreError {
    pub code: ErrorCode,
    message: &'static str,
}

impl AttachmentStoreError {
    fn new(code: ErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    pub fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for AttachmentStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for AttachmentStoreError {}

#[derive(Clone)]
struct Preview {
    dimensions: ImageDimensions,
    bytes: Arc<[u8]>,
}

#[derive(Clone)]
struct Stored {
    attachment: Attachment,
    extension: VerifiedAttachmentExtension,
    previews: BTreeMap<u32, Preview>,
}

#[derive(Default)]
struct State {
    active_sessions: BTreeSet<SessionId>,
    ended_sessions: BTreeSet<SessionId>,
    ingesting: BTreeSet<(SessionId, AttachmentId)>,
    stored: BTreeMap<(SessionId, AttachmentId), Stored>,
}

/// Live, memory-indexed attachment state backed by rooted atomic files.
pub struct AttachmentStore {
    paths: SessionPaths,
    limits: AttachmentLimits,
    state: Mutex<State>,
}

impl AttachmentStore {
    pub fn new(
        paths: SessionPaths,
        limits: AttachmentLimits,
    ) -> Result<Self, AttachmentStoreError> {
        if !limits.valid() {
            return Err(AttachmentStoreError::new(
                ErrorCode::InvalidRequest,
                "attachment limits are invalid",
            ));
        }
        Ok(Self {
            paths,
            limits,
            state: Mutex::new(State::default()),
        })
    }

    /// Register a newly created live session. This performs no filesystem I/O.
    pub fn session_started(&self, session_id: SessionId) {
        let mut state = self.lock();
        state.ended_sessions.remove(&session_id);
        state.active_sessions.insert(session_id);
    }

    /// Forget one ended session and remove its exact rooted directory.
    /// Call on a blocking worker.
    pub fn session_ended(&self, session_id: SessionId) -> Result<bool, AttachmentStoreError> {
        {
            let mut state = self.lock();
            state.active_sessions.remove(&session_id);
            state.ended_sessions.insert(session_id);
            state
                .ingesting
                .retain(|(session, _)| *session != session_id);
            state
                .stored
                .retain(|(session, _), _| *session != session_id);
        }
        self.paths
            .remove_session_attachments(session_id)
            .map_err(|_| io_failure())
    }

    /// Decode, validate, thumbnail, and atomically publish one image.
    ///
    /// No `Attachment` becomes visible until every worker-owned step succeeds.
    pub fn ingest(&self, request: IngestRequest) -> Result<Attachment, AttachmentStoreError> {
        self.reserve(&request)?;
        let key = (request.session_id, request.attachment_id);
        let prepared = prepare_image(&request.bytes, self.limits);
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.release(key);
                return Err(error);
            }
        };
        let atomic = self.paths.attachment_atomic_path(
            request.session_id,
            request.attachment_id,
            prepared.extension,
        );
        let write = self
            .paths
            .prepare_attachment_directory(request.session_id)
            .and_then(|_| write_atomic(&atomic, &request.bytes));
        if write.is_err() {
            self.release(key);
            return Err(io_failure());
        }

        let attachment = Attachment {
            id: request.attachment_id,
            session_id: request.session_id,
            kind: AttachmentKind::Image,
            source: request.source,
            media_type: prepared.media_type.into(),
            byte_count: request.bytes.len().try_into().unwrap_or(u64::MAX),
            dimensions: Some(prepared.dimensions),
            status: AttachmentStatus::Pending,
            delivery_mode: DeliveryMode::Unsupported,
            delivery: DeliveryState::Pending,
            turn_id: None,
            preview_available: true,
            created_at_ms: request.created_at_ms,
        };
        let mut previews = BTreeMap::new();
        previews.insert(self.limits.preview_edge, prepared.preview);
        let stored = Stored {
            attachment: attachment.clone(),
            extension: prepared.extension,
            previews,
        };
        let committed = {
            let mut state = self.lock();
            state.ingesting.remove(&key);
            if state.active_sessions.contains(&request.session_id) {
                state.stored.insert(key, stored);
                true
            } else {
                false
            }
        };
        if !committed {
            let _ = std::fs::remove_file(atomic.destination());
            return Err(AttachmentStoreError::new(
                ErrorCode::SessionExited,
                "session ended during attachment ingest",
            ));
        }
        Ok(attachment)
    }

    pub fn list(&self, session_id: SessionId) -> Result<Vec<Attachment>, AttachmentStoreError> {
        let state = self.lock();
        require_known_session(&state, session_id)?;
        Ok(state
            .stored
            .range((session_id, minimum_attachment_id())..=(session_id, maximum_attachment_id()))
            .map(|(_, stored)| stored.attachment.clone())
            .collect())
    }

    pub fn preview(
        &self,
        session_id: SessionId,
        attachment_id: AttachmentId,
        max_edge: u32,
    ) -> Result<AttachmentPreview, AttachmentStoreError> {
        if max_edge == 0 || max_edge > self.limits.preview_edge {
            return Err(AttachmentStoreError::new(
                ErrorCode::AttachmentLimit,
                "attachment preview size exceeds the configured limit",
            ));
        }
        let key = (session_id, attachment_id);
        if let Some(preview) = self
            .lock()
            .stored
            .get(&key)
            .and_then(|stored| stored.previews.get(&max_edge))
            .cloned()
        {
            return Ok(public_preview(attachment_id, preview));
        }

        let source = {
            let state = self.lock();
            require_known_session(&state, session_id)?;
            state
                .stored
                .get(&key)
                .and_then(|stored| stored.previews.get(&self.limits.preview_edge))
                .cloned()
                .ok_or_else(not_found)?
        };
        let image = decode_png(&source.bytes, self.limits.max_decoded_bytes)?;
        let preview = make_preview(&image, max_edge)?;
        let mut state = self.lock();
        require_known_session(&state, session_id)?;
        let stored = state.stored.get_mut(&key).ok_or_else(not_found)?;
        let preview = stored.previews.entry(max_edge).or_insert(preview).clone();
        Ok(public_preview(attachment_id, preview))
    }

    pub fn path(
        &self,
        session_id: SessionId,
        attachment_id: AttachmentId,
    ) -> Result<PathBuf, AttachmentStoreError> {
        let state = self.lock();
        require_known_session(&state, session_id)?;
        let stored = state
            .stored
            .get(&(session_id, attachment_id))
            .ok_or_else(not_found)?;
        Ok(self
            .paths
            .attachment_file(session_id, attachment_id, stored.extension))
    }

    pub fn remove(
        &self,
        session_id: SessionId,
        attachment_id: AttachmentId,
    ) -> Result<Attachment, AttachmentStoreError> {
        let key = (session_id, attachment_id);
        let stored = {
            let mut state = self.lock();
            require_known_session(&state, session_id)?;
            state.stored.remove(&key).ok_or_else(not_found)?
        };
        let path = self
            .paths
            .attachment_file(session_id, attachment_id, stored.extension);
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != io::ErrorKind::NotFound
        {
            self.lock().stored.insert(key, stored);
            return Err(io_failure());
        }
        let directory = self.paths.attachment_directory(session_id);
        let _ = std::fs::remove_dir(directory);
        let mut removed = stored.attachment;
        removed.status = AttachmentStatus::Removed;
        removed.preview_available = false;
        Ok(removed)
    }

    fn reserve(&self, request: &IngestRequest) -> Result<(), AttachmentStoreError> {
        let byte_count = u64::try_from(request.bytes.len()).unwrap_or(u64::MAX);
        if request.bytes.is_empty() || byte_count > self.limits.max_bytes {
            return Err(AttachmentStoreError::new(
                ErrorCode::AttachmentLimit,
                "attachment byte limit exceeded",
            ));
        }
        let key = (request.session_id, request.attachment_id);
        let mut state = self.lock();
        require_known_session(&state, request.session_id)?;
        if state.stored.contains_key(&key) || state.ingesting.contains(&key) {
            return Err(AttachmentStoreError::new(
                ErrorCode::InvalidTransition,
                "attachment id is already in use",
            ));
        }
        let count = state
            .stored
            .keys()
            .filter(|(session, _)| *session == request.session_id)
            .count()
            + state
                .ingesting
                .iter()
                .filter(|(session, _)| *session == request.session_id)
                .count();
        if count >= self.limits.max_per_session {
            return Err(AttachmentStoreError::new(
                ErrorCode::AttachmentLimit,
                "attachment count limit exceeded",
            ));
        }
        state.ingesting.insert(key);
        Ok(())
    }

    fn release(&self, key: (SessionId, AttachmentId)) {
        self.lock().ingesting.remove(&key);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }
}

struct PreparedImage {
    extension: VerifiedAttachmentExtension,
    media_type: &'static str,
    dimensions: ImageDimensions,
    preview: Preview,
}

fn prepare_image(
    bytes: &[u8],
    limits: AttachmentLimits,
) -> Result<PreparedImage, AttachmentStoreError> {
    let format = image::guess_format(bytes).map_err(|_| invalid_image())?;
    let (extension, media_type) = verified_format(format)?;
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| invalid_image())?;
    validate_dimensions(width, height, limits)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(limits.max_dimension);
    decode_limits.max_image_height = Some(limits.max_dimension);
    decode_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(decode_limits);
    let decoded = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => AttachmentStoreError::new(
            ErrorCode::AttachmentLimit,
            "attachment decode limit exceeded",
        ),
        _ => invalid_image(),
    })?;
    let preview = make_preview(&decoded, limits.preview_edge)?;
    Ok(PreparedImage {
        extension,
        media_type,
        dimensions: ImageDimensions { width, height },
        preview,
    })
}

fn validate_dimensions(
    width: u32,
    height: u32,
    limits: AttachmentLimits,
) -> Result<(), AttachmentStoreError> {
    let rgba_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(limit_error)?;
    if width == 0
        || height == 0
        || width > limits.max_dimension
        || height > limits.max_dimension
        || rgba_bytes > limits.max_decoded_bytes
    {
        return Err(limit_error());
    }
    Ok(())
}

fn verified_format(
    format: ImageFormat,
) -> Result<(VerifiedAttachmentExtension, &'static str), AttachmentStoreError> {
    match format {
        ImageFormat::Png => Ok((VerifiedAttachmentExtension::Png, "image/png")),
        ImageFormat::Jpeg => Ok((VerifiedAttachmentExtension::Jpeg, "image/jpeg")),
        ImageFormat::Gif => Ok((VerifiedAttachmentExtension::Gif, "image/gif")),
        ImageFormat::WebP => Ok((VerifiedAttachmentExtension::Webp, "image/webp")),
        _ => Err(AttachmentStoreError::new(
            ErrorCode::UnsupportedMedia,
            "attachment media type is unsupported",
        )),
    }
}

fn make_preview(image: &DynamicImage, max_edge: u32) -> Result<Preview, AttachmentStoreError> {
    let thumbnail = image.thumbnail(max_edge, max_edge);
    let dimensions = ImageDimensions {
        width: thumbnail.width(),
        height: thumbnail.height(),
    };
    let mut bytes = Vec::new();
    thumbnail
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|_| io_failure())?;
    Ok(Preview {
        dimensions,
        bytes: bytes.into(),
    })
}

fn decode_png(bytes: &[u8], max_alloc: u64) -> Result<DynamicImage, AttachmentStoreError> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);
    reader.decode().map_err(|_| invalid_image())
}

fn public_preview(attachment_id: AttachmentId, preview: Preview) -> AttachmentPreview {
    AttachmentPreview {
        attachment_id,
        dimensions: preview.dimensions,
        bytes: preview.bytes.as_ref().to_vec(),
    }
}

fn write_atomic(path: &AtomicPath, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, || Ok(()))
}

fn write_atomic_with(
    path: &AtomicPath,
    bytes: &[u8],
    before_publish: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.staging())?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        before_publish()?;
        std::fs::rename(path.staging(), path.destination())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path.staging());
    }
    result
}

fn require_known_session(state: &State, session: SessionId) -> Result<(), AttachmentStoreError> {
    if state.active_sessions.contains(&session) {
        Ok(())
    } else if state.ended_sessions.contains(&session) {
        Err(AttachmentStoreError::new(
            ErrorCode::SessionExited,
            "session has ended",
        ))
    } else {
        Err(AttachmentStoreError::new(
            ErrorCode::SessionNotFound,
            "session is not registered",
        ))
    }
}

// Range sentinels are valid v4 UUIDs and never escape this crate.
fn minimum_attachment_id() -> AttachmentId {
    "00000000-0000-4000-8000-000000000000"
        .parse()
        .expect("valid minimum v4 UUID")
}

fn maximum_attachment_id() -> AttachmentId {
    "ffffffff-ffff-4fff-bfff-ffffffffffff"
        .parse()
        .expect("valid maximum v4 UUID")
}

fn not_found() -> AttachmentStoreError {
    AttachmentStoreError::new(ErrorCode::AttachmentNotFound, "attachment is not pending")
}

fn invalid_image() -> AttachmentStoreError {
    AttachmentStoreError::new(ErrorCode::InvalidImage, "attachment image is invalid")
}

fn limit_error() -> AttachmentStoreError {
    AttachmentStoreError::new(
        ErrorCode::AttachmentLimit,
        "attachment dimensions exceed the configured limit",
    )
}

fn io_failure() -> AttachmentStoreError {
    AttachmentStoreError::new(ErrorCode::Internal, "attachment storage operation failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_atomic_write_never_publishes_a_partial_file() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("agent/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let paths = SessionPaths::from_resolved_directory(sessions);
        let session = SessionId::mint();
        paths.prepare_attachment_directory(session).unwrap();
        let atomic = paths.attachment_atomic_path(
            session,
            AttachmentId::mint(),
            VerifiedAttachmentExtension::Png,
        );
        let error = write_atomic_with(&atomic, b"complete bytes", || {
            Err(io::Error::other("injected interruption"))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(!atomic.destination().exists());
        assert!(!atomic.staging().exists());
    }
}
