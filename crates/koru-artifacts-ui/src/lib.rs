// SPDX-License-Identifier: MPL-2.0

//! **The Artifacts surface** — what a delegated run produced, and what it
//! left behind that is not a result yet.
//!
//! You hand a piece of work to an agent, walk away, and come back to a
//! result you can read, trust and use again (ROADMAP §1). This column is
//! where you come back to. It shows, for the workspace in front of you:
//!
//! - **The results.** A *family* is the thing you name — "the quarterly
//!   report" — and its *versions* accumulate under it, newest first, each
//!   with its size and, when you select one, **its provenance as a
//!   sentence**: where it came from, what it was built on, and who has used
//!   it since. Not a graph. A sentence.
//! - **What is not a result yet** (D19). An agent writes under
//!   `<run-workspace>/.artifacts/<RunId>/`, and a file it never registered
//!   is not a result (D29) — so those files sit here, in their own section,
//!   visibly not results, each with a one-click **Register**. That
//!   registration goes through the *same* register path rule the agent's
//!   own tools obey (P1.11's `staging`), reached through the gateway route
//!   the person's own session is entitled to, so there is one rule about
//!   what may become a result and not one per caller.
//! - **What this device cannot reach.** A version whose bytes are not in
//!   this device's blob store is named, sized and dated all the same, and
//!   says the content is elsewhere (D27) rather than offering something it
//!   cannot give.
//!
//! ## Three rules it is built under
//!
//! **Opening a family costs what is shown, never what it has been.** A
//! family with five thousand versions lays out exactly as many rows as one
//! with five, because only [`VERSIONS_SHOWN`] of them are rows at all and
//! the rest are behind one *N earlier versions* line. [`Work`] counts what
//! was built so a test can assert the ratio rather than a wall clock.
//!
//! **Nothing here touches disk, the store or the network.** Every read is
//! the [`reader`]'s, on its own threads; the window thread only folds what
//! came back. Under `cfg(test)` and with the `no-reader` feature there is
//! no reader at all, so no test can dial a developer's own gateway.
//!
//! **Every word goes through `koru-i18n`** and every time and size through
//! its formatters — see [`text`], which is the only module that spells a
//! sentence.
//!
//! ## What it is not
//!
//! It is not the reader. Pressing *Open* on a version opens
//! [`ArtifactTab`] — a tab in the main window, the way every other document
//! in Koru opens (ADR-369) — and the column keeps its list. It does not
//! read a result, and it never did more than one thing at a time.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use koru_artifact_wire::RunId;
use koru_artifact_wire::run_folder::RelativePath;
use koru_i18n::Lang;

mod draw;
mod gateway;
mod input;
pub mod model;
mod reader;
mod result;
mod rows;
mod tab;
pub mod text;

pub use draw::MODE_ICON;
pub use result::{ResultBody, read_result};
// The ids a host names when it says which version it opened and where that
// version sits, so a host drawing artifacts need not name the wire crate.
pub use koru_artifact_wire::{ArtifactFamilyId, ArtifactVersionId, BlobHash, WorkspaceId};
pub use koru_markdown_view::{Content, MarkdownDocument, MarkdownView};
pub use model::{Catalog, FamilyEntry, Load, Registering, RunEntry};
pub use rows::{Notice, Row, RowParts, VERSIONS_SHOWN};
pub use tab::{ArtifactTab, OpenedArtifact, SavingVersion, TabRow, VersionPlace};

/// What the surface built, so a test can assert a ratio rather than a wall
/// clock (standing requirement 3).
///
/// `rows` is how many rows the last layout produced; `layouts` is how many
/// text layouts the last draw performed. Both are *per call*, reset at the
/// start of each, because what matters is the cost of one open — not the
/// total since the window came up.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct Work {
    pub rows: usize,
    pub layouts: usize,
}

/// A key press the column answers. Its own vocabulary rather than winit's,
/// so the keyboard can be driven from a test without building an event.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Press {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    Escape,
}

impl Press {
    /// The keys this column claims, and **only** those: everything else
    /// falls through, so a focused column never makes the window feel
    /// stuck (`DESIGN.md`, the explore column's own rule).
    pub fn of(key: &winit::keyboard::Key) -> Option<Self> {
        use winit::keyboard::{Key, NamedKey};
        match key {
            Key::Named(NamedKey::ArrowUp) => Some(Self::Up),
            Key::Named(NamedKey::ArrowDown) => Some(Self::Down),
            Key::Named(NamedKey::ArrowLeft) => Some(Self::Left),
            Key::Named(NamedKey::ArrowRight) => Some(Self::Right),
            Key::Named(NamedKey::Enter) => Some(Self::Enter),
            Key::Named(NamedKey::Space) => Some(Self::Space),
            Key::Named(NamedKey::Escape) => Some(Self::Escape),
            Key::Character(text) if text == " " => Some(Self::Space),
            _ => None,
        }
    }
}

/// The Artifacts column.
pub struct ArtifactsSurface {
    pub(crate) lang: Lang,
    /// The clock every relative time is read against. Pushed by the host,
    /// because this crate has none — the same discipline `koru_i18n::time`
    /// keeps.
    pub(crate) now_ms: i64,
    pub(crate) workspace: Option<WorkspaceId>,
    pub(crate) load: Load,
    pub(crate) catalog: Catalog,
    pub(crate) provenances: model::Provenances,
    /// D27's device names, per run, pushed by a host that knows them. Empty
    /// until something does: a version this device cannot reach then says
    /// the content is on another device without naming one, which is the
    /// half of D27 that can be answered from the Artifact Store alone.
    pub(crate) devices: HashMap<RunId, String>,
    pub(crate) expanded: HashSet<ArtifactFamilyId>,
    /// How many of a family's versions are rows. Absent means
    /// [`VERSIONS_SHOWN`].
    pub(crate) shown: HashMap<ArtifactFamilyId, usize>,
    /// The version whose provenance sentence is showing.
    pub(crate) selected: Option<ArtifactVersionId>,
    pub(crate) registering: HashMap<(RunId, RelativePath), Registering>,
    pub(crate) hover: Option<Row>,
    /// **A version the person asked to read.** The column does not open it:
    /// a document in Koru opens as a tab, and which tab strip it lands in
    /// was never a column's to decide (ADR-369). The host drains this.
    pub(crate) wants_open: Option<OpenedArtifact>,
    /// Where the arrows are. `Some` doubles as *this column holds the
    /// keyboard*, the same two-piece state the explore tree keeps.
    pub(crate) focus: Option<Row>,
    pub(crate) scroll: f32,
    pub(crate) work: Work,
    reader_link: Option<reader::Reader>,
    seq: u64,
}

impl Default for ArtifactsSurface {
    fn default() -> Self {
        Self::without_reader()
    }
}

impl ArtifactsSurface {
    /// A surface with its reader running.
    ///
    /// **Never under test.** `cfg(test)` covers this crate's own suite and
    /// the `no-reader` feature covers a host's, so nothing in a test build
    /// dials a gateway; a surface built there simply shows what it is
    /// pushed.
    pub fn new(root: Option<PathBuf>) -> Self {
        let mut surface = Self::without_reader();
        if cfg!(test) || cfg!(feature = "no-reader") {
            return surface;
        }
        surface.reader_link = reader::Reader::start(root).ok();
        surface
    }

    /// A surface that will never read anything — for a test, and for a host
    /// that drives it itself.
    pub fn without_reader() -> Self {
        Self {
            lang: Lang::default(),
            now_ms: 0,
            workspace: None,
            load: Load::Cold,
            catalog: Catalog::default(),
            provenances: model::Provenances::default(),
            devices: HashMap::new(),
            expanded: HashSet::new(),
            shown: HashMap::new(),
            selected: None,
            registering: HashMap::new(),
            hover: None,
            wants_open: None,
            focus: None,
            scroll: 0.0,
            work: Work::default(),
            reader_link: None,
            seq: 0,
        }
    }

    // ------------------------------------------------------ what it knows

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn set_lang(&mut self, lang: Lang) -> bool {
        let changed = self.lang != lang;
        self.lang = lang;
        changed
    }

    /// The moment relative times are read against. The host pushes it on
    /// the frames it already redraws for; an idle column asks for none.
    pub fn set_now(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
    }

    pub fn load(&self) -> Load {
        self.load
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn work(&self) -> Work {
        self.work
    }

    /// Which workspace the column is about. A change clears what was on
    /// screen — another workspace's results are not this one's — and asks
    /// again.
    pub fn set_workspace(&mut self, workspace: Option<WorkspaceId>) -> bool {
        if self.workspace == workspace {
            return false;
        }
        self.workspace = workspace;
        self.catalog = Catalog::default();
        self.provenances = model::Provenances::default();
        self.expanded.clear();
        self.shown.clear();
        self.selected = None;
        self.registering.clear();
        self.hover = None;
        self.wants_open = None;
        self.focus = None;
        self.scroll = 0.0;
        self.load = Load::Cold;
        self.refresh();
        true
    }

    /// D27's device names: which device holds what a given run produced.
    /// A host with Run documents in hand pushes them; one without pushes
    /// nothing and the column says *another device* rather than a name.
    pub fn set_run_devices(&mut self, devices: HashMap<RunId, String>) -> bool {
        let changed = self.devices != devices;
        self.devices = devices;
        changed
    }

    /// Ask the gateway again. Harmless without a reader.
    pub fn refresh(&mut self) {
        let Some(workspace) = self.workspace else {
            return;
        };
        self.seq = self.seq.wrapping_add(1);
        if let Some(reader) = &self.reader_link
            && reader.refresh(self.seq, workspace)
            && self.load != Load::Ready
        {
            // A refresh over an answer that is already showing is not a
            // reason to blank it: what is on screen stays until the newer
            // answer replaces it.
            self.load = Load::Loading;
        }
    }

    /// Take in whatever the reader has answered. `true` when the column
    /// changed and a repaint is owed.
    ///
    /// **The only place an answer is accepted**, and it is called from the
    /// host's wake path — never from a timer, because an idle column does
    /// no timer work.
    pub fn poll(&mut self) -> bool {
        let Some(reader) = &self.reader_link else {
            return false;
        };
        let answers = reader.drain();
        if answers.is_empty() {
            return false;
        }
        let mut refresh_after = false;
        for answer in answers {
            match answer {
                reader::Answer::Catalog { seq, result } => {
                    // An older in-flight answer can never replace newer
                    // visible state.
                    if seq != self.seq {
                        continue;
                    }
                    self.take_catalog(result);
                }
                reader::Answer::Provenance { version, result } => {
                    if let Ok(provenance) = result {
                        self.provenances.insert(version, provenance);
                    }
                }
                // The column asks for neither: a version's bytes and the
                // next version of a family are the tab's business, on the
                // tab's own link (ADR-369).
                reader::Answer::Content { .. } | reader::Answer::Saved { .. } => {}
                reader::Answer::Registered { run, path, result } => {
                    match result {
                        Ok(_) => {
                            self.registering.remove(&(run, path));
                            // The file is a result now, and the two
                            // sections have to agree about that.
                            refresh_after = true;
                        }
                        Err(failure) => {
                            self.registering
                                .insert((run, path), Registering::Refused(failure.code));
                        }
                    }
                }
            }
        }
        if refresh_after {
            self.refresh();
        }
        true
    }

    /// Accept an answer the host fetched itself — the seam a host without
    /// this crate's reader drives, and the one every test uses.
    pub fn take_catalog(&mut self, result: Result<Catalog, koru_artifact_wire::Failure>) {
        match result {
            Ok(catalog) => {
                self.catalog = catalog;
                self.load = Load::Ready;
                self.prune();
            }
            Err(failure) if text::unavailable(&failure.code) => {
                // **Not an empty column.** What is on screen stays; only
                // the state changes, so a person reads *the store is not
                // answering* rather than *you have nothing*.
                self.load = Load::Unavailable;
            }
            Err(_) => self.load = Load::Ready,
        }
    }

    /// Accept a provenance walk the host fetched itself.
    pub fn take_provenance(
        &mut self,
        version: ArtifactVersionId,
        walk: koru_artifact_wire::Provenance,
    ) {
        self.provenances.insert(version, walk);
    }

    /// Accept a Register's answer the host sent itself.
    pub fn take_registered(
        &mut self,
        run: RunId,
        path: RelativePath,
        result: Result<(), koru_artifact_wire::Failure>,
    ) {
        match result {
            Ok(()) => {
                self.registering.remove(&(run, path));
                self.refresh();
            }
            Err(failure) => {
                self.registering
                    .insert((run, path), Registering::Refused(failure.code));
            }
        }
    }

    /// Drop what a fresh catalog no longer holds. A fold, a selection or a
    /// refusal that names something the store has forgotten is a row that
    /// will never be drawn again, and holding it would leak one id per
    /// refresh for the life of the window.
    fn prune(&mut self) {
        let families: HashSet<ArtifactFamilyId> = self
            .catalog
            .families
            .iter()
            .map(|entry| entry.family.id)
            .collect();
        self.expanded.retain(|id| families.contains(id));
        self.shown.retain(|id, _| families.contains(id));
        let versions: HashSet<ArtifactVersionId> = self
            .catalog
            .families
            .iter()
            .flat_map(|entry| entry.versions.iter().map(|version| version.id))
            .collect();
        if self.selected.is_some_and(|id| !versions.contains(&id)) {
            self.selected = None;
        }
        let live: HashSet<(RunId, RelativePath)> = self
            .catalog
            .runs
            .iter()
            .flat_map(|run| {
                run.files
                    .iter()
                    .map(|file| (run.run, file.path.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        self.registering.retain(|key, _| live.contains(key));
        // The rows have moved under the keyboard; an index into them means
        // something else now.
        self.hover = None;
        if self.focus.is_some() {
            self.focus = Some(Row::ResultsHeading);
        }
    }

    // ------------------------------------------------------- what it does

    /// Show or hide a family's versions.
    pub fn toggle_family(&mut self, id: ArtifactFamilyId) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        // Opening one starts from the newest page again, whatever was
        // revealed last time: the reason to open a family is the newest
        // version, and a family reopened three pages down hides it.
        self.shown.remove(&id);
    }

    pub fn is_expanded(&self, id: ArtifactFamilyId) -> bool {
        self.expanded.contains(&id)
    }

    /// How many of a family's versions are rows.
    pub fn shown(&self, id: ArtifactFamilyId) -> usize {
        self.shown.get(&id).copied().unwrap_or(VERSIONS_SHOWN)
    }

    /// Reveal one more page of a family's history.
    pub fn reveal_more(&mut self, id: ArtifactFamilyId) {
        let shown = self.shown(id);
        self.shown.insert(id, shown.saturating_add(VERSIONS_SHOWN));
    }

    pub fn selected(&self) -> Option<ArtifactVersionId> {
        self.selected
    }

    /// **A version the person asked to read**, taken by the host that can
    /// open a tab on it. `None` most of the time: this is set by one press.
    pub fn take_wants_open(&mut self) -> Option<OpenedArtifact> {
        self.wants_open.take()
    }

    /// Ask the host to open a version as a tab.
    ///
    /// Nothing is read here and nothing on the column changes: the list
    /// stays exactly as the person left it, and the document opens where
    /// every other document in Koru opens (ADR-369). A version whose bytes
    /// are elsewhere is named from what the catalog already says (D27), so
    /// the tab never has to ask a gateway a question it would refuse.
    pub(crate) fn ask_to_open(&mut self, family: usize, version: usize) {
        let Some(entry) = self.catalog.families.get(family) else {
            return;
        };
        let Some(found) = entry.version(version) else {
            return;
        };
        self.wants_open = Some(OpenedArtifact {
            version: found.id,
            title: entry.family.title.as_str().to_owned(),
            size_bytes: found.size_bytes,
            produced_by: found.produced_by.map(|by| by.run),
            held_elsewhere: self.holder(found).map(|device| device.map(str::to_owned)),
        });
    }

    /// Select a version — which is what shows its provenance sentence, and
    /// what asks for the walk that completes it.
    pub fn select_version(&mut self, version: ArtifactVersionId) {
        if self.selected == Some(version) {
            self.selected = None;
            return;
        }
        self.selected = Some(version);
        if !self.provenances.holds(version)
            && let Some(reader) = &self.reader_link
        {
            reader.act(reader::Act::Provenance { version });
        }
    }

    /// **Show one version and put the keyboard on it.**
    ///
    /// What *view in Artifacts* means when a host already knows which
    /// version it is looking at: the family is opened, enough of its
    /// history is shown for the version to be a row at all, its provenance
    /// sentence is asked for, the row is scrolled into the band and the
    /// arrows land on it.
    ///
    /// **Not [`select_version`](Self::select_version), which toggles.**
    /// Revealing is not a toggle: asking twice for the same version must
    /// not close what the first ask opened.
    ///
    /// `false` when this catalog holds no such version — it has not
    /// answered yet, or the column is about another workspace — so a
    /// caller can say something truthful rather than select nothing.
    pub fn reveal_version(
        &mut self,
        version: ArtifactVersionId,
        area: koru_draw::geometry::Rect,
        top: f32,
    ) -> bool {
        let Some((family, index)) = self.locate_version(version) else {
            return false;
        };
        let Some(id) = self.family_id(family) else {
            return false;
        };
        // The newest version is the family row itself. Older versions live
        // under its disclosure caret; revealing the primary result must not
        // create a duplicate version row underneath it.
        if index > 0 {
            self.expanded.insert(id);
        }
        // Enough of the history to reach it. A version older than the first
        // page sits behind *N earlier versions*, and a row that is not there
        // cannot be selected, scrolled to or focused.
        if index >= self.shown(id) {
            let pages = index / VERSIONS_SHOWN + 1;
            self.shown.insert(id, pages * VERSIONS_SHOWN);
        }
        if self.selected != Some(version) {
            self.selected = Some(version);
            if !self.provenances.holds(version)
                && let Some(reader) = &self.reader_link
            {
                reader.act(reader::Act::Provenance { version });
            }
        }
        let row = if index == 0 {
            Row::Family(family)
        } else {
            Row::Version {
                family,
                version: index,
            }
        };
        self.focus = Some(row);
        self.scroll_into_view(row, area, top);
        true
    }

    /// Where a version sits in the catalog: `(family, version)`, the two
    /// indices a [`Row::Version`] is.
    fn locate_version(&self, version: ArtifactVersionId) -> Option<(usize, usize)> {
        self.catalog
            .families
            .iter()
            .enumerate()
            .find_map(|(family, entry)| {
                entry
                    .versions
                    .iter()
                    .position(|candidate| candidate.id == version)
                    .map(|index| (family, index))
            })
    }

    /// **D19's one click.** Send the file through the same register path
    /// rule the run's own tools obey.
    pub fn register(&mut self, run: RunId, path: RelativePath) {
        if matches!(
            self.registering.get(&(run, path.clone())),
            Some(Registering::Sending)
        ) {
            return;
        }
        self.registering
            .insert((run, path.clone()), Registering::Sending);
        if let Some(reader) = &self.reader_link {
            reader.act(reader::Act::Register { run, path });
        }
    }

    /// How far a Register has got, if one has been pressed.
    pub fn registering(&self, run: RunId, path: &RelativePath) -> Option<&Registering> {
        self.registering.get(&(run, path.clone()))
    }

    /// The device that holds a version's content, when this one does not.
    ///
    /// `None` — the version is here. `Some(None)` — elsewhere, and no
    /// device can be named. `Some(Some(name))` — D27 in full.
    pub(crate) fn holder(
        &self,
        version: &koru_artifact_wire::ArtifactVersion,
    ) -> Option<Option<&str>> {
        if self.catalog.holds(version) {
            return None;
        }
        Some(
            version
                .produced_by
                .and_then(|by| self.devices.get(&by.run))
                .map(String::as_str),
        )
    }
}

#[cfg(test)]
mod tests;
