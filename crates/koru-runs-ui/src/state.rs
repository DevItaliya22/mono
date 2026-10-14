// SPDX-License-Identifier: MPL-2.0

//! The model the three views draw from.
//!
//! **Three availabilities, not two.** A surface that has never heard from
//! the worker is not the same as one whose worker has gone away, and
//! neither is the same as one holding a run. Collapsing them is how a
//! window ends up saying *this run registered nothing* about a run it has
//! not managed to read yet — which is D29's sentence said about the wrong
//! thing, and worse than saying nothing.
//!
//! **What this device may do is a value, not a rule to remember.** The
//! worker answers with the device it is, and the header names the device
//! executing the run; [`Standing`] is the one comparison, made once. Every
//! control asks it, so there is no path by which a second device lays out
//! an action it may not perform (D15).

use koru_delegate_api::wire::{RunView, TimelineItem};
use koru_delegate_core::{
    Attention, ContextState, ControlState, DeviceId, RunHeader, RunId, RunStatus, SessionId,
    WorkspaceId,
};
use koru_delegate_preamble::{Notice as PreambleNotice, Source as PreambleSource, WriteFault};
use koru_markdown_view::Content;
use koru_session_protocol::HandoverState;

/// Which of the four views is showing.
///
/// Three of them are about *this run*; [`SystemPrompt`] is not — it is the
/// instructions every delegated run starts from, and it draws whether or
/// not the run has been read yet.
///
/// [`SystemPrompt`]: Tab::SystemPrompt
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Overview,
    Activity,
    Result,
    /// The delegated-agent instructions, read-only.
    SystemPrompt,
}

impl Tab {
    pub const ALL: [Self; 4] = [
        Self::Overview,
        Self::Activity,
        Self::Result,
        Self::SystemPrompt,
    ];

    /// The closed token this tab is named by — what a test asks for, and
    /// what a host would persist if it ever remembered one.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Activity => "activity",
            Self::Result => "result",
            Self::SystemPrompt => "system_prompt",
        }
    }
}

/// **The instructions a delegated agent is started with**, as this surface
/// holds them.
///
/// Deliberately *not* a record of what this run was handed. A run's
/// document carries ids, counts and step kinds (D45) and no prose; putting
/// the preamble in it would copy a file's contents into every run, which is
/// the thing D45 refuses for the timeline. So the tab reads the file **as
/// it stands now**, through the same `koru_delegate_preamble::resolve` the
/// delegate worker calls at the start of every run, and says so in a line
/// above the text (ADR-360).
#[derive(Clone, PartialEq, Eq)]
pub struct PreambleView {
    /// The instructions as a document, ready to draw. Built on the
    /// surface's own thread beside the read, because a parse is
    /// proportional to the file and a frame must not be.
    pub content: Content,
    /// Whether that text is the file on this machine or Koru's built-in
    /// copy.
    pub source: PreambleSource,
    /// Set when a file existed and could not be used, so the tab can say
    /// that the edit did not take and why.
    pub notice: Option<PreambleNotice>,
    /// **Whether there is a file to write back to.** False exactly when the
    /// built-in copy is in force, and the tab then offers no save at all
    /// rather than a control that would have nowhere to put the text.
    pub savable: bool,
}

impl std::fmt::Debug for PreambleView {
    /// The instructions are provider payload and never printed — the same
    /// rule `koru_delegate_preamble::Resolved` keeps (§13.7).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreambleView")
            .field("source", &self.source)
            .field("notice", &self.notice)
            .field("savable", &self.savable)
            .finish()
    }
}

/// Why an edit to the instructions was not saved.
///
/// Closed and content-free, like every other refusal this surface renders:
/// the worker does not know which language a window draws in, and a path is
/// never in a message (§13.7). **Every one of them means the file on disk
/// is exactly what it was.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFailure {
    /// The built-in copy is in force. There is nowhere to write.
    NoFile,
    /// The composer holds something Koru could not read as Markdown, so
    /// there is no document to export.
    NotADocument,
    /// The write itself refused, and said why.
    Write(WriteFault),
}

impl SaveFailure {
    /// The sentence, in the person's language.
    pub fn message(self, lang: koru_i18n::Lang) -> String {
        use koru_i18n::{Key, t};
        match self {
            Self::NoFile => t(lang, Key::RunSystemPromptNoFileToSave).to_owned(),
            Self::NotADocument => t(lang, Key::MarkdownUnreadable).to_owned(),
            Self::Write(fault) => fault.message(lang),
        }
    }
}

/// Where the System prompt tab's save stands.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Saving {
    /// Nothing has been asked for.
    #[default]
    Idle,
    /// Asked, no answer yet. The controls are laid out and refuse, because
    /// a control that vanishes mid-press is worse than one that waits.
    Sending,
    /// It landed.
    Saved,
    /// It did not, and nothing was changed.
    Failed(SaveFailure),
}

/// Whether this surface has a run to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Availability {
    /// Nothing has been asked for yet, or the answer has not arrived.
    #[default]
    Loading,
    /// A run is held; [`Snapshot`] has it.
    Live,
    /// The worker would not answer. Whatever was last read stays on
    /// screen, read-only, and says so.
    Down,
}

/// What this device may do about the run it is showing.
///
/// D15 in one value. `Executing` is the device the header names; every
/// other device is `Elsewhere` and lays out **no action at all** — not a
/// dimmed one with a tooltip, not a disabled chip: absent. The plan is
/// explicit about it, and the reason is that a control which is present
/// but refuses is a promise the device cannot keep, on a surface whose
/// only job elsewhere is to tell the truth about a run somebody else is
/// running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// This device is running the work; its actions are laid out.
    Executing,
    /// Another device is. Read-only, and the surface names it.
    Elsewhere { device: Option<DeviceId> },
}

impl Standing {
    pub fn may_act(self) -> bool {
        matches!(self, Self::Executing)
    }
}

/// Where a takeover this surface asked for stands.
///
/// The surface does not decide any of this — the orchestrator does, and the
/// host reports it from the session's own `handover` (§7.4: the surface
/// must not claim a takeover it has not been granted). What the surface
/// keeps is *that it asked*, so it can say **waiting for the step to end**
/// rather than leaving the person to guess whether the click landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Takeover {
    #[default]
    Idle,
    /// Asked, and the agent has not let go yet.
    Asked,
    /// The lease is free; the host is opening the session.
    Released,
    /// The person holds it.
    Held,
}

impl Takeover {
    /// What the session's own handover says, mapped onto what the surface
    /// shows. `Requested` is the only state the surface could also have
    /// reached on its own, and it agrees with it.
    pub fn of(handover: HandoverState) -> Self {
        match handover {
            HandoverState::Idle | HandoverState::Returned => Self::Idle,
            HandoverState::Requested => Self::Asked,
            HandoverState::Released => Self::Released,
            HandoverState::Taken => Self::Held,
        }
    }
}

/// **The folder a run worked in, and whether it is still there.**
///
/// The path comes off the wire; the answer to *is it still on this disk* is
/// asked once, by the worker, beside the read that carried the path — never
/// in a frame (standing requirement 3). A folder that has been moved or
/// deleted must say so rather than being offered as a link to nothing.
///
/// **A path is a person's own** (§13.7). It is drawn and it is handed to the
/// file manager; it reaches no log, no event attribute and no `Debug`, which
/// is why this type writes its own.
#[derive(Clone, PartialEq, Eq)]
pub struct RunFolder {
    path: String,
    present: bool,
}

impl RunFolder {
    /// **Look, once, on the thread that is allowed to.** `None` when the
    /// device answering did not name a folder at all.
    pub fn look(path: Option<&str>) -> Option<Self> {
        let path = path?;
        Some(Self {
            present: std::path::Path::new(path).is_dir(),
            path: path.to_owned(),
        })
    }

    /// One this device could not look at — what a test holds, and what a
    /// surface with no worker behind it would get.
    pub fn unchecked(path: &str, present: bool) -> Self {
        Self {
            path: path.to_owned(),
            present,
        }
    }

    /// Whether it was still there when the run was last read.
    pub fn present(&self) -> bool {
        self.present
    }

    /// **The folder's own name**, which is what a person recognises and what
    /// is drawn. The whole path is not: it is a wide line of little use next
    /// to a control that opens the folder itself.
    ///
    /// Falls back to the whole path for a root — `D:\` has no name — because
    /// an empty line is worse than a short one.
    pub fn name(&self) -> &str {
        std::path::Path::new(&self.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.path)
    }

    /// The path itself, for the one caller that opens it.
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Whether the folder was there, and nothing else. The path is the person's.
impl std::fmt::Debug for RunFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunFolder")
            .field("present", &self.present)
            .finish()
    }
}

/// One run, as this surface holds it: the wire's answer plus the window of
/// history it has read so far.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub run: RunId,
    pub workspace: Option<WorkspaceId>,
    pub header: RunHeader,
    /// The state of each pin, in the header's order — or empty, which
    /// means *this device could not tell*, never *gone* (D46).
    pub context: Vec<ContextState>,
    /// The provider session this device is driving, when it is driving one.
    pub session: Option<SessionId>,
    /// The device that answered.
    pub device: DeviceId,
    /// The folder the run worked in, as the device driving it resolved it.
    /// `None` elsewhere — a scope's *kind* travels in the header, and which
    /// folder it turned out to be does not.
    pub folder: Option<RunFolder>,
    /// The window of history held, oldest first, and where it starts.
    pub items: Vec<TimelineItem>,
    pub from: u64,
    pub total: u64,
}

impl Snapshot {
    pub fn from_view(
        run: RunId,
        workspace: Option<WorkspaceId>,
        view: RunView,
        folder: Option<RunFolder>,
    ) -> Self {
        Self {
            run,
            workspace,
            header: view.header,
            context: view.context,
            session: view.execution.session,
            device: view.execution.device,
            folder,
            items: view.timeline.items,
            from: view.timeline.from,
            total: view.timeline.total,
        }
    }

    /// D15's one comparison.
    pub fn standing(&self) -> Standing {
        match self.header.executing_device() {
            Some(device) if device == self.device => Standing::Executing,
            other => Standing::Elsewhere { device: other },
        }
    }

    pub fn control(&self) -> ControlState {
        self.header.status.control().unwrap_or_default()
    }

    /// **D29 in one place.** A run that ended having registered nothing is
    /// finished *needing attention*, and a run still carrying that marker
    /// has not delivered a result either. Neither reads as delivered.
    pub fn delivered(&self) -> bool {
        !self.header.artifacts.is_empty()
    }

    pub fn needs_attention(&self) -> bool {
        self.header
            .status
            .attention()
            .is_some_and(|set| !set.is_empty())
    }

    pub fn finished_needing_attention(&self) -> bool {
        self.header
            .status
            .attention()
            .is_some_and(|set| set.contains(Attention::FinishedNeedingAttention))
    }

    /// D44's *finished*: over as far as the person is concerned. The same
    /// rule the worker applies, so the surface never offers a gesture the
    /// worker would refuse.
    pub fn rateable(&self) -> bool {
        self.header.status.is_terminal() || self.finished_needing_attention()
    }

    pub fn resumable(&self) -> bool {
        matches!(self.header.status, RunStatus::Interrupted { .. })
    }

    /// **Whether this run is over**, and so can be asked for again (ADR-366).
    ///
    /// Deliberately the exact complement of [`resumable`](Self::resumable) as
    /// far as any real status goes: `Interrupted` is the one status that is
    /// not terminal and not running, and it is Resume's alone. The two
    /// controls can therefore never stand on the same run, which is the whole
    /// of how a person is kept from mistaking one for the other.
    pub fn re_runnable(&self) -> bool {
        self.header.status.is_terminal()
    }

    /// Whether ending the run is still a thing that could happen.
    pub fn endable(&self) -> bool {
        !self.header.status.is_terminal()
    }
}

/// Something the worker refused, in tokens rather than sentences: it does
/// not know which language a window is drawing in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub operation: crate::Operation,
    pub code: String,
}
