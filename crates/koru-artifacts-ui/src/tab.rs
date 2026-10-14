// SPDX-License-Identifier: MPL-2.0

//! **One artifact version, open as a tab in the main window.**
//!
//! A result is a document, and a document in Koru opens as a tab (ADR-368).
//! This is that tab's surface — the *one* of them. It replaced two: the run
//! pane's read-only view of a result and the Artifacts column's in-column
//! reader, whose English button text was word for word the same and which
//! were the same screen drawn twice (ADR-369).
//!
//! ## Why it lives in this crate
//!
//! Because the tab **saves**. The Artifacts column's reader offered *Save as
//! version N*, and moving it onto a tab had to keep that; a version is
//! immutable (ADR-344), so saving means registering the next one — the
//! workspace's blob store first, then `register_version` naming the blob,
//! which is P1.11's own register path and not a second one. That path needs
//! `koru-artifact-wire` and `koru-store-core`, and `koru-runs-ui`'s
//! dependency fence forbids both: a run view that could name a family could
//! reach an artifact without going through the store's own rule. So the
//! surface sits where the saving already lived.
//!
//! ## What it is told, and what it finds out
//!
//! The host tells it what a person can already see: the version's id, what
//! the result is called, how big it is, and — when this device does not hold
//! the bytes — which device does (D27). Everything a *save* needs it learns
//! for itself, on its worker, when it reads the version: the family the next
//! version goes under, the workspace whose blob store the bytes belong in,
//! and the number this version already is. That is deliberate: a tab opened
//! from a run is opened from the run's header, and a run's header carries no
//! family.
//!
//! **Nothing here touches disk, the store or the network.** Every read and
//! every save is the [`crate::reader`]'s, on a thread of its own; the window
//! thread only folds what came back.
//!
//! ## What this tab does *not* draw: the document
//!
//! A Markdown result is a document, and Koru already has a document view —
//! `koru-editor`, the one every page in the library opens in. It is three
//! crates this surface may not name ([`crate::tests::dependency`]), and that
//! fence is deliberate: a projection of the Artifact Store must not be able
//! to reach a document somebody owns. So the tab does not render the
//! document at all. It draws everything *around* one — the name, the
//! version, the controls, the sentences — and everything that is **not** a
//! document: bytes still being read, bytes on another device, bytes that
//! would not read, and bytes that are not Markdown.
//! [`ArtifactTab::document_for_host`] is the whole of the rule, and it is one
//! function so that the layout, the drawing and the host cannot disagree
//! about which of them is showing the bytes (ADR-372).
//!
//! ## Editing is that same document, and there is no second way to do it
//!
//! *Edit* used to swap the page for a composer holding the document's
//! Markdown. It is gone (ADR-378). Editing is the document view with its
//! read-only flag off, which is why this file no longer holds a buffer, a
//! caret or a text engine's worth of composer: the host owns the page, the
//! host is told when to make it editable, and the host hands back the
//! Markdown its exporter wrote when a save is asked for.
//!
//! That leaves the tab with **the honesty**, which is its own job:
//! [`ArtifactTab::begin_editing`] is refused for anything that is not a
//! readable document with a place to save to, and the sentence under the
//! controls says both what a save makes (ADR-344) and — when the version
//! does not survive its own round trip — that a save rewrites lines nobody
//! touched. `crates/koru-markdown-view/src/tests/round_trip.rs` is the
//! measurement that sentence stands on.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_i18n::{Key, Lang, fill, number, t};
use koru_markdown_view::{Content, MarkdownView};
use vello::Scene;
use vello::kurbo::Affine;
use winit::event::MouseScrollDelta;

use crate::Work;
use crate::reader;

mod draw;
mod input;

#[cfg(test)]
mod tests;

/// The widest a result is laid out in, however wide the window is. The run
/// views' own measure: a result read in a tab and a run read beside it must
/// not wrap differently.
pub(crate) const COLUMN_W: f32 = 620.0;
pub(crate) const PAD_X: f32 = 24.0;
pub(crate) const TOP: f32 = 20.0;
/// What the result is called.
const TITLE_H: f32 = 32.0;
/// The line under it — which version, and how big.
const META_H: f32 = 20.0;
/// A sentence: what saving would do, or why it did not.
const NOTE_H: f32 = 18.0;
/// Air under the document.
const FOOTER_PAD: f32 = 24.0;
/// Air between the last chrome row and the document the host draws under it.
const BAND_GAP: f32 = 12.0;

/// **What a host hands the tab.** Everything a person can already see about
/// a result before a byte of it is read — which is exactly what both the
/// surfaces that open one already hold.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenedArtifact {
    pub version: koru_artifact_wire::ArtifactVersionId,
    /// What the result is called — the tab strip's label, and the heading.
    pub title: String,
    pub size_bytes: u64,
    /// The run that produced this version, when it came from one. This is a
    /// navigation target, not authority: the host separately tells the tab
    /// whether that durable run still exists before the control is offered.
    pub produced_by: Option<koru_artifact_wire::RunId>,
    /// `None` when this device holds the bytes. `Some(device)` when it does
    /// not, naming the device that does when one can be named (D27) — so the
    /// tab says so without asking a gateway a question it would refuse.
    pub held_elsewhere: Option<Option<String>>,
}

impl std::fmt::Debug for OpenedArtifact {
    /// **A result's title is content** (standing requirement 7): it sits
    /// where a document's name sits and it is somebody's writing. The id,
    /// the size and *whether* the bytes are elsewhere are what a reader of
    /// one needs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedArtifact")
            .field("version", &self.version)
            .field("size_bytes", &self.size_bytes)
            .field("produced_by", &self.produced_by.is_some())
            .field("elsewhere", &self.held_elsewhere.is_some())
            .finish()
    }
}

/// **Where a version sits**, learned from the store rather than told.
///
/// A tab opened from a run knows a version id, a title and a size and
/// nothing else — the run's header carries no family. Everything a *save*
/// needs is here, read once when the version is opened, on the worker: the
/// family the next version goes under, the workspace whose blob store the
/// bytes go into, and the number this one already is, so the control can say
/// what the next one would be called before it is pressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VersionPlace {
    pub family: koru_artifact_wire::ArtifactFamilyId,
    pub workspace: koru_artifact_wire::WorkspaceId,
    pub ordinal: u32,
}

/// Where a save stands.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum SavingVersion {
    #[default]
    Idle,
    /// Sent, no answer yet. The controls stay laid out and refuse, because a
    /// control that vanishes mid-press is worse than one that waits.
    Sending,
    /// Refused, by the stable code the route answered. The tab renders that
    /// code as a sentence; the code itself is never drawn.
    Refused(String),
}

/// One line of the tab.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TabRow {
    /// What the result is called.
    Title,
    /// Which version it is, and how big.
    Meta,
    /// The producing run, as an icon immediately before the edit control.
    Run,
    Edit,
    Save,
    StopEditing,
    /// One sentence about the open version.
    Note(usize),
    /// **Everything the surface shows that is not a document**: the sentence
    /// for bytes still being read, held elsewhere or unreadable, and the
    /// lines naming what a non-Markdown version is. A version that *is* a
    /// readable document has no such row — the host draws it, in Koru's own
    /// document view, editable or not. Its height is the document surface's,
    /// because that is where it was measured.
    Document,
}

impl TabRow {
    /// Whether a press on this row does anything. A title, a meta line and a
    /// note are prose; the keyboard steps over them.
    pub fn pressable(self) -> bool {
        matches!(
            self,
            Self::Run | Self::Edit | Self::Save | Self::StopEditing
        )
    }

    fn height(self) -> f32 {
        match self {
            Self::Title => TITLE_H,
            Self::Meta => META_H,
            Self::Note(_) => NOTE_H,
            Self::Document => 0.0,
            _ => theme::NAV_ROW_H,
        }
    }
}

/// **The result tab.**
pub struct ArtifactTab {
    lang: Lang,
    /// The tab's own text engine. A document measures itself, and the host
    /// hands an engine only when it draws — too late for a hit test.
    engine: TextEngine,
    opened: Option<OpenedArtifact>,
    /// Where the open version sits, once the worker has said. `None` until
    /// then, and for a version the store would not describe — the edit is
    /// not offered while it is, because saving would not know what it makes.
    place: Option<VersionPlace>,
    view: MarkdownView,
    saving: SavingVersion,
    viewport: (f32, f32),
    /// The text column the host wants the chrome laid out in, when it has
    /// one. A host drawing the document below these rows knows where that
    /// document's prose starts, and a heading sitting over prose that begins
    /// somewhere else is two columns, not one.
    column: Option<(f32, f32)>,
    scroll: f32,
    laid: Vec<(TabRow, Rect)>,
    stale: bool,
    hover: Option<TabRow>,
    focus: Option<TabRow>,
    /// **Whether the page below is open for typing.** The tab holds the
    /// *decision*; the host holds the page, and reads this to build it
    /// editable. Not `view.is_editing()` any more — that is the composer,
    /// and the composer is gone from this surface (ADR-378).
    editing: bool,
    /// Whether the host's page has been changed since the edit opened.
    /// Pushed down each time the host settles, because only the page knows;
    /// the tab uses it for one thing, which is whether *Save* is live.
    document_edited: bool,
    /// *Save* was pressed and the host has not been handed the ask yet.
    ///
    /// A press cannot save on its own: the bytes are the host's document,
    /// and exporting one is work proportional to it that must not happen in
    /// a hit test. So the press records the ask and
    /// [`ArtifactTab::take_save_request`] hands it over once.
    save_requested: bool,
    /// Whether the producing run still resolves in the host's live or Trash
    /// catalog. Kept apart from `produced_by`: retirement must remove the
    /// control without forgetting which run made the immutable version.
    run_available: bool,
    /// A click on the run icon, waiting for the window that owns tabs.
    wants_run: bool,
    work: Work,
    link: Option<reader::Reader>,
}

impl Default for ArtifactTab {
    fn default() -> Self {
        Self::without_reader()
    }
}

impl ArtifactTab {
    /// A tab with its worker running.
    ///
    /// **Never under test.** `cfg(test)` covers this crate's own suite and
    /// the `no-reader` feature covers a host's, so nothing in a test build
    /// dials a gateway.
    pub fn new(root: Option<std::path::PathBuf>) -> Self {
        let mut tab = Self::without_reader();
        if cfg!(test) || cfg!(feature = "no-reader") {
            return tab;
        }
        tab.link = reader::Reader::start_acting(root).ok();
        tab
    }

    /// A tab that will never read anything — for a test, and for a host that
    /// drives it itself.
    pub fn without_reader() -> Self {
        Self {
            lang: Lang::default(),
            engine: TextEngine::new(),
            opened: None,
            place: None,
            view: MarkdownView::new(),
            saving: SavingVersion::Idle,
            viewport: (COLUMN_W, 600.0),
            column: None,
            scroll: 0.0,
            laid: Vec::new(),
            stale: true,
            hover: None,
            focus: None,
            editing: false,
            document_edited: false,
            save_requested: false,
            run_available: false,
            wants_run: false,
            work: Work::default(),
            link: None,
        }
    }

    // ------------------------------------------------------ what it knows

    /// The version this tab holds — how a host finds the tab already open on
    /// a result rather than opening a second one on it.
    pub fn version(&self) -> Option<koru_artifact_wire::ArtifactVersionId> {
        self.opened.as_ref().map(|opened| opened.version)
    }

    /// What the result is called. The tab strip's label, and the heading.
    pub fn title(&self) -> Option<&str> {
        self.opened.as_ref().map(|opened| opened.title.as_str())
    }

    /// The document, for a test to read the tab out of.
    pub fn view(&self) -> &MarkdownView {
        &self.view
    }

    pub fn saving(&self) -> &SavingVersion {
        &self.saving
    }

    /// The number a save would make, once the store has said what this
    /// version is called.
    pub fn next_ordinal(&self) -> Option<u32> {
        self.place.map(|place| place.ordinal.saturating_add(1))
    }

    pub fn work(&self) -> Work {
        self.work
    }

    /// The run that made the open version, whether or not it still resolves.
    pub fn producing_run(&self) -> Option<koru_artifact_wire::RunId> {
        self.opened.as_ref().and_then(|opened| opened.produced_by)
    }

    /// Tell the tab whether its producing run still has a durable record.
    pub fn set_run_available(&mut self, available: bool) -> bool {
        let available = available && self.producing_run().is_some();
        if self.run_available == available {
            return false;
        }
        self.run_available = available;
        self.stale = true;
        true
    }

    /// Take one request to open the producing run.
    pub fn take_wants_run(&mut self) -> Option<koru_artifact_wire::RunId> {
        if !std::mem::take(&mut self.wants_run) {
            return None;
        }
        self.producing_run()
    }

    pub fn set_lang(&mut self, lang: Lang) {
        if self.lang != lang {
            self.lang = lang;
            self.view.set_lang(lang);
            self.stale = true;
        }
    }

    // ------------------------------------------------------- what it does

    /// **Open one version and read it.**
    ///
    /// A version whose bytes are on another device is answered from what the
    /// host already knows (D27) rather than by asking a gateway that would
    /// have to refuse.
    pub fn open(&mut self, opened: OpenedArtifact) {
        let elsewhere = opened.held_elsewhere.clone();
        let version = opened.version;
        self.opened = Some(opened);
        self.place = None;
        self.saving = SavingVersion::Idle;
        self.scroll = 0.0;
        self.focus = None;
        self.hover = None;
        self.editing = false;
        self.document_edited = false;
        self.save_requested = false;
        // Availability is host knowledge. A producer id alone is not proof
        // that the durable Run still exists in this workspace.
        self.run_available = false;
        self.wants_run = false;
        self.stale = true;
        self.view.set_lang(self.lang);
        match elsewhere {
            Some(device) => self.view.set_content(Content::Elsewhere { device }),
            None => {
                self.view.set_content(Content::Reading);
                if let Some(link) = &self.link {
                    link.act(reader::Act::Content { version });
                }
            }
        }
    }

    /// **Open the version's document for typing.**
    ///
    /// Refused unless there is a readable document *and* somewhere to save
    /// it — the same pair [`Self::plan`] offers the control on, asked here
    /// too so that a host driving this by keyboard cannot reach a state the
    /// rows never showed.
    pub fn begin_editing(&mut self) {
        if self.opened.is_none() || self.place.is_none() || self.document_for_host().is_none() {
            return;
        }
        self.editing = true;
        self.document_edited = false;
        self.saving = SavingVersion::Idle;
        self.stale = true;
    }

    /// Close the edit and throw it away. The host rebuilds its page from the
    /// version's own bytes, so nothing typed survives this.
    pub fn stop_editing(&mut self) {
        self.editing = false;
        self.document_edited = false;
        self.stale = true;
    }

    /// Whether the page below should be open for typing.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// The host's page has (or has not) been changed since the edit opened.
    pub fn set_document_edited(&mut self, edited: bool) {
        if self.document_edited != edited {
            self.document_edited = edited;
            self.stale = true;
        }
    }

    /// Whether the host's page has been changed since the edit opened.
    pub fn document_edited(&self) -> bool {
        self.document_edited
    }

    /// **Take the pending ask to save, once.** `true` means the host owes
    /// this tab the Markdown its page exports to, through
    /// [`Self::save_as_next_version`].
    pub fn take_save_request(&mut self) -> bool {
        std::mem::take(&mut self.save_requested)
    }

    /// **Register the edit as the next version.** Never as a change to this
    /// one: a version is immutable, and the control said so before it was
    /// pressed.
    ///
    /// `markdown` is what the host's document exports to, through the same
    /// exporter every other export in Koru goes through. A save whose text
    /// is what the open version already holds is not sent: it would make a
    /// second copy of a version rather than a revision of it.
    pub fn save_as_next_version(&mut self, markdown: String) {
        let Some(opened) = self.opened.as_ref() else {
            return;
        };
        let Some(place) = self.place else { return };
        if self
            .view
            .document()
            .is_some_and(|open| open.markdown() == markdown)
        {
            return;
        }
        self.saving = SavingVersion::Sending;
        self.stale = true;
        if let Some(link) = &self.link {
            link.act(reader::Act::SaveVersion {
                workspace: place.workspace,
                family: place.family,
                derived_from: opened.version,
                text: markdown,
            });
        }
    }

    /// **The host could not turn its page back into Markdown.** Nothing was
    /// sent and the open version is exactly what it was — said in the same
    /// place, and with the same words, as a refusal from the store.
    pub fn refuse_save(&mut self, code: &str) {
        self.saving = SavingVersion::Refused(code.to_owned());
        self.stale = true;
    }

    /// Take in whatever the worker has answered. `true` when the tab changed
    /// and a repaint is owed.
    pub fn poll(&mut self) -> bool {
        let Some(link) = &self.link else {
            return false;
        };
        let answers = link.drain();
        if answers.is_empty() {
            return false;
        }
        for answer in answers {
            match answer {
                reader::Answer::Content {
                    version,
                    place,
                    content,
                } => self.take_content(version, place, *content),
                reader::Answer::Saved { result } => self.take_saved(result),
                // Nothing else is ever asked for on this link.
                _ => {}
            }
        }
        self.stale = true;
        true
    }

    /// Accept a version's content the host fetched itself — the seam a host
    /// without this crate's worker drives, and the one every test uses.
    pub fn take_content(
        &mut self,
        version: koru_artifact_wire::ArtifactVersionId,
        place: Option<VersionPlace>,
        content: Content,
    ) {
        if self.version() != Some(version) {
            return;
        }
        self.place = place;
        self.view.set_content(content);
        // **The bytes under an open edit have changed.** Keeping the edit
        // would let a save write a revision of something that is no longer
        // there — the same rule `MarkdownView::set_content` has always kept
        // for its own composer.
        self.editing = false;
        self.document_edited = false;
        self.stale = true;
    }

    /// Accept a save's answer.
    pub fn take_saved(
        &mut self,
        result: Result<
            (koru_artifact_wire::ArtifactVersionId, u32, Box<Content>),
            koru_artifact_wire::Failure,
        >,
    ) {
        match result {
            Ok((version, ordinal, content)) => {
                // **The tab follows the version it made.** A save registers
                // the next version and this tab is now reading it: the id it
                // is keyed by, the number in its heading and the document on
                // screen all move together, so a person who saves and then
                // saves again builds a chain rather than two version 2s.
                if let Some(opened) = self.opened.as_mut() {
                    opened.version = version;
                }
                if let Some(place) = self.place.as_mut() {
                    place.ordinal = ordinal;
                }
                self.saving = SavingVersion::Idle;
                self.view.set_content(*content);
                self.editing = false;
                self.document_edited = false;
            }
            Err(failure) => self.saving = SavingVersion::Refused(failure.code),
        }
        self.stale = true;
    }

    // ------------------------------------------------------------ geometry

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        if self.viewport != (w, h) {
            self.viewport = (w, h);
            self.stale = true;
        }
    }

    /// Lay the chrome out in this text column instead of the tab's own.
    ///
    /// The host that draws the document knows where that document's prose
    /// begins and how wide it runs; giving the tab the same column is what
    /// keeps the result's name, its version line and its *Edit* over the
    /// text rather than beside it.
    pub fn set_text_column(&mut self, x: f32, w: f32) {
        let column = Some((x, w.max(120.0)));
        if self.column != column {
            self.column = column;
            self.stale = true;
        }
    }

    /// The column the document is laid out in: centred, and never wider than
    /// text stays readable in — unless the host has said where its own text
    /// column is, in which case that one, so the two line up.
    pub(crate) fn column(&self) -> Rect {
        let (x, width) = match self.column {
            Some((x, w)) => (x, w),
            None => {
                let width = COLUMN_W.min(self.viewport.0 - PAD_X * 2.0).max(120.0);
                (((self.viewport.0 - width) * 0.5).max(PAD_X), width)
            }
        };
        Rect::new(x, TOP, width, (self.viewport.1 - TOP).max(0.0))
    }

    /// **The document this tab is not drawing** — `Some` exactly when the
    /// host should open it in Koru's document view, and `None` whenever the
    /// tab has something of its own to show in that space.
    ///
    /// One function, asked by the layout, by the drawing and by the host, so
    /// there is never a frame in which two of them disagree about who is
    /// showing the bytes. **An open edit does not change the answer**: the
    /// page is the same page, with typing allowed (ADR-378) — which is
    /// exactly why the tab now offers *Edit* on the same condition it hands
    /// the document over on, and never on one the rows never showed.
    pub fn document_for_host(&self) -> Option<&koru_markdown_view::MarkdownDocument> {
        // **A version with nothing in it is a state, not a page.** An empty
        // file opened as a document would be an empty column under a name,
        // which reads as a surface that failed rather than as a result that
        // holds nothing — so the sentence saying so stays this tab's.
        self.view.document().filter(|document| !document.is_empty())
    }

    /// Where the document the host draws begins, in this tab's own
    /// coordinates: under the last chrome row, with air between them.
    pub fn document_top(&mut self) -> f32 {
        self.relayout();
        self.laid
            .last()
            .map(|(_, rect)| rect.y + rect.h + BAND_GAP)
            .unwrap_or_else(|| self.column().y)
    }

    /// Every row, in order, with its rect. The one geometry function:
    /// drawing, hit-testing, the hover, the keyboard and the scroll clamp
    /// all come from here, so a row cannot be pressable somewhere other than
    /// where it is drawn.
    pub fn relayout(&mut self) {
        if !self.stale {
            return;
        }
        self.stale = false;
        let column = self.column();
        // The surface measures itself first, so every row below it sits where
        // it will be drawn. It does so at this width, on this surface's own
        // engine — and not at all when what it holds is a document the host
        // is drawing, because laying out blocks nobody will draw is the
        // whole of what a second renderer costs.
        if self.document_for_host().is_none() {
            let mut engine = std::mem::replace(&mut self.engine, TextEngine::new());
            let mut view = std::mem::take(&mut self.view);
            view.relayout(&mut engine, column.w.max(1.0));
            self.view = view;
            self.engine = engine;
        }

        let plan = self.plan();
        self.work.rows = plan.len();
        let mut laid = Vec::with_capacity(plan.len());
        let mut y = column.y;
        let mut at = 0;
        while at < plan.len() {
            let row = plan[at];
            let h = match row {
                TabRow::Document => self.view.height(),
                other => other.height(),
            };
            // The run is an icon immediately before the action, not a whole
            // text row of its own. Both controls still have independent hit
            // targets and keyboard stops; they merely share one baseline.
            if row == TabRow::Run {
                let icon_w = theme::NAV_ROW_H;
                laid.push((row, Rect::new(column.x, y, icon_w, h)));
                if let Some(next) = plan.get(at + 1).copied()
                    && matches!(next, TabRow::Edit | TabRow::Save)
                {
                    laid.push((
                        next,
                        Rect::new(
                            column.x + icon_w + 8.0,
                            y,
                            (column.w - icon_w - 8.0).max(0.0),
                            next.height(),
                        ),
                    ));
                    at += 1;
                }
            } else {
                laid.push((row, Rect::new(column.x, y, column.w, h)));
            }
            y += h;
            at += 1;
        }
        self.laid = laid;
        if self
            .focus
            .is_some_and(|row| !self.laid.iter().any(|(laid, _)| *laid == row))
        {
            self.focus = None;
        }
    }

    /// The rows this tab shows, in order — what a test asserting *what is
    /// on screen* reads.
    pub fn rows(&mut self) -> Vec<TabRow> {
        self.relayout();
        self.laid.iter().map(|(row, _)| *row).collect()
    }

    /// The same rows with the rects they are drawn in, so a press can be
    /// aimed at exactly where a row is.
    pub fn laid_rows(&mut self) -> Vec<(TabRow, Rect)> {
        self.relayout();
        self.laid.clone()
    }

    /// The rows this tab shows, in order.
    fn plan(&self) -> Vec<TabRow> {
        if self.opened.is_none() {
            return Vec::new();
        }
        let mut plan = vec![TabRow::Title, TabRow::Meta];
        if self.run_available {
            plan.push(TabRow::Run);
        }
        // **Edit exactly where there is a page to edit.** Two conditions,
        // and both are about honesty rather than plumbing: the store must
        // have said where this version sits, or a Save could not say which
        // family it was making the next version of; and there must be a
        // document the host is drawing, because *that page* is what editing
        // opens now. An empty version, a version held elsewhere, one that
        // would not read and one that is not Markdown all fail the second
        // and keep the tab's own lines with no edit offered.
        if self.document_for_host().is_some() && self.place.is_some() {
            if self.editing {
                plan.push(TabRow::Save);
                plan.push(TabRow::StopEditing);
            } else {
                plan.push(TabRow::Edit);
            }
        }
        for note in 0..self.notes().len() {
            plan.push(TabRow::Note(note));
        }
        // A readable document is the host's to draw, and the tab's rows end
        // above it. Everything else — a state sentence, a media type, the
        // composer — is this surface's own, and gets the row.
        if self.document_for_host().is_none() {
            plan.push(TabRow::Document);
        }
        plan
    }

    /// How tall the tab is, unscrolled.
    fn content_h(&mut self) -> f32 {
        self.relayout();
        let top = self.column().y;
        self.laid
            .last()
            .map(|(_, rect)| rect.y + rect.h - top + FOOTER_PAD)
            .unwrap_or(0.0)
    }

    fn clamp_scroll(&mut self) {
        let band = (self.viewport.1 - TOP).max(0.0);
        let max = (self.content_h() - band).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    pub fn scroll_by(&mut self, delta: MouseScrollDelta) {
        let step = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * 48.0,
            MouseScrollDelta::PixelDelta(position) => position.y as f32,
        };
        self.scroll -= step;
        self.clamp_scroll();
    }

    /// Which row is under a point, if any. In view space: the rows are laid
    /// out unscrolled, so the scroll is added to the point rather than taken
    /// off every rect.
    fn row_at(&mut self, x: f32, y: f32) -> Option<(TabRow, Rect)> {
        self.relayout();
        let y = y + self.scroll;
        self.laid
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .copied()
    }

    // --------------------------------------------------------------- words

    /// The sentences under the controls, in order.
    ///
    /// While an edit is open, the one that matters: what saving would do,
    /// said before the click. After a refusal, what happened and what is
    /// still true.
    pub(crate) fn notes(&self) -> Vec<String> {
        let Some(place) = self.place else {
            return Vec::new();
        };
        let lang = self.lang;
        let mut notes = Vec::new();
        if self.editing {
            notes.push(fill(
                t(lang, Key::ArtifactsSaveVersionNote),
                &[
                    &number::decimal(lang, u64::from(place.ordinal.saturating_add(1))),
                    &number::decimal(lang, u64::from(place.ordinal)),
                ],
            ));
            // **The round trip, said out loud, and only when it is true of
            // this version** (ADR-378). Editing a result means its Markdown
            // goes through Koru's node model and comes back out, and a
            // handful of constructs come back spelled differently — an
            // ordered list that did not start at 1, a hard line break, a
            // footnote, an HTML block.
            // `crates/koru-markdown-view/src/tests/round_trip.rs` is the
            // measurement. A version whose
            // bytes *are* their own round trip says nothing, because there
            // is nothing to say.
            if self
                .view
                .document()
                .is_some_and(|document| !document.is_faithful())
            {
                notes.push(t(lang, Key::ArtifactsSaveRespells).to_owned());
            }
        }
        if let SavingVersion::Refused(code) = &self.saving {
            notes.push(fill(
                t(lang, Key::ArtifactsSaveFailed),
                &[&number::decimal(lang, u64::from(place.ordinal))],
            ));
            notes.push(crate::text::refusal(lang, code).to_owned());
        }
        notes
    }

    /// The word on the Save control, which names the version it would make.
    pub(crate) fn save_label(&self) -> String {
        let next = self.next_ordinal().unwrap_or(1);
        fill(
            t(self.lang, Key::ArtifactsSaveVersion),
            &[&number::decimal(self.lang, u64::from(next))],
        )
    }

    /// Which version this is, and how big — the line under the title.
    pub(crate) fn meta_line(&self) -> String {
        let size = self
            .opened
            .as_ref()
            .map(|opened| crate::text::size(self.lang, opened.size_bytes))
            .unwrap_or_default();
        match self.place {
            Some(place) => {
                let version = fill(
                    t(self.lang, Key::ArtifactsVersion),
                    &[&number::decimal(self.lang, u64::from(place.ordinal))],
                );
                format!("{version} · {size}")
            }
            None => size,
        }
    }

    /// Every line of text this tab is showing, in layout order — what a test
    /// reads a result out of.
    pub fn lines(&mut self) -> Vec<String> {
        self.relayout();
        let rows: Vec<TabRow> = self.laid.iter().map(|(row, _)| *row).collect();
        let mut lines = Vec::new();
        for row in rows {
            match row {
                TabRow::Title => lines.extend(self.title().map(str::to_owned)),
                TabRow::Meta => lines.push(self.meta_line()),
                // Icon-only navigation; it contributes no prose line.
                TabRow::Run => {}
                TabRow::Edit => lines.push(t(self.lang, Key::ArtifactsEdit).to_owned()),
                TabRow::Save => lines.push(self.save_label()),
                TabRow::StopEditing => {
                    lines.push(t(self.lang, Key::ArtifactsStopEditing).to_owned())
                }
                TabRow::Note(index) => lines.extend(self.notes().into_iter().nth(index)),
                TabRow::Document => lines.extend(self.view.lines()),
            }
        }
        lines
    }

    // ---------------------------------------------------------------- paint

    /// Draw everything again: the palette, the density or the font moved
    /// under this surface.
    pub fn invalidate(&mut self) {
        self.engine = TextEngine::new();
        self.stale = true;
    }

    pub fn draw(&mut self, scene_out: &mut Scene, root: Affine) {
        self.clamp_scroll();
        self.relayout();
        let (vw, vh) = self.viewport;
        let p = theme::p();
        scene::fill(scene_out, root, p.bg, &scene::rect(0.0, 0.0, vw, vh));
        let shifted = root * Affine::translate((0.0, -(self.scroll as f64)));
        self.work.layouts = 0;
        let rows = self.laid.clone();
        for (row, rect) in rows {
            self.draw_row(scene_out, shifted, row, rect, p);
        }
    }
}
