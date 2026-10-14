// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use koru_session_protocol::{AttachmentId, PendingInput};

/// Pending-input payload carried by the shared session-surface boundary.
///
/// The view never opens the clipboard, reads a file, decodes an image, or
/// speaks IPC. A host performs those jobs away from its event thread and sends
/// the resulting typed state back to the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingInputIntent {
    AcquireClipboardImage,
    PasteClipboard,
    AddFiles { paths: Vec<PathBuf> },
    RemoveAttachment { attachment_id: AttachmentId },
    Send { input: PendingInput },
    Retry { input: PendingInput },
}
