// SPDX-License-Identifier: MPL-2.0

//! Reusable fixed-grid presentation for immutable terminal render snapshots.
//!
//! This crate deliberately has no process, root, window, IPC, Agent, or
//! emulator dependency. Hosts supply a shared text engine and translate the
//! typed interaction intents at their own identity/lease boundary.

mod cursor;
mod geometry;
mod history;
mod input;
mod interaction;
mod managed;
mod model;
mod pending_input;
mod render;
mod row;
mod selection;
mod style;
mod surface;

pub use cursor::{
    TerminalCursorBlink, TerminalCursorHeight, TerminalCursorPreferences, TerminalCursorWidth,
};
pub use geometry::{CONTENT_INSET_X, CellPoint, CellSelection, GridGeometry};
pub use history::{PAGE_CACHE_BYTE_BUDGET, PAGE_CACHE_ROW_BUDGET};
pub use input::{
    ClipboardAction, InputOutcome, KeyPhase, MAX_PASTE_BYTES, PointerModifiers, TerminalKeyEvent,
    TerminalKeyInput, TerminalShortcut,
};
pub use interaction::TerminalIntent;
pub use managed::{
    ApprovalFocus, MANAGED_ITEM_LIMIT, MANAGED_MESSAGE_BLOCK_BYTE_LIMIT, MANAGED_TEXT_BYTE_LIMIT,
    ManagedApplyOutcome, ManagedApprovalLayout, ManagedApprovalState, ManagedAttachmentState,
    ManagedDrawOutcome, ManagedHit, ManagedInputOutcome, ManagedIntent, ManagedItemContent,
    ManagedItemKind, ManagedItemLayout, ManagedKey, ManagedLayout, ManagedPresentationToken,
    ManagedRenderWork, ManagedSessionCopy, ManagedSessionView, ManagedToolState,
    ManagedTranscriptItem, ManagedTurnState,
};
pub use model::{ApplyOutcome, TerminalModel};
pub use pending_input::{
    AttachmentLayout, COMPOSER_HEIGHT, MAX_DRAFT_CHARS, PendingAttachment, PendingAttachmentStatus,
    PendingInputCopy, PendingInputHit, PendingInputIntent, PendingInputLayout, PendingInputModel,
    PendingInputView, PreviewImage,
};
pub use render::{PaintRecord, RenderWork, TerminalView};
pub use style::{
    ResolvedCellStyle, StyleEpoch, TerminalAppearance, TerminalFace, TerminalStyle,
    terminal_background,
};
pub use surface::{SessionSurfaceIntent, SessionSurfaceInteraction};
