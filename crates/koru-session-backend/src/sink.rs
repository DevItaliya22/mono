// SPDX-License-Identifier: MPL-2.0

//! The backend's event surface, and the sink a host implements to receive it.
//!
//! **Surface-neutral by contract.** Every variant here is something the
//! session backend itself produces — connection state, catalogs, render
//! frames, managed streams, attachment results. Anything a particular host
//! adds around that (window routing, a Gateway catalog, settings wake-ups)
//! belongs to that host's own event enum, into which it maps these.

use koru_session_profiles::ProfileRow;
use koru_session_protocol::{
    ApprovalId, Attachment, AttachmentId, AttachmentSource, ErrorCode, ProjectSummary,
    RenderUpdate, ScrollbackPage, SendPendingInputResult, SequencedAgentEvent, SessionFailure,
    SessionId, SessionMode, SessionSummary, StreamSequence,
};
use koru_session_view::PreviewImage;

/// What the session backend tells its host.
///
/// Emitted only through [`BackendSink::deliver`], from the backend's command
/// thread and its subscription readers — never from the host's own thread.
#[derive(Clone, Debug)]
pub enum BackendEvent {
    ProviderModels {
        request: koru_session_protocol::IdempotencyKey,
        result: Result<koru_session_protocol::ProviderModels, ErrorCode>,
    },
    Profiles(Vec<ProfileRow>),
    Providers(Vec<koru_session_protocol::ProviderReadiness>),
    Connected(Vec<SessionSummary>),
    Sessions(Vec<SessionSummary>),
    /// The device's projects, for the host's recents. **Names only reach the
    /// screen**; SP-11 owns the tree, the placement and every mutation verb.
    Projects(Vec<ProjectSummary>),
    Attached {
        session_id: SessionId,
        mode: SessionMode,
        epoch: u64,
    },
    /// **A watch was established**: this backend is reading a managed
    /// session's event stream and holds no controller lease on it. Every
    /// `ManagedBatch`/`ManagedGap` that follows carries this `epoch`, the
    /// same late-frame guard an attachment's does.
    ///
    /// Deliberately not an `Attached` with a flag: a host that treated the
    /// two alike would eventually send input down the one that cannot carry
    /// it, and the difference between watching a session and holding it is
    /// the whole point of the variant.
    Observing {
        session_id: SessionId,
        epoch: u64,
    },
    RawFallback {
        session_id: SessionId,
    },
    Terminated {
        session_id: SessionId,
    },
    Render(RenderUpdate),
    ManagedBatch {
        session_id: SessionId,
        epoch: u64,
        events: Vec<SequencedAgentEvent>,
        earlier_truncated: bool,
    },
    ManagedGap {
        session_id: SessionId,
        epoch: u64,
        expected: StreamSequence,
        actual: StreamSequence,
    },
    ManagedCommandFailure {
        session_id: SessionId,
        epoch: u64,
        approval_id: Option<ApprovalId>,
        code: ErrorCode,
    },
    ManagedStreamFailure {
        session_id: SessionId,
        epoch: u64,
        failure: SessionFailure,
    },
    Scrollback(ScrollbackPage),
    Attachments {
        session_id: SessionId,
        attachments: Vec<Attachment>,
    },
    AttachmentIngesting {
        session_id: SessionId,
        attachment_id: AttachmentId,
        source: AttachmentSource,
    },
    AttachmentIngested {
        session_id: SessionId,
        attachment: Attachment,
    },
    AttachmentPreview {
        session_id: SessionId,
        attachment_id: AttachmentId,
        preview: PreviewImage,
    },
    AttachmentRemoved {
        session_id: SessionId,
        attachment_id: AttachmentId,
    },
    PendingInputSent {
        session_id: SessionId,
        result: SendPendingInputResult,
    },
    PendingInputFailure {
        session_id: SessionId,
        attachment_id: Option<AttachmentId>,
        code: ErrorCode,
    },
    PendingDraftPaste {
        session_id: SessionId,
        text: String,
    },
    TerminalPaste {
        session_id: SessionId,
        text: String,
    },
    OperationFailure {
        operation: &'static str,
        session_id: Option<SessionId>,
        failure: SessionFailure,
    },
    Failure(SessionFailure),
    /// The session-host connection was lost and the backend is re-establishing
    /// it on its own thread. Reported once per outage — news, not activity —
    /// and followed by exactly one of [`Recovered`](Self::Recovered) or
    /// [`SessionsLost`](Self::SessionsLost) once a host answers the handshake.
    ///
    /// Before W3b this variant was terminal ("gone, restart the window");
    /// recovery deliberately subsumes it as the *connecting* state, so a host
    /// that maps it to its existing unavailable presentation is still telling
    /// the truth.
    /// Protocol v14, §7.4: the controller handover moved. The window reads
    /// the same value off every refreshed `SessionSummary`; this arrives
    /// the moment the host says so, which is what makes *take the session*
    /// feel like a press rather than a poll.
    Handover {
        session_id: SessionId,
        handover: koru_session_protocol::HandoverState,
    },
    Disconnected,
    /// The connection came back to the **same host instance** — decided from
    /// `HostHello.instance_id` / `supervisor_launch_id`, never inferred from
    /// the catalog — so nothing was lost: `sessions` is the fresh catalog, and
    /// if a session was attached a new [`Attached`](Self::Attached) (fresh
    /// lease, fresh epoch) follows. A quiet resume.
    Recovered {
        sessions: Vec<SessionSummary>,
    },
    /// The connection came back to a **different (restarted) host instance**:
    /// every session the previous connection knew died with the old host.
    /// `sessions` is the new instance's catalog — empty in practice, and an
    /// empty catalog on a new instance is loss, not "nothing changed", which
    /// is why identity comes from the handshake and not from the list. The
    /// host presents session loss; nothing is re-launched or replayed on the
    /// person's behalf.
    SessionsLost {
        sessions: Vec<SessionSummary>,
    },
    Backpressure,
}

/// Where the backend's events go.
///
/// A host hands one of these to [`spawn`](crate::spawn); the backend clones it
/// into each of its reader threads. Delivery must be cheap and non-blocking —
/// a windowed host (koru.exe) wraps a `winit` event-loop proxy; a headless host
/// can wrap a channel sender.
pub trait BackendSink: Clone + Send + 'static {
    /// Deliver one event to the host. Returns `false` once the host is gone
    /// — the receiving loop has shut down — which is a reader thread's signal
    /// to stop rather than an error to report.
    fn deliver(&self, event: BackendEvent) -> bool;
}
