// SPDX-License-Identifier: MPL-2.0

//! **"Is my work saved?" — answered when asked, and said out loud only when
//! the answer became news.**
//!
//! One sentence, and what it excludes: *this turns the Store facts the shell
//! was handed into a status a screen reader can navigate to, and into an
//! announcement it makes once when saving fails or recovers.* It excludes the
//! facts themselves, which are
//! [`koru_workspace::durability`]'s; it excludes the offline chip's words,
//! which are `koru_i18n::store`'s and are already drawn beside the tab strip;
//! and it excludes anything that could cause a save — nothing here writes,
//! publishes, buffers or asks the Store a question.
//!
//! The decision, with the alternatives that were rejected, is
//! **ADR-340-savestatus** in
//! [docs/decisions/](../../../../../docs/decisions/README.md). Cited by slug
//! rather than by number: a parallel branch may claim 339 first and the
//! delivery lead renumbers at merge.
//!
//! ## The two jobs, and why they are two nodes
//!
//! A screen reader has to do two different things here, and one mechanism
//! cannot do both.
//!
//! - **Answer on demand.** `#document` is a plain [`Role::Label`] carrying this
//!   document's current status. It is silent — a person navigates to it when
//!   they want to know. It changes on every keystroke, because *pending* and
//!   *saved* is exactly what it is reporting.
//! - **Say it once.** `#news` is a [`Role::Alert`] marked
//!   [`Live::Polite`](accesskit::Live), and a live region is spoken **whenever
//!   its name changes** — `accesskit_windows` raises
//!   `UIA_LiveRegionChangedEventId` on exactly that, and on nothing else (see
//!   `guide/contributing/deps/ACCESSKIT.md`). So its label carries news and never status: it
//!   moves when saving starts failing, when it starts failing differently, and
//!   when it recovers, and it does not move at all while the ordinary
//!   type-publish-acknowledge cycle turns over.
//!
//! Putting the status on the live node is the one-line version of the bug §5
//! forbids: a screen reader that says "saving" after every letter is a screen
//! reader people learn to ignore, and the one line that mattered goes with it.
//!
//! ## What makes something news
//!
//! [`Situation`] is the whole of it, and it is deliberately coarse. Nothing
//! about *this* document: the news is about whether Koru is saving at all, so
//! it is the worst answer across every open document and the workspace's own
//! list, which is the same shape [`Workspace::store_trouble`] takes for the
//! chip.
//!
//! Two rules keep it from becoming activity:
//!
//! - **A blip is not news.** A Store restarting looks exactly like a link that
//!   is retrying — `LinkState::Reconnecting`'s own note says the *duration* is
//!   what decides whether it is worth telling anybody — so a retry says nothing
//!   until it has outlasted [`RETRY_PATIENCE`].
//! - **The first observation is not a transition.** A client attaching is
//!   handed the current status through `#document`; it is not told the state it
//!   arrived in as though it had just happened. News begins at the first
//!   *change* after that.
//!
//! ## Read-only is not a failed save
//!
//! [`Status::of`] asks about read-only before it asks about durability, because
//! the two produce the same Store bookkeeping and mean opposite things. A
//! document written by a newer build has work outstanding that will never be
//! sent — `Compatibility` refuses the write, which is
//! [AGENTS.md](../../../../../AGENTS.md)'s *an older build never writes a newer
//! document* — and describing that as "not saved" sends somebody looking for a
//! failure that is really a deliberate refusal.
//!
//! ## Storage, not sync
//!
//! Every sentence published here is about this computer's storage. A durable
//! acknowledgement means the bytes are on **this machine's** disk
//! ([ADR-79](../../../../../docs/decisions/ADR-076-100.md)); whether another
//! device has seen the edit is `koru-replication`'s question, with its own
//! failure modes, and no word here implies an answer to it. §5's *keep local
//! durability separate from peer synchronization* is met by this module saying
//! nothing at all about peers.
//!
//! ## Two of these have no pixels, and that is stated rather than faked
//!
//! `#chip` is a real control drawn in the caption and carries the rectangle the
//! press is already resolved against. `#document`, `#storage` and `#news` are
//! text, and Koru draws no per-document save status anywhere — the chip speaks
//! for the workspace and only while the Store is away. They therefore publish
//! **no bounds**, which AccessKit serves as an empty rectangle and documents as
//! legal. Borrowing the tab's or the chip's rectangle would satisfy the shell's
//! *every control has somewhere a pointer can land* rule by publishing a
//! control somewhere other than where it is, which is the rule underneath that
//! one. The asymmetry — a screen reader can ask a question a sighted person
//! cannot — is recorded in `docs/features/accessibility/README.md` and
//! `docs/product/PRODUCT.md` rather than papered over.
//!
//! ## Nothing here moves the caret
//!
//! [`MainView::accessibility_focus`] never returns one of these occurrences.
//! The status changing, and the announcement being spoken, both leave the
//! keyboard exactly where it was — which is §2 step 6's *accessible without
//! displacing editor focus*, held as a property of where focus is read from
//! rather than as a promise.
//!
//! [`Workspace::store_trouble`]: koru_workspace::Workspace::store_trouble

use accesskit::{Action, Live, Role};
use koru_core::DocumentId;
use koru_i18n::{Key, t};
use koru_workspace::durability::{DocumentDurability, Durability, LinkHealth};
use koru_workspace_host::accessibility::{Occurrence, Revision, SemanticNode};

use super::{Shell, parts, surface_of};
use crate::mainview::MainView;

/// The save-status surface. A locator then reads `.../s:save#document`, and a
/// person can see which layer the control came from.
const SAVE: &str = "save";

/// The parts of it.
mod part {
    use koru_workspace_host::accessibility::ControlPart;

    // The group's own part is the shell's shared `parts::ROOT` — "the whole of
    // the object", the same keyword every other surface uses for itself.
    /// This document's own answer. Silent; read when asked.
    pub(super) const DOCUMENT: ControlPart = ControlPart::new("document");
    /// The workspace-wide sentence the offline chip's popover already draws.
    pub(super) const STORAGE: ControlPart = ControlPart::new("storage");
    /// The chip itself — the one existing control, and the way to the detail a
    /// sighted person clicks for.
    pub(super) const CHIP: ControlPart = ControlPart::new("chip");
    /// The live region. See the module docs for why it is not the status.
    pub(super) const NEWS: ControlPart = ControlPart::new("news");
}

/// How long a link may be retrying before it is worth saying out loud.
///
/// **A Store restarting is indistinguishable from one that has gone**, from
/// here — `LinkState::Reconnecting`'s own documentation says so and says the
/// duration is what decides. Ten seconds is far longer than a restart and far
/// shorter than a person's patience with silence; `koru_workspace`'s watchdog
/// wakes the window every second while a link is unhappy, so the threshold is
/// reached without anything else having to happen.
///
/// It gates the **announcement** only. The status answers truthfully from the
/// first moment, because a person who asks is asking about now.
const RETRY_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// What the shell was last told, and what it has already said about it.
///
/// **Owned by [`MainView`] rather than computed in the semantic pass**, because
/// an announcement is a claim about a *change* and `View::semantics` is handed
/// `&self`. The change is noticed where `&mut self` and the facts are both in
/// hand — `View::set_durability`, on the loop thread — and the semantic pass
/// then reads a value that is already decided.
#[derive(Debug, Default)]
pub(crate) struct Persistence {
    /// The last facts pushed. Empty until a screen reader attaches, because
    /// that is when the shell starts being told.
    facts: Durability,
    /// What was said last, kept until different news replaces it.
    ///
    /// **Kept rather than cleared**, and the reason is the adapter's: a live
    /// region's event carries a node the client reads *afterwards*, so an
    /// announcement withdrawn on the next pass can be gone before it is read.
    /// Left standing, it is also still true — "everything is saved again"
    /// remains the last thing that happened until something else does.
    news: Option<Key>,
    /// The situation as of the last observation, so the next one can see a
    /// transition. `None` before the first, which is what makes the state a
    /// client attaches into current status rather than news.
    seen: Option<Situation>,
    /// Durability observations taken. A work counter (§7.3) rather than a
    /// timing limit: it must stay flat while nothing about saving changes, and
    /// it must be zero in a session with no screen reader attached.
    observations: u64,
}

impl Persistence {
    /// Take in what the shell was told, and decide whether any of it is news.
    pub(crate) fn observe(&mut self, facts: &Durability) {
        self.observations += 1;
        // Clone rather than borrow: the facts are read again in the semantic
        // pass, which holds `&self` and cannot reach the workspace. Every
        // `String` in them is `None` on the healthy path, so the ordinary cost
        // is a `Vec` of small values.
        if self.facts != *facts {
            self.facts = facts.clone();
        }
        // **Nothing open is not a situation.** A workspace with no Store
        // sessions — a folder-backed one, or the moment between leaving one
        // workspace and arriving in another — has nothing to be safe or unsafe
        // *about*. Treating it as quiet would announce "everything is saved
        // again" for a workspace that was switched away from while work was at
        // risk, which is the most confidently wrong thing this module could
        // say. The last observation is left standing instead, so arriving back
        // in the same state announces nothing either.
        let Some(now) = Situation::of(facts, std::time::Instant::now()) else {
            return;
        };
        match self.seen.replace(now) {
            // **The first observation seeds, and says nothing.** A person who
            // starts a screen reader while the Store is away is told by
            // `#document` when they ask; they are not told, as though it had
            // just happened, about a state that was already true.
            None => self.note(None, now),
            Some(before) if before == now => {}
            Some(before) => {
                self.news = Some(match now {
                    Situation::Quiet => Key::A11ySaveNewsRecovered,
                    Situation::KeptHere => Key::A11ySaveNewsBuffered,
                    Situation::Stopped => Key::A11ySaveNewsStopped,
                    Situation::AtRisk => Key::A11ySaveNewsAtRisk,
                });
                self.note(Some(before), now);
            }
        }
    }

    /// Durability observations taken since this view was built.
    ///
    /// **A work counter, not a stopwatch** (§7.3). It says how many times the
    /// Store's bookkeeping was read, which must be zero in a session with no
    /// screen reader attached and must not grow with semantic queries, frames
    /// or caret blinks. Read by the tests and by
    /// [`note`](Self::note)'s diagnostic line.
    pub(crate) fn observations(&self) -> u64 {
        self.observations
    }

    /// One line about a transition, behind `KORU_ACCESSIBILITY_DEBUG`.
    ///
    /// **News, not activity** (§7.4): only the first observation and an actual
    /// change write anything, so a window sitting in one state writes nothing
    /// however long it sits there. And **never a sentence**: the situation's
    /// own keyword and a count, so nothing a person wrote — and nothing a
    /// translator wrote either — reaches an operational log.
    fn note(&self, before: Option<Situation>, now: Situation) {
        if !koru_env::switch(koru_env::name::KORU_ACCESSIBILITY_DEBUG) {
            return;
        }
        eprintln!(
            "[koru:a11y] save from={} to={} documents={} observations={}",
            before.map_or("-", Situation::keyword),
            now.keyword(),
            self.facts.documents.len(),
            self.observations(),
        );
    }

    fn status(&self, focused: Option<DocumentId>) -> Option<Status> {
        self.facts.document(focused?).map(Status::of)
    }
}

/// Whether Koru is saving at all, as coarsely as it can be put.
///
/// Ordered worst-last, and [`AtRisk`](Self::AtRisk) is last rather than
/// [`Stopped`](Self::Stopped): a stopped link means nothing *more* can be
/// saved, and work sitting on no disk at all means something already typed
/// would go. Losing content is the one unrecoverable bug, so it outranks.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Situation {
    /// Nothing anybody needs telling about — including the ordinary half
    /// second between typing and an acknowledgement, and a link that has been
    /// retrying for less than [`RETRY_PATIENCE`].
    Quiet,
    /// Work the Store has not confirmed, written down on this machine.
    KeptHere,
    /// A session that will not come back without somebody doing something.
    Stopped,
    /// Work that is in this process and nowhere else.
    AtRisk,
}

impl Situation {
    /// The code's own word for it, for a diagnostic line. Untranslated and not
    /// content — the sentence a person hears never enters a log.
    fn keyword(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::KeptHere => "kept-here",
            Self::Stopped => "stopped",
            Self::AtRisk => "at-risk",
        }
    }

    /// The worst answer across every open document **and** the workspace's own
    /// list. `None` when there is neither — see [`Persistence::observe`].
    ///
    /// Aggregated here and only here: one document failing to save is Koru
    /// failing to save as far as an announcement is concerned, which is the
    /// same judgement `store_trouble` makes for the chip. The *status* is not
    /// aggregated at all — see [`Persistence::status`].
    fn of(facts: &Durability, now: std::time::Instant) -> Option<Self> {
        facts
            .documents
            .iter()
            .map(|(_, durability)| durability)
            .chain(facts.document_list.as_ref())
            .map(|durability| Self::of_document(durability, now))
            .max()
    }

    fn of_document(durability: &DocumentDurability, now: std::time::Instant) -> Self {
        let owed = !durability.durable;
        // A reported failure to write locally is news at once: it is the one
        // state where closing the window loses something, and no amount of
        // patience makes it less true.
        if owed && durability.buffer_failure.is_some() {
            return Self::AtRisk;
        }
        if durability.stopped.is_some() || durability.link == LinkHealth::Stopped {
            return Self::Stopped;
        }
        let LinkHealth::Retrying { since } = durability.link else {
            // Live, and either confirmed or on its way. Both are activity.
            return Self::Quiet;
        };
        if !owed || now.saturating_duration_since(since) < RETRY_PATIENCE {
            return Self::Quiet;
        }
        // Past patience with work outstanding. `buffer_if_due` writes at most
        // once a second while a link is not live, so by now either a copy
        // exists or there is nothing standing between this work and a power
        // cut.
        if durability.buffered {
            Self::KeptHere
        } else {
            Self::AtRisk
        }
    }
}

/// One document's answer to the question, as a sentence a person hears.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Status {
    /// Nothing will be written, and nothing is wrong. Asked first; see the
    /// module docs.
    ReadOnly,
    /// The Store has confirmed everything this replica holds.
    Saved,
    /// Handed over, not yet answered, link getting through.
    Saving,
    /// Owed, and on this machine's disk.
    KeptHere,
    /// Owed, and nowhere.
    AtRisk,
    /// This document's session has given up. **Says nothing about whether the
    /// last edit landed**, because it may well have: what it says is that
    /// nothing more will, which is true either way.
    Stopped,
}

impl Status {
    fn of(durability: &DocumentDurability) -> Self {
        if durability.read_only.is_some() {
            return Self::ReadOnly;
        }
        // **Ahead of `durable`, and its sentence is worded so that is honest.**
        // A session that has given up may well have acknowledged everything
        // typed so far, so "not saved" would be a lie — and "saved" would be a
        // worse one, because nothing typed from here on can be. The row says
        // what is true in both cases: Koru has stopped saving this document,
        // and will not recover on its own.
        if durability.stopped.is_some() || durability.link == LinkHealth::Stopped {
            return Self::Stopped;
        }
        if durability.durable {
            return Self::Saved;
        }
        if durability.buffer_failure.is_some() {
            return Self::AtRisk;
        }
        if durability.buffered {
            return Self::KeptHere;
        }
        if durability.link.is_live() {
            return Self::Saving;
        }
        // Owed, the link is not getting through, and nothing on this machine
        // covers it yet. True for at most the second before `buffer_if_due`
        // comes round, and true is what a person asking deserves.
        Self::AtRisk
    }

    fn key(self) -> Key {
        match self {
            Self::ReadOnly => Key::A11ySaveReadOnly,
            Self::Saved => Key::A11ySaveSaved,
            Self::Saving => Key::A11ySaveSaving,
            Self::KeptHere => Key::A11ySaveBuffered,
            Self::AtRisk => Key::A11ySaveAtRisk,
            Self::Stopped => Key::A11ySaveStopped,
        }
    }

    /// A byte for the revision fingerprint. Written out rather than cast from
    /// the discriminant so reordering the enum cannot silently renumber it.
    fn tag(self) -> u8 {
        match self {
            Self::ReadOnly => 1,
            Self::Saved => 2,
            Self::Saving => 3,
            Self::KeptHere => 4,
            Self::AtRisk => 5,
            Self::Stopped => 6,
        }
    }
}

impl MainView {
    /// Which document the person is looking at, if they are looking at one.
    ///
    /// **The tab's own record, not its body's root.** `Tab::document` is what
    /// the strip resolves a tab against; a rebuilt body can report a different
    /// document's node, which is the bug that comment on the field describes.
    /// `None` — a welcome screen, the Trash, Settings, a draft with no document
    /// yet — means there is no document whose save status to report, and this
    /// module publishes none rather than answering about a neighbour.
    fn persistence_document(&self) -> Option<DocumentId> {
        self.tabs().active_tab().document
    }

    /// The save-status group, when there is anything in it.
    pub(super) fn persistence_semantics(&self, cx: &Shell) -> Option<SemanticNode> {
        let mut children = Vec::new();

        if let Some(status) = self.save_status.status(self.persistence_document()) {
            children.push(
                SemanticNode::new(cx.surface(SAVE, part::DOCUMENT), Role::Label)
                    .with_label(t(cx.lang, status.key())),
            );
        }

        // **The chip's own sentence, not a second one.** `store_detail` is the
        // map `draw_offline_chip` uses, `koru_i18n::store` owns every branch of
        // it, and a popped-out document draws the same words in its own
        // caption. A third assembly here is how two surfaces start describing
        // one Store two ways.
        if let Some(trouble) = self.store_trouble.as_ref() {
            children.push(
                SemanticNode::new(cx.surface(SAVE, part::STORAGE), Role::Label)
                    .with_label(crate::mainview::draw::store_detail(cx.lang, trouble)),
            );
            // The one existing control: the chip a pointer presses to open the
            // detail popover. Published where it is drawn, with the rectangle
            // the press is already resolved against.
            if let Some(chip) = self.offline_chip() {
                children.push(
                    SemanticNode::new(cx.surface(SAVE, part::CHIP), Role::Button)
                        .with_label(t(cx.lang, Key::StoreOfflineTitle))
                        .with_bounds(chip)
                        // No `expanded` for the popover: it would cost the
                        // button its Invoke pattern and `accesskit_windows`
                        // 0.32.1 implements no ExpandCollapse pattern to
                        // replace it with. See `guide/contributing/deps/ACCESSKIT.md`.
                        .with_action(Action::Click),
                );
            }
        }

        if let Some(news) = self.save_status.news {
            children.push(
                SemanticNode::new(cx.surface(SAVE, part::NEWS), Role::Alert)
                    .with_label(t(cx.lang, news))
                    // **On the leaf, never on the group.** `live` is inherited
                    // by descendants, so marking the container would make the
                    // status above speak on every keystroke — the exact
                    // collapse this module exists to avoid.
                    .announcing(Live::Polite),
            );
        }

        // A named group with nothing in it is a dead end for somebody asking a
        // question, so there is no group unless there is an answer.
        (!children.is_empty()).then(|| {
            SemanticNode::new(cx.surface(SAVE, parts::ROOT), Role::Group)
                .with_label(t(cx.lang, Key::A11ySaveStatus))
                .with_children(children)
        })
    }

    /// Fold this surface's inputs into the shell's fingerprint.
    ///
    /// Everything that changes a published label, and nothing that moves on its
    /// own: not `StoreTrouble::since`, which never stops advancing and would
    /// rebuild the tree on every watchdog tick forever.
    pub(super) fn persistence_revision(&self, rev: &mut Revision) {
        let focused = self.persistence_document();
        rev.opt_uuid(focused.map(|id| id.as_uuid()));
        rev.byte(
            self.save_status
                .status(focused)
                .map_or(0, |status| status.tag()),
        );
        // The key's own ordinal, plus one so *no news* is distinguishable from
        // the first row in the table. `Key` has hundreds of variants, so this
        // is a `u64` and not a byte that would wrap two of them together.
        rev.u64(self.save_status.news.map_or(0, |news| news as u64 + 1));
        match self.store_trouble.as_ref() {
            // The same projection `set_store_trouble` compares on, for the same
            // reason, plus the message because it is published verbatim.
            Some(trouble) => {
                rev.byte(1);
                rev.u64(trouble.owed as u64);
                rev.bool(trouble.fatal);
                rev.u64(trouble.unconfirmed.documents as u64);
                rev.bool(trouble.unconfirmed.document_list);
                rev.bool(trouble.unconfirmed.not_buffered.is_some());
                rev.str(&trouble.message);
            }
            None => {
                rev.byte(0);
            }
        }
        rev.bool(self.offline_chip().is_some());
    }
}

/// Route a request at the chip.
///
/// The only control here — the labels answer questions and perform nothing —
/// and what it does is what a press on it does: open the detail, or shut it.
pub(super) fn action(view: &mut MainView, target: Occurrence, action: Action) -> bool {
    if surface_of(target) != Some(SAVE) || action != Action::Click {
        return false;
    }
    if target.part != part::CHIP || view.offline_chip().is_none() {
        return false;
    }
    // `mouse_down`'s chip branch is `self.offline_open = on_chip && !was`, and
    // a request that named this node *is* `on_chip`. So this is that expression
    // with its known half substituted, rather than a second opinion about what
    // pressing the chip means.
    view.offline_open = !view.offline_open;
    true
}

#[cfg(test)]
mod tests;
