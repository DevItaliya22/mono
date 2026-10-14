// SPDX-License-Identifier: MPL-2.0

//! What a hosted surface needs from its host's path to the session backend.
//!
//! The three views inside a [`SessionSurface`](crate::SessionSurface) already
//! speak `koru_session_view`'s frozen boundary — typed intents through
//! [`SessionSurfaceInteraction::dispatch`], with session ids, lease tokens and
//! idempotency keys added by the host, never by the view. This trait is the
//! small remainder: the two things the *surface itself* sends, which the
//! intent funnel does not carry.
//!
//! The host implements it on its own handle to the backend's command queue
//! (koru.exe wraps the koru-session-backend crate's `InteractionSender`);
//! this crate deliberately does not depend on that backend, so the same
//! surface can be bound to a different command path by a different host.

use koru_session_protocol::GridSize;
use koru_session_view::{ClipboardAction, SessionSurfaceInteraction};

pub trait SurfaceInteraction: SessionSurfaceInteraction + Clone {
    /// Resize the raw grid the connection currently controls. Sent when a
    /// pending raw switch retargets — the one command a switch owns itself.
    fn resize(&self, size: GridSize);

    /// Acquire the clipboard **off the loop** — reading it can block on
    /// another process, and a terminal that stalled on a paste would be a
    /// terminal that stalled on somebody else's window.
    fn clipboard(&self, action: ClipboardAction);
}

/// The null interaction, for tests — the same affordance
/// `koru-session-view`'s `impl SessionSurfaceInteraction for ()` gives its
/// views: a surface built over `()` drops every command, so its presentation
/// state can be driven without a backend behind it.
impl SurfaceInteraction for () {
    fn resize(&self, _size: GridSize) {}
    fn clipboard(&self, _action: ClipboardAction) {}
}
