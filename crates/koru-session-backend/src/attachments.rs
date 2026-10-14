// SPDX-License-Identifier: MPL-2.0

use std::fs::File;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};

use image::ImageReader;
use koru_clipboard::{ErrorCode as ClipboardErrorCode, ImageLimits, ImagePayload, TextPayload};
use koru_session_client::SessionClient;
use koru_session_protocol::{
    AttachmentId, AttachmentSource, ControllerLeaseToken, ErrorCode, PendingInput, SessionId,
};
use koru_session_view::{MAX_DRAFT_CHARS, PreviewImage};
use vello::peniko::{Blob, ImageAlphaType, ImageBrush, ImageData, ImageFormat};

use crate::{BackendEvent, BackendSink};

const JOB_CAPACITY: usize = 32;
const PREVIEW_EDGE: u32 = 96;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FILES_PER_JOB: usize = 16;

#[derive(Clone, Copy)]
pub(crate) struct Target {
    pub session_id: SessionId,
    pub lease: ControllerLeaseToken,
}

enum Job {
    Load(Target),
    ClipboardImage(Target),
    PendingPaste(Target),
    Files(Target, Vec<PathBuf>),
    Remove(Target, AttachmentId),
    Send(Target, PendingInput),
    TerminalPaste(Target),
    CopyText(String),
}

/// One bounded worker owns all clipboard, file, image-decode, and attachment
/// IPC work. The window thread and the terminal-command router only enqueue.
pub(crate) struct Worker {
    send: SyncSender<Job>,
    overflowed: Arc<AtomicBool>,
}

impl Worker {
    pub fn spawn<S: BackendSink>(client: Arc<SessionClient>, sink: S) -> Self {
        let (send, receive) = mpsc::sync_channel(JOB_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let _ = std::thread::Builder::new()
            .name("koru-session-backend attachment worker".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                while let Ok(job) = receive.recv() {
                    run_job(&client, &sink, job);
                }
            });
        Self { send, overflowed }
    }

    fn enqueue(&self, job: Job) -> bool {
        match self.send.try_send(job) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.overflowed.store(true, Ordering::Release);
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn load(&self, target: Target) -> bool {
        self.enqueue(Job::Load(target))
    }

    pub fn clipboard_image(&self, target: Target) -> bool {
        self.enqueue(Job::ClipboardImage(target))
    }

    pub fn pending_paste(&self, target: Target) -> bool {
        self.enqueue(Job::PendingPaste(target))
    }

    pub fn files(&self, target: Target, paths: Vec<PathBuf>) -> bool {
        self.enqueue(Job::Files(target, paths))
    }

    pub fn remove(&self, target: Target, id: AttachmentId) -> bool {
        self.enqueue(Job::Remove(target, id))
    }

    pub fn send(&self, target: Target, input: PendingInput) -> bool {
        self.enqueue(Job::Send(target, input))
    }

    pub fn terminal_paste(&self, target: Target) -> bool {
        self.enqueue(Job::TerminalPaste(target))
    }

    pub fn copy_text(&self, text: String) -> bool {
        self.enqueue(Job::CopyText(text))
    }

    pub fn take_overflow(&self) -> bool {
        self.overflowed.swap(false, Ordering::AcqRel)
    }
}

fn run_job<S: BackendSink>(client: &SessionClient, sink: &S, job: Job) {
    match job {
        Job::Load(target) => load(client, sink, target),
        Job::ClipboardImage(target) => clipboard_image(client, sink, target),
        Job::PendingPaste(target) => match koru_clipboard::system().text() {
            TextPayload::Text(text) => send(
                sink,
                BackendEvent::PendingDraftPaste {
                    session_id: target.session_id,
                    text: text.chars().take(MAX_DRAFT_CHARS + 1).collect(),
                },
            ),
            TextPayload::NonText => clipboard_image(client, sink, target),
            TextPayload::Unavailable => {
                pending_failure(sink, target.session_id, None, ErrorCode::Internal)
            }
        },
        Job::Files(target, paths) => {
            let overflow = paths.len() > MAX_FILES_PER_JOB;
            for path in paths.into_iter().take(MAX_FILES_PER_JOB) {
                ingest_file(client, sink, target, path, AttachmentSource::Drop);
            }
            if overflow {
                pending_failure(sink, target.session_id, None, ErrorCode::AttachmentLimit);
            }
        }
        Job::Remove(target, id) => {
            match client.remove_attachment(target.session_id, target.lease, id) {
                Ok(_) => send(
                    sink,
                    BackendEvent::AttachmentRemoved {
                        session_id: target.session_id,
                        attachment_id: id,
                    },
                ),
                Err(error) => pending_failure(
                    sink,
                    target.session_id,
                    Some(id),
                    crate::client_failure(error).code,
                ),
            }
        }
        Job::Send(target, input) => {
            match client.send_pending_input(target.session_id, target.lease, input) {
                Ok(result) => send(
                    sink,
                    BackendEvent::PendingInputSent {
                        session_id: target.session_id,
                        result,
                    },
                ),
                Err(error) => pending_failure(
                    sink,
                    target.session_id,
                    None,
                    crate::client_failure(error).code,
                ),
            }
        }
        Job::TerminalPaste(target) => match koru_clipboard::system().text() {
            TextPayload::Text(text) => send(
                sink,
                BackendEvent::TerminalPaste {
                    session_id: target.session_id,
                    text,
                },
            ),
            TextPayload::NonText => clipboard_image(client, sink, target),
            TextPayload::Unavailable => {
                pending_failure(sink, target.session_id, None, ErrorCode::Internal)
            }
        },
        Job::CopyText(text) => {
            let _ = koru_clipboard::system().set_text(&text);
        }
    }
}

fn load<S: BackendSink>(client: &SessionClient, sink: &S, target: Target) {
    match client.attachments(target.session_id) {
        Ok(attachments) => {
            send(
                sink,
                BackendEvent::Attachments {
                    session_id: target.session_id,
                    attachments: attachments.clone(),
                },
            );
            for attachment in attachments {
                if attachment.preview_available {
                    preview(client, sink, target.session_id, attachment.id);
                }
            }
        }
        Err(error) => pending_failure(
            sink,
            target.session_id,
            None,
            crate::client_failure(error).code,
        ),
    }
}

fn clipboard_image<S: BackendSink>(client: &SessionClient, sink: &S, target: Target) {
    match koru_clipboard::system().images(clipboard_limits()) {
        Ok(ImagePayload::Bitmap(bitmap)) => ingest_bytes(
            client,
            sink,
            target,
            AttachmentId::mint(),
            AttachmentSource::Clipboard,
            bitmap.bytes,
        ),
        Ok(ImagePayload::Files(paths)) => {
            let overflow = paths.len() > MAX_FILES_PER_JOB;
            for path in paths.into_iter().take(MAX_FILES_PER_JOB) {
                ingest_file(client, sink, target, path, AttachmentSource::Clipboard);
            }
            if overflow {
                pending_failure(sink, target.session_id, None, ErrorCode::AttachmentLimit);
            }
        }
        Ok(ImagePayload::Empty) => {
            pending_failure(sink, target.session_id, None, ErrorCode::InvalidImage)
        }
        Err(error) => pending_failure(
            sink,
            target.session_id,
            None,
            map_clipboard_error(error.code),
        ),
    }
}

fn ingest_file<S: BackendSink>(
    client: &SessionClient,
    sink: &S,
    target: Target,
    path: PathBuf,
    source: AttachmentSource,
) {
    let id = AttachmentId::mint();
    send(
        sink,
        BackendEvent::AttachmentIngesting {
            session_id: target.session_id,
            attachment_id: id,
            source,
        },
    );
    match read_bounded(path) {
        Ok(bytes) => ingest_bytes_started(client, sink, target, id, source, bytes),
        Err(code) => pending_failure(sink, target.session_id, Some(id), code),
    }
}

fn ingest_bytes<S: BackendSink>(
    client: &SessionClient,
    sink: &S,
    target: Target,
    id: AttachmentId,
    source: AttachmentSource,
    bytes: Vec<u8>,
) {
    send(
        sink,
        BackendEvent::AttachmentIngesting {
            session_id: target.session_id,
            attachment_id: id,
            source,
        },
    );
    ingest_bytes_started(client, sink, target, id, source, bytes);
}

fn ingest_bytes_started<S: BackendSink>(
    client: &SessionClient,
    sink: &S,
    target: Target,
    id: AttachmentId,
    source: AttachmentSource,
    bytes: Vec<u8>,
) {
    match client.ingest_attachment(target.session_id, target.lease, id, source, &bytes) {
        Ok(attachment) => {
            send(
                sink,
                BackendEvent::AttachmentIngested {
                    session_id: target.session_id,
                    attachment,
                },
            );
            preview(client, sink, target.session_id, id);
        }
        Err(error) => pending_failure(
            sink,
            target.session_id,
            Some(id),
            crate::client_failure(error).code,
        ),
    }
}

fn preview<S: BackendSink>(
    client: &SessionClient,
    sink: &S,
    session_id: SessionId,
    id: AttachmentId,
) {
    let result = client
        .attachment_preview(session_id, id, PREVIEW_EDGE)
        .map_err(crate::client_failure)
        .and_then(|preview| decode_preview(&preview.bytes));
    match result {
        Ok(preview) => send(
            sink,
            BackendEvent::AttachmentPreview {
                session_id,
                attachment_id: id,
                preview,
            },
        ),
        Err(error) => tracing::warn!(
            target: "koru_session_backend",
            code = %crate::closed_error_code(&error.code),
            session_id = %session_id,
            attachment_id = %id,
            "attachment preview unavailable"
        ),
    }
}

fn decode_preview(bytes: &[u8]) -> Result<PreviewImage, koru_session_protocol::SessionFailure> {
    let reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| failure(ErrorCode::InvalidImage))?;
    let image = reader
        .decode()
        .map_err(|_| failure(ErrorCode::InvalidImage))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let data = ImageData {
        data: Blob::new(Arc::new(image.into_raw())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    };
    Ok(PreviewImage {
        brush: ImageBrush::new(data),
        width,
        height,
    })
}

fn read_bounded(path: PathBuf) -> Result<Vec<u8>, ErrorCode> {
    let metadata = std::fs::metadata(&path).map_err(|_| ErrorCode::InvalidImage)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES {
        return Err(ErrorCode::AttachmentLimit);
    }
    let mut file = File::open(path).map_err(|_| ErrorCode::InvalidImage)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::InvalidImage)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(ErrorCode::AttachmentLimit);
    }
    Ok(bytes)
}

fn clipboard_limits() -> ImageLimits {
    ImageLimits {
        max_width: 8_192,
        max_height: 8_192,
        max_pixels: 32_000_000,
        max_rgba_bytes: 128 * 1024 * 1024,
        max_png_bytes: MAX_FILE_BYTES as usize,
    }
}

fn map_clipboard_error(code: ClipboardErrorCode) -> ErrorCode {
    match code {
        ClipboardErrorCode::OversizedImage => ErrorCode::AttachmentLimit,
        ClipboardErrorCode::CorruptImage | ClipboardErrorCode::EncodeFailed => {
            ErrorCode::InvalidImage
        }
        ClipboardErrorCode::Unavailable => ErrorCode::Internal,
    }
}

fn pending_failure<S: BackendSink>(
    sink: &S,
    session_id: SessionId,
    attachment_id: Option<AttachmentId>,
    code: ErrorCode,
) {
    send(
        sink,
        BackendEvent::PendingInputFailure {
            session_id,
            attachment_id,
            code,
        },
    );
}

fn failure(code: ErrorCode) -> koru_session_protocol::SessionFailure {
    koru_session_protocol::SessionFailure {
        message: code.as_str().into(),
        code,
    }
}

fn send<S: BackendSink>(sink: &S, event: BackendEvent) {
    let _ = sink.deliver(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_files_are_refused_before_allocation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("large.png");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert_eq!(read_bounded(path), Err(ErrorCode::AttachmentLimit));
    }

    #[test]
    fn clipboard_bounds_match_the_attachment_host_contract() {
        let limits = clipboard_limits();
        assert_eq!(limits.max_png_bytes as u64, MAX_FILE_BYTES);
        assert_eq!(limits.max_rgba_bytes as u64, 128 * 1024 * 1024);
    }
}
