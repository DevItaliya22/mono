// SPDX-License-Identifier: MPL-2.0

//! **What a press means, and what only the host can do about it.**
//!
//! Two vocabularies, the way `koru-workspaces-ui` has them and for the same
//! reason. [`Action`] is *which control was pressed* — private, produced by
//! the layout pass and consumed by [`crate::RunsSurface::press`].
//! [`Request`] is *what only the host can do* — public, drained by the host,
//! and deliberately small.
//!
//! The line falls in an unusual place here, and it is worth saying why.
//! Reading a run, ending it, resuming it and rating it are all calls this
//! surface makes for itself through `client.runs()`, off its own thread.
//! **The handover is not**, and cannot be: the lease belongs to the
//! connection that holds it, and in this window that connection is the
//! session backend's — one process-wide client, one lease, one session tab.
//! A surface that opened its own connection to take control would take a
//! *second* lease and the host would have two, which is exactly the
//! conflict §7.4 exists to avoid. So the three handover verbs are requests,
//! and the host performs them on the connection that already owns the
//! session.

use std::collections::BTreeSet;

use koru_delegate_core::{
    ArtifactVersionId, Objective, Rating, RunContextItem, RunId, SessionId, SessionProfileKey,
    WorkspaceId, WorkspaceScope,
};
use koru_session_protocol::TakeoverUrgency;

use crate::state::Tab;

/// One control, named by what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Show one of the three views.
    Show(Tab),

    // ------------------------------------------------- the session (§7.4)
    /// Ask for the session at the next step boundary. **Recoverable**: the
    /// agent finishes what it is doing and the run carries on.
    TakeControl,
    /// Ask for the session now, ending the step the agent is on.
    /// **Recoverable too** — D36's whole point: this ends the step, not the
    /// job, and no confirmation stands between a person and it.
    TakeControlNow,
    /// Give the session back; the orchestrator resumes.
    GiveBackControl,
    /// Open the session tab on the session this person now holds.
    OpenSession,

    // ------------------------------------------ the run itself (D36's other half)
    /// **Irreversible.** Opens the confirmation; nothing happens until it
    /// is answered.
    AskToEndRun,
    /// Continue an interrupted run as a fresh attempt.
    Resume,
    /// **Re-run** on a run that has ended (ADR-366).
    /// Not [`Resume`](Self::Resume) and never beside it: that one continues
    /// *this* run and exists only while one is interrupted; this one makes a
    /// new run and leaves this one exactly as it is.
    ReRun,

    // ------------------------------------------------------ the rating (D44)
    Rate(Rating),
    /// Save the note typed under a rating that already exists.
    SaveNote,

    // ------------------------------------------------------------ the activity
    /// Widen the window of history by one page, oldest-ward.
    ShowEarlier,

    /// **Show the run's working folder in the OS file manager.** Offered
    /// only where a folder was named *and* is still there — a link to
    /// nothing is worse than no link.
    RevealFolder,

    // ----------------------------------------------- the document a run produced
    /// **Open one of the run's results as a tab** (ADR-368). Not here: the
    /// reason to come back to a run is to read what it produced, and a
    /// document in Koru opens as a tab.
    OpenResult(usize),
    /// Lay out one more page of whichever document is showing.
    ShowMoreMarkdown,

    // ------------------------------------------- the instructions, and saving them
    /// Open the instructions in the composer.
    EditPreamble,
    /// Close the composer and keep the file as it was.
    StopEditingPreamble,
    /// Write the edit back to the file the instructions were resolved from.
    SavePreamble,
}

/// **What the surface cannot do for itself.**
///
/// Every one of these needs the window's own session connection — the one
/// that holds the controller lease and draws the session tab. A surface
/// hosted beside that connection may ask; it may not reach past it.
#[derive(Clone, PartialEq, Eq)]
pub enum Request {
    /// §7.4's `request_control`. `urgency` is the whole of D36's
    /// recoverable half: `AtTurnBoundary` waits for the agent to finish
    /// the step, `Now` ends the step with `CancelReason::TakeoverBoundary`
    /// so the boundary arrives sooner. **Neither ends the run.**
    TakeControl {
        session: SessionId,
        urgency: TakeoverUrgency,
    },
    /// §7.4's `return_control`. The orchestrator reacquires and carries on.
    GiveBackControl { session: SessionId },
    /// Show the session the person now holds. The host owns what a tab is.
    OpenSession { session: SessionId },
    /// **Open the delegation surface, filled in from this run** (ADR-366).
    ///
    /// A request rather than something this surface does, for the same reason
    /// the three above are: the delegation modal is the window's, it names
    /// documents, projects and a workspace, and this crate is chartered to
    /// name none of them. What travels is what the run was *asked for* --
    /// everything a person may then change before pressing Delegate.
    ///
    /// **Nothing about the old run is touched by this.** It is a request to
    /// open a surface, and the run it came from is only along so the window
    /// can say which one it is filling in from.
    ReRun(Box<ReRun>),
    /// **Open one of this run's results as a tab** (ADR-368).
    ///
    /// A request rather than something this surface does, for the reason
    /// every other one here is: a tab belongs to the window, and which tab
    /// strip a document lands in is not a run view's to decide. What
    /// travels is what the *run* already knows about the result — never its
    /// bytes, which the tab reads for itself through the same route this
    /// surface would have.
    OpenResult(Box<OpenedResult>),
    /// **Show the run's working folder in the OS file manager.**
    ///
    /// A request for the same reason the rest are: spawning a file manager
    /// is a process this surface does not start, the platform seam for it
    /// already exists once in the shell (`koru_workspace_host::reveal`), and
    /// a second one would be a second thing to keep true on macOS. The
    /// window forwards it to the shell, which does the work off the window
    /// thread.
    ///
    /// The path travels because it is the only thing a reveal needs. It is a
    /// person's own (§13.7): it is drawn, it is handed to the file manager,
    /// and it reaches no log, no event attribute and no `Debug` — which is
    /// why this variant writes its own.
    RevealFolder(std::path::PathBuf),
}

/// The one variant carrying a path prints that it is a reveal and nothing
/// else. Every other variant carries ids, which are what a reader of a log
/// line needs.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TakeControl { session, urgency } => f
                .debug_struct("TakeControl")
                .field("session", session)
                .field("urgency", urgency)
                .finish(),
            Self::GiveBackControl { session } => f
                .debug_struct("GiveBackControl")
                .field("session", session)
                .finish(),
            Self::OpenSession { session } => f
                .debug_struct("OpenSession")
                .field("session", session)
                .finish(),
            Self::ReRun(re_run) => f.debug_tuple("ReRun").field(re_run).finish(),
            Self::OpenResult(opened) => f.debug_tuple("OpenResult").field(opened).finish(),
            Self::RevealFolder(_) => f.write_str("RevealFolder"),
        }
    }
}

/// One of a run's results, handed to whoever can open a tab on it.
///
/// Everything here is the run's header as it stands, plus the one thing a
/// header cannot answer: what the device holding the bytes is *called*.
/// A device id is not a name (PRINCIPLES, *a stable identifier never
/// derives from a name*), so the run views resolve it at display time and
/// send the name along.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenedResult {
    /// The run that produced it. Along so the window can say where the
    /// document came from; nothing about the run is touched by opening it.
    pub run: RunId,
    pub version: ArtifactVersionId,
    /// What the result is called — the tab's label, and the heading over
    /// the document.
    pub title: String,
    pub size_bytes: u64,
    /// `None` when this device holds the bytes. `Some(name)` names the
    /// device that does (D27), so the tab can say so without asking a
    /// gateway a question it would have to refuse.
    pub held_elsewhere: Option<String>,
}

impl std::fmt::Debug for OpenedResult {
    /// **A result's title is content** (§13.7): it sits where a document's
    /// name sits and it is somebody's writing. The ids, the size and
    /// *whether* the bytes are elsewhere are what a reader of one needs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedResult")
            .field("run", &self.run)
            .field("version", &self.version)
            .field("size_bytes", &self.size_bytes)
            .field("elsewhere", &self.held_elsewhere.is_some())
            .finish()
    }
}

/// What a finished run was asked for, handed to whoever can ask for it again.
///
/// Every field is the earlier run's header, unchanged -- objective, agent,
/// working folder, what it was given, what it could reach. The surface that
/// opens from it makes all of them editable, which is the difference between
/// re-running and repeating.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReRun {
    /// The run this was read from. Untouched by any of it.
    pub run: RunId,
    pub workspace: WorkspaceId,
    pub objective: Objective,
    pub agent: SessionProfileKey,
    pub requested_model: Option<koru_delegate_core::ModelName>,
    pub scope: WorkspaceScope,
    /// The earlier run's pins, **minus any whose source this device can see
    /// is in the trash or gone** (D48). Filtered here because this is where
    /// the state of each pin is known; carrying one that admission would
    /// refuse would put a person in front of a run that could not start and
    /// tell them so afterwards.
    pub context: Vec<RunContextItem>,
    /// Whether anything was left out for that reason, so the surface can say
    /// so. Never `true` when this device could not resolve the sources at
    /// all -- not knowing is not the same as gone (D46).
    pub dropped: bool,
    pub grants: BTreeSet<WorkspaceId>,
}

/// What the surface asked its worker to do for it, so an answer can name
/// the question. Closed tokens, never sentences: the worker thread does not
/// know which language a window is drawing in and must not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Read,
    EndRun,
    Resume,
    Rate,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::EndRun => "end_run",
            Self::Resume => "resume",
            Self::Rate => "rate",
        }
    }
}
