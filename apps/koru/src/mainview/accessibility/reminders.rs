// SPDX-License-Identifier: MPL-2.0

//! **Setting a reminder, correcting it, and coming back to hear what was
//! actually stored.**
//!
//! One sentence, and what it excludes: *this publishes the reminder popover as
//! a surface a person can work through with the keyboard, publishes the
//! schedules the Store gave back as something they can ask about afterwards,
//! and says once when a save fails or lands.* It excludes the reminder model
//! and every Store call, which are [`crate::mainview::reminders::Controller`]'s;
//! it excludes the popover's geometry, focus and words, which are
//! `mainview/reminder_creation.rs`'s and are read from it rather than
//! recomputed; and it excludes the bell, the attention popover and the global
//! All-Reminders overlay, which are `reminder_shell.rs`'s surface and another
//! plan's.
//!
//! The decision, with the alternatives that were rejected, is
//! **ADR-345-accessiblereminders** in
//! [docs/decisions/](../../../../../docs/decisions/README.md). Cited by slug
//! rather than by number: a parallel branch may claim 345 first and the
//! delivery lead renumbers at merge.
//!
//! ## The trap this module exists to avoid
//!
//! Pressing *Set reminder* closes the popover and shows an inline pill while
//! the save is still in flight. A test — or a person — that asserts the input
//! it just typed, or reads the pill that just appeared, has proved nothing:
//! [`OptimisticState::Pending`] is what that pill is made of, and
//! `MainView::push_reminder_chip` says so in as many words. **An input that has
//! disappeared is not the saved schedule.**
//!
//! So the question *what is this item's reminder?* is answered from
//! [`Controller::global`]'s rows — the Store's own snapshot — and a reminder
//! that has been submitted and not yet acknowledged is published as exactly
//! that: a schedule the Store has not confirmed. The two are told apart by
//! whether the projection has a row for the reminder's id, which is the same
//! test `push_reminder_chip` makes to decide whether an optimistic record is
//! still the inline source of truth.
//!
//! And the *revisit* route inherits the property rather than restating it:
//! `MainView::open_reminders_here` reopens through `open_reminder_edit`, which
//! seeds the editor from those same authoritative rows and refuses when there
//! is none. An optimistic pill cannot be reopened, so it cannot be mistaken
//! for the schedule.
//!
//! ## Two nodes, not one — and it is the same two ADR-340 built
//!
//! - `s:remind#root` is a navigable [`Role::Group`] holding one silent
//!   [`Role::Label`] per saved reminder. It is the answer on demand, it moves
//!   whenever the Store's answer moves, and it says nothing out loud.
//! - `s:remind#news` is a [`Role::Alert`] carrying **only news**: a save
//!   landed, or a save failed. `accesskit_windows` raises
//!   `UIA_LiveRegionChangedEventId` when a live node's *name* changes and on
//!   nothing else (`guide/contributing/deps/ACCESSKIT.md`), so a node whose label carried
//!   the status would speak on every keystroke.
//!
//! There is no third mechanism, and `live` goes on the leaf that speaks and
//! never on the group around it, because `accesskit_consumer` inherits it.
//!
//! The popover's own running commentary — *When should Koru remind you?*,
//! *Scheduled for …*, *Saving reminder…*, *This reminder is not valid* — is a
//! **status** and is published as `#message`, silent, exactly like
//! `s:save#document`. It changes on every keystroke, which is precisely why it
//! may not be the thing that speaks.
//!
//! ## Identity
//!
//! Three kinds of object, and each is the only honest answer for its case:
//!
//! - **The popover is an instance.** `remind:<n>#set` — a number
//!   `Surface::instance` mints once per opening and never reuses. Close the
//!   panel while a client holds one of its controls and the id resolves to
//!   nothing, which is the refusal §5 asks for; reopen and the successor holds
//!   a different number, so it cannot inherit the request either.
//! - **A saved reminder is its stored id.** `n:<reminder uuid>#schedule` and
//!   `n:<reminder uuid>#edit`. Not a position — the rows are asynchronous and
//!   reorder — and not the phrase somebody typed. This is what makes *the same
//!   reminder, revisited* a move rather than a coincidence.
//! - **A failed submission is its request token.** `n:<token uuid>#retry` —
//!   the same identity `Controller::failed_creations` files it under, so
//!   Retry, Edit and Dismiss reach the one that failed even when three are
//!   standing.
//!
//! ## Two kinds of node publish no rectangle, for two different reasons
//!
//! Every **control** carries the rectangle drawing and hit-testing already
//! share, straight out of `Surface::controls` — a control cannot be published
//! somewhere other than where a press finds it. Two things here are not
//! controls, and neither borrows a neighbour's rectangle to look like one:
//!
//! - **The saved schedules** have no drawn twin of their own. The pill a
//!   person sees is drawn after its paragraph by `koru_editor::draw`, and it
//!   is a *different* thing from this sentence: this is the answer to a
//!   question, and it says whether storage has confirmed the schedule, which
//!   the pill does not. The asymmetry — a screen reader can ask something a
//!   pointer cannot — is the one ADR-340 recorded for the save status.
//! - **`#message`** *is* drawn, and its rectangle is not in the shared
//!   geometry: `draw_creation` produces it by laying the sentence out with the
//!   text engine, per frame, at the panel's width. Publishing it would mean
//!   measuring text inside a `&self` semantic pass, which is the one thing §7.3
//!   forbids on this path. It is text rather than a control, so an empty
//!   rectangle costs a reader nothing.
//!
//! AccessKit serves an absent rectangle as an empty one and documents that as
//! legal. What *is* a gap, and is written down in
//! `docs/features/accessibility/README.md` rather than papered over: **the inline
//! pill itself is not yet a control in the document's reading order.**
//!
//! ## An action is the press that already exists
//!
//! [`action`] resolves a request to a [`Hit`] and hands it to
//! `Surface::invoke`, which applies the same two gates `Surface::key` applies —
//! enabled, and in the Tab order — and then calls the same `activate` a click
//! and Enter both call. Nothing here is a command path of its own, and an
//! advertised action set is a hint rather than a filter, so the refusal lives
//! in the owner rather than in what the node offered.
//!
//! [`OptimisticState::Pending`]: koru_editor::reminders::OptimisticState::Pending
//! [`Controller::global`]: crate::mainview::reminders::Controller::global

use accesskit::{Action, Live, Role};
use koru_editor::reminders::{
    Draft, Failure, GlobalItem, GlobalItemState, OptimisticCreation, OptimisticState,
};
use koru_editor::utils::when::{DueRelation, ReminderDraft, ReminderMoment};
use koru_i18n::{Key, t};
use koru_workspace_host::accessibility::{
    ControlPart, ObjectKey, Occurrence, Revision, SemanticNode,
};

use super::{Shell, instance_of, parts, surface_of};
use crate::mainview::MainView;
use crate::mainview::reminder_creation::{FailedAction, Hit, Preset, RowAction};

/// The reminder surface. A locator then reads `.../s:remind#root`, and a person
/// can see which layer the control came from.
pub(crate) const REMIND: &str = "remind";

/// One opening of the reminder popover. See `Surface::instance`.
const POPOVER: &str = "remind";

/// The parts of it.
mod part {
    use koru_workspace_host::accessibility::ControlPart;

    // The group's own part is the shell's shared `parts::ROOT`.
    /// One saved reminder's schedule, as the Store answered it. Silent.
    pub(super) const SCHEDULE: ControlPart = ControlPart::new("schedule");
    /// The live region. See the module docs for why it is not the status.
    pub(super) const NEWS: ControlPart = ControlPart::new("news");
    /// The popover's running commentary — preview, validation, progress.
    /// Silent, for the same reason `s:save#document` is.
    pub(super) const MESSAGE: ControlPart = ControlPart::new("message");
    /// The natural-language field, and the two structured ones.
    pub(super) const PHRASE: ControlPart = ControlPart::new("phrase");
    pub(super) const DATE: ControlPart = ControlPart::new("date");
    pub(super) const TIME: ControlPart = ControlPart::new("time");
    /// The popover's own actions.
    pub(super) const MORE: ControlPart = ControlPart::new("more");
    pub(super) const END_OF_DAY: ControlPart = ControlPart::new("end-of-day");
    pub(super) const SET: ControlPart = ControlPart::new("set");
    pub(super) const CANCEL: ControlPart = ControlPart::new("cancel");
    pub(super) const LATER_TODAY: ControlPart = ControlPart::new("later-today");
    pub(super) const TOMORROW: ControlPart = ControlPart::new("tomorrow");
    pub(super) const CUSTOM: ControlPart = ControlPart::new("custom");
    pub(super) const ADD: ControlPart = ControlPart::new("add");
    pub(super) const HISTORY: ControlPart = ControlPart::new("history");
    pub(super) const CLEAR: ControlPart = ControlPart::new("clear");
    pub(super) const CLEAR_CONFIRM: ControlPart = ControlPart::new("clear-confirm");
    pub(super) const CLEAR_CANCEL: ControlPart = ControlPart::new("clear-cancel");
    /// The red line under the structured fields. Its **own** part rather than
    /// the retry control's: the two can stand at once, and two nodes sharing an
    /// occurrence share an AccessKit node id.
    pub(super) const INVALID: ControlPart = ControlPart::new("invalid");
    /// The recovery pair a failed load leaves in the panel.
    pub(super) const RETRY: ControlPart = ControlPart::new("retry");
    pub(super) const DISCARD: ControlPart = ControlPart::new("discard");
    /// A saved reminder's own three, on the reminder rather than on the panel.
    pub(super) const EDIT: ControlPart = ControlPart::new("edit");
    pub(super) const PAUSE: ControlPart = ControlPart::new("pause");
    pub(super) const DELETE: ControlPart = ControlPart::new("delete");
    pub(super) const RECREATE: ControlPart = ControlPart::new("recreate");
    /// One failed submission's recovery pill, scoped by its request token: the
    /// pill itself, then the three controls on it.
    pub(super) const PILL: ControlPart = ControlPart::new("pill");
    pub(super) const PILL_RETRY: ControlPart = ControlPart::new("pill-retry");
    pub(super) const PILL_EDIT: ControlPart = ControlPart::new("pill-edit");
    pub(super) const PILL_DISMISS: ControlPart = ControlPart::new("pill-dismiss");
}

/// Whether Koru has told this person anything about a reminder yet, and what.
///
/// **Owned by [`MainView`] rather than computed in the semantic pass**, for
/// ADR-340's reason: an announcement is a claim about a *change*, and
/// `View::semantics` is handed `&self`. The change is noticed where `&mut self`
/// and the facts are both in hand — the maintenance pass that already collects
/// the optimistic records — and the semantic pass then reads a value that is
/// already decided.
#[derive(Debug, Default)]
pub(crate) struct ReminderNews {
    /// What was said last, kept until different news replaces it.
    ///
    /// **Kept rather than cleared**, and the reason is the adapter's: a live
    /// region's event carries a node the client reads *afterwards*, so an
    /// announcement withdrawn on the next pass can be gone before it is read.
    news: Option<Key>,
    /// The worst thing observed about the submissions in flight, so the next
    /// observation can see a transition. `None` before the first, which is what
    /// makes the state a client attaches into current status rather than news.
    seen: Option<Submissions>,
    /// Observations taken. A work counter (§7.3) rather than a timing limit: it
    /// must stay flat while nothing about a submission changes.
    observations: u64,
}

impl ReminderNews {
    /// Take in the submissions maintenance just collected, and decide whether
    /// any of it is news.
    ///
    /// **Cheap because the list is already in hand.** Unlike a durability
    /// reading, which had to walk version vectors and is therefore taken only
    /// while a client is attached, this is a comparison over the `Vec` that
    /// `sync_reminder_creation_surface` builds every pass anyway. There is
    /// nothing to gate.
    pub(crate) fn observe(&mut self, submissions: &[OptimisticState]) {
        self.observations += 1;
        // **Nothing submitted is not a situation.** A person who has never set
        // a reminder, or whose last one has been retired into the authoritative
        // projection, has nothing to be told about — and treating that as
        // "saved" would announce a success every time a pill was cleaned up.
        // The last observation is left standing instead.
        let Some(now) = Submissions::of(submissions) else {
            return;
        };
        match self.seen.replace(now) {
            // **The first observation seeds, and says nothing.** A screen
            // reader attaching while a save is in flight is told by `#root`
            // when it asks; it is not told, as though it had just happened,
            // about a state that was already true.
            None => self.note(None, now),
            Some(before) if before == now => {}
            Some(before) => {
                // **Waiting is never news.** Arriving at *in flight* is the
                // ordinary half second after pressing `Set reminder`, or a
                // second reminder being submitted; a screen reader that said
                // "Saving reminder…" each time is one people learn to ignore,
                // taking the sentence that mattered with it. The transition is
                // still recorded, so the *next* one is seen.
                if let Some(key) = now.news_key() {
                    self.news = Some(key);
                }
                self.note(Some(before), now);
            }
        }
    }

    /// Observations taken since this view was built. A work counter, not a
    /// stopwatch (§7.3): it must not grow with semantic queries, frames or
    /// caret blinks, only with maintenance passes that had something to look
    /// at.
    #[cfg(test)]
    pub(crate) fn observations(&self) -> u64 {
        self.observations
    }

    /// One line about a transition, behind `KORU_ACCESSIBILITY_DEBUG`.
    ///
    /// **News, not activity** (§7.4): only the first observation and an actual
    /// change write anything. And **never a phrase, a schedule or a title** —
    /// the situation's own keyword and a count, so nothing the person wrote
    /// about their own life reaches an operational log.
    fn note(&self, before: Option<Submissions>, now: Submissions) {
        if !koru_env::switch(koru_env::name::KORU_ACCESSIBILITY_DEBUG) {
            return;
        }
        eprintln!(
            "[koru:a11y] reminder from={} to={} observations={}",
            before.map_or("-", Submissions::keyword),
            now.keyword(),
            self.observations,
        );
    }
}

/// What has become of the reminders this person has submitted, as coarsely as
/// it can be put.
///
/// Ordered worst-last: one submission failing is Koru failing to save a
/// reminder, which is the same judgement the save status makes about
/// documents, and it is the one a person is waiting on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Submissions {
    /// Handed over, not yet answered. Not news — it is the ordinary half
    /// second between pressing *Set reminder* and the Store agreeing.
    InFlight,
    /// Every submission still standing has been acknowledged.
    Landed,
    /// At least one was refused, and its Retry/Edit/Dismiss pill is standing.
    Failed(FailureKeyword),
}

/// Which refusal, kept as its own value so a *different* failure is news again
/// rather than being swallowed by the one already announced.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum FailureKeyword {
    Unavailable,
    InvalidData,
    Rejected,
}

impl Submissions {
    fn of(submissions: &[OptimisticState]) -> Option<Self> {
        submissions
            .iter()
            .map(|state| match state {
                OptimisticState::Pending => Self::InFlight,
                OptimisticState::Succeeded => Self::Landed,
                OptimisticState::Failed { reason } => Self::Failed(match reason {
                    Failure::Unavailable => FailureKeyword::Unavailable,
                    Failure::InvalidData => FailureKeyword::InvalidData,
                    Failure::Rejected => FailureKeyword::Rejected,
                }),
            })
            .max()
    }

    /// The sentence a person hears when this becomes true, or `None` when
    /// becoming it is not worth saying.
    ///
    /// The failures reuse the rows the popover already draws for the same
    /// refusal — *Reminders are unavailable*, *This reminder is not valid*,
    /// *The reminder was not saved* — so what is announced and what is read
    /// back are one vocabulary.
    fn news_key(self) -> Option<Key> {
        Some(match self {
            Self::InFlight => return None,
            Self::Landed => Key::A11yReminderNewsSaved,
            Self::Failed(FailureKeyword::Unavailable) => Key::ReminderErrorUnavailable,
            Self::Failed(FailureKeyword::InvalidData) => Key::ReminderErrorInvalidData,
            Self::Failed(FailureKeyword::Rejected) => Key::ReminderErrorRejected,
        })
    }

    /// The code's own word for it, for a diagnostic line. Untranslated, and
    /// never content.
    fn keyword(self) -> &'static str {
        match self {
            Self::InFlight => "in-flight",
            Self::Landed => "landed",
            Self::Failed(FailureKeyword::Unavailable) => "failed-unavailable",
            Self::Failed(FailureKeyword::InvalidData) => "failed-invalid",
            Self::Failed(FailureKeyword::Rejected) => "failed-rejected",
        }
    }

    /// A byte for the revision fingerprint. Written out rather than cast from
    /// the discriminant, so reordering the enum cannot silently renumber it.
    fn tag(self) -> u8 {
        match self {
            Self::InFlight => 1,
            Self::Landed => 2,
            Self::Failed(FailureKeyword::Unavailable) => 3,
            Self::Failed(FailureKeyword::InvalidData) => 4,
            Self::Failed(FailureKeyword::Rejected) => 5,
        }
    }
}

/// Which control part a popover [`Hit`] is published under.
///
/// **A keyword the code chose, never a label.** Two of them are `None`: the
/// panel itself is the group, and a management row's three actions hang off
/// the *reminder* rather than off the panel, so they are addressed by
/// [`saved_part`] instead.
fn popover_part(hit: Hit) -> Option<ControlPart> {
    Some(match hit {
        Hit::Panel => return None,
        Hit::Field => part::PHRASE,
        Hit::Date => part::DATE,
        Hit::Time => part::TIME,
        Hit::More => part::MORE,
        Hit::EndOfDaySettings => part::END_OF_DAY,
        Hit::Set => part::SET,
        Hit::Cancel => part::CANCEL,
        Hit::Preset(Preset::LaterToday) => part::LATER_TODAY,
        Hit::Preset(Preset::Tomorrow) => part::TOMORROW,
        Hit::Preset(Preset::Custom) => part::CUSTOM,
        Hit::Add => part::ADD,
        Hit::History => part::HISTORY,
        Hit::Clear => part::CLEAR,
        Hit::ConfirmClear => part::CLEAR_CONFIRM,
        Hit::CancelClear => part::CLEAR_CANCEL,
        Hit::RetryCurrent => part::RETRY,
        Hit::DiscardCurrent => part::DISCARD,
        // Scoped by the reminder, not by the panel.
        Hit::Row(..) | Hit::Recreate(_) => return None,
        // Scoped by the request token, not by the panel.
        Hit::Failed(..) => return None,
    })
}

/// …and the inverse, for a request arriving from a client.
fn popover_hit(part: ControlPart) -> Option<Hit> {
    Some(match part.name() {
        "phrase" => Hit::Field,
        "date" => Hit::Date,
        "time" => Hit::Time,
        "more" => Hit::More,
        "end-of-day" => Hit::EndOfDaySettings,
        "set" => Hit::Set,
        "cancel" => Hit::Cancel,
        "later-today" => Hit::Preset(Preset::LaterToday),
        "tomorrow" => Hit::Preset(Preset::Tomorrow),
        "custom" => Hit::Preset(Preset::Custom),
        "add" => Hit::Add,
        "history" => Hit::History,
        "clear" => Hit::Clear,
        "clear-confirm" => Hit::ConfirmClear,
        "clear-cancel" => Hit::CancelClear,
        "retry" => Hit::RetryCurrent,
        "discard" => Hit::DiscardCurrent,
        _ => return None,
    })
}

/// The part a management row's action is published under, on the reminder's own
/// object.
fn saved_part(action: RowAction) -> ControlPart {
    match action {
        RowAction::Edit => part::EDIT,
        RowAction::Pause => part::PAUSE,
        RowAction::Delete => part::DELETE,
    }
}

fn saved_action(part: ControlPart) -> Option<RowAction> {
    Some(match part.name() {
        "edit" => RowAction::Edit,
        "pause" => RowAction::Pause,
        "delete" => RowAction::Delete,
        _ => return None,
    })
}

fn pill_part(action: FailedAction) -> ControlPart {
    match action {
        FailedAction::Retry => part::PILL_RETRY,
        FailedAction::Edit => part::PILL_EDIT,
        FailedAction::Dismiss => part::PILL_DISMISS,
    }
}

fn pill_action(part: ControlPart) -> Option<FailedAction> {
    Some(match part.name() {
        "pill-retry" => FailedAction::Retry,
        "pill-edit" => FailedAction::Edit,
        "pill-dismiss" => FailedAction::Dismiss,
        _ => return None,
    })
}

/// One reminder on the item the caret is in, **and where the schedule came
/// from**.
///
/// The distinction is a type rather than a flag because it is the whole of what
/// this plan is about: a caller cannot read a schedule out of one of these
/// without having said which of the two it is holding. `MainView::
/// push_reminder_chip` makes exactly the same split for the drawn pill, in the
/// same order, on the same test — an optimistic record whose client-minted id
/// has not turned up in the authoritative snapshot yet.
pub(crate) enum SavedReminder<'a> {
    /// The Store's own answer, out of the reconciliation snapshot. **This is
    /// the saved schedule.**
    Confirmed(&'a GlobalItem),
    /// Submitted, and the projection has not caught up. A pill exists and a
    /// person can see it; nothing has confirmed what it says.
    InFlight(&'a OptimisticCreation),
}

impl SavedReminder<'_> {
    fn id(&self) -> &str {
        match self {
            Self::Confirmed(row) => &row.reminder_id,
            Self::InFlight(record) => &record.reminder_id,
        }
    }

    fn draft(&self) -> &Draft {
        match self {
            Self::Confirmed(row) => &row.draft,
            Self::InFlight(record) => &record.draft,
        }
    }

    /// The projected state, when there is a projection to have one.
    fn state(&self) -> Option<GlobalItemState> {
        match self {
            Self::Confirmed(row) => Some(row.state),
            Self::InFlight(_) => None,
        }
    }
}

/// A control of the open popover.
fn popover(cx: &Shell, instance: u64, part: ControlPart) -> Occurrence {
    cx.instance(POPOVER, instance, part)
}

/// A control belonging to one saved reminder, wherever it is drawn.
fn saved(cx: &Shell, reminder: uuid::Uuid, part: ControlPart) -> Occurrence {
    Occurrence::model(cx.window, cx.view, reminder, part)
}

impl MainView {
    /// Every reminder on the item the caret is in, authoritative first.
    ///
    /// **The same two passes, in the same order, on the same test as
    /// `push_reminder_chip`** — the Store's rows for this entity, then the
    /// submissions whose client-minted id has not turned up among them. A
    /// refused submission is in neither: its recovery pill is a control of its
    /// own, and letting it stand here would be a reminder that does not exist
    /// claiming to be scheduled.
    pub(crate) fn saved_reminders(&self) -> Vec<SavedReminder<'_>> {
        let Some(target) = self
            .reminder_subject()
            .and_then(|node| self.reminder_target(node))
        else {
            return Vec::new();
        };
        let key = target.entity.to_storage();
        let mut out: Vec<SavedReminder<'_>> = self
            .reminders
            .global()
            .rows
            .iter()
            .filter(|row| row.target == key)
            .map(SavedReminder::Confirmed)
            .collect();
        let confirmed: Vec<&str> = out.iter().map(SavedReminder::id).collect();
        let in_flight: Vec<SavedReminder<'_>> = self
            .reminders
            .optimistic_creations()
            .filter(|record| {
                record.target == key
                    && !matches!(record.state, OptimisticState::Failed { .. })
                    && !confirmed.contains(&record.reminder_id.as_str())
            })
            .map(SavedReminder::InFlight)
            .collect();
        out.extend(in_flight);
        out
    }

    /// Which saved reminder a management row stands for, and the inverse.
    ///
    /// A row's identity is the reminder's own id rather than its position, so a
    /// request that arrives after an asynchronous refresh reordered the list
    /// either finds the same reminder or finds nothing.
    fn saved_row_id(&self, index: usize) -> Option<uuid::Uuid> {
        let state = self.reminder_creation.management()?;
        uuid::Uuid::parse_str(&state.active.get(index)?.id).ok()
    }

    fn saved_row_index(&self, id: uuid::Uuid) -> Option<usize> {
        let state = self.reminder_creation.management()?;
        state
            .active
            .iter()
            .position(|item| uuid::Uuid::parse_str(&item.id).ok() == Some(id))
    }

    fn saved_history_id(&self, index: usize) -> Option<uuid::Uuid> {
        let state = self.reminder_creation.management()?;
        uuid::Uuid::parse_str(&state.history.get(index)?.reminder_id).ok()
    }

    fn saved_history_index(&self, id: uuid::Uuid) -> Option<usize> {
        let state = self.reminder_creation.management()?;
        state
            .history
            .iter()
            .position(|item| uuid::Uuid::parse_str(&item.reminder_id).ok() == Some(id))
    }

    /// Perform a control of the open popover, and settle the result the way a
    /// keystroke on it settles.
    fn invoke_reminder_control(
        &mut self,
        ws: &mut koru_core::schema::WorkspaceDoc,
        hit: Hit,
    ) -> bool {
        let Some(outcome) = self.reminder_creation.invoke(&mut self.engine, hit) else {
            return false;
        };
        self.settle_reminder_creation(ws, outcome)
    }

    /// **What is this item's reminder?** — answered from what the Store gave
    /// back, plus whatever became news.
    ///
    /// Published beside the save status, and for the same reason: somebody
    /// walking the chrome on their way into the document passes *is my work
    /// saved* and *when will Koru remind me* before they arrive, rather than
    /// after they have left.
    pub(super) fn reminder_semantics(&self, cx: &Shell) -> Option<SemanticNode> {
        // Nothing of reminders is exposed in a build that hides them
        // (koru-features).
        if !koru_features::reminders() {
            return None;
        }
        let mut children = Vec::new();
        let saved_here = self.saved_reminders();
        for (index, reminder) in saved_here.iter().enumerate() {
            let Ok(id) = uuid::Uuid::parse_str(reminder.id()) else {
                // A reminder whose id is not a uuid cannot be given a stable
                // identity, and inventing one would let a request land on a
                // neighbour. It is left out rather than published under a
                // number that means nothing.
                continue;
            };
            children.push(
                SemanticNode::new(saved(cx, id, part::SCHEDULE), Role::Label)
                    // A `Role::Label`'s accessible name is read from its
                    // **value** on Windows; `build` copies the label across, so
                    // saying it once here is enough. See `guide/contributing/deps/ACCESSKIT.md`.
                    .with_label(self.saved_reminder_sentence(cx, reminder))
                    // Zero-based: `accesskit_windows` adds the one a person
                    // hears. **No total beside it**: `size_of_set` is read off
                    // the *selection container*, and `Role::Group` is not one
                    // of the roles AccessKit counts as one — a number set here
                    // would be discarded silently (ADR-338, and
                    // `guide/contributing/deps/ACCESSKIT.md`). Making the group a `ListBox`
                    // to buy the total would claim these are selectable, and
                    // they are sentences.
                    .with_position(index),
            );
        }
        // The recovery pills a refused submission leaves standing. These *are*
        // drawn, outside the panel and whether or not the popover is open, so
        // they carry the rectangle a press is already resolved against.
        children.extend(self.reminder_recovery_semantics(cx));
        if let Some(news) = self.reminder_news.news {
            children.push(
                SemanticNode::new(cx.surface(REMIND, part::NEWS), Role::Alert)
                    .with_label(t(cx.lang, news))
                    // **On the leaf, never on the group.** `live` is inherited
                    // by descendants, so marking the container would make every
                    // schedule above speak whenever the Store answered — the
                    // exact collapse ADR-340 exists to avoid.
                    .announcing(match news {
                        // A refusal is what somebody is waiting on; a save that
                        // landed is progress they asked for and can hear when
                        // the reader gets to it. The setting changing is itself
                        // a reason the adapter re-announces, which is right:
                        // *saved* then *not saved* is two pieces of news.
                        Key::A11yReminderNewsSaved => Live::Polite,
                        _ => Live::Assertive,
                    }),
            );
        }
        // A named group with nothing in it is a dead end for somebody asking a
        // question, so there is no group unless there is an answer.
        (!children.is_empty()).then(|| {
            SemanticNode::new(cx.surface(REMIND, parts::ROOT), Role::Group)
                .with_label(t(cx.lang, Key::A11yReminderOnThisItem))
                .with_children(children)
        })
    }

    /// One saved reminder, as a sentence.
    ///
    /// **Three facts, and the middle one is the point.** The schedule as the
    /// Store holds it, rendered in the reader's display zone by the same
    /// function the pill's own caption comes from; whether the Store has
    /// confirmed it or it is still a submission in flight; and what state the
    /// projection says the rule is in, in the words the overlay already uses.
    fn saved_reminder_sentence(&self, cx: &Shell, reminder: &SavedReminder<'_>) -> String {
        let schedule = crate::mainview::reminder_creation::format_draft_label_during_maintenance(
            cx.lang,
            &self.reminder_environment.locale,
            &self.reminder_environment.resolution.current_zone,
            reminder.draft(),
        );
        let mut out = t(cx.lang, Key::ReminderScheduledFor).replace("{}", &schedule);
        match reminder.state() {
            // **Said, rather than left to be inferred.** The pill a sighted
            // person sees looks the same either way; this is the sentence that
            // makes the difference audible, and it is why a screen reader can
            // answer *is this the saved schedule?* when a pointer cannot.
            None => {
                out.push_str(" \u{00b7} ");
                out.push_str(t(cx.lang, Key::A11yReminderUnconfirmed));
            }
            // *Coming up* is what an ordinary confirmed reminder is, and
            // saying it about every one of them is noise; the four states that
            // are not that are worth hearing.
            Some(GlobalItemState::ComingUp) => {}
            Some(state) => {
                out.push_str(" \u{00b7} ");
                out.push_str(t(
                    cx.lang,
                    crate::mainview::reminder_shell::state_key(state),
                ));
            }
        }
        out
    }

    /// The Retry / Edit / Dismiss a refused submission offers.
    ///
    /// Scoped by the request token, which is what `Controller::retry_creation`
    /// and its two neighbours take, so three failures standing at once each
    /// recover the one that failed.
    fn reminder_recovery_semantics(&self, cx: &Shell) -> Vec<SemanticNode> {
        self.reminder_creation
            .failed_pills()
            .map(|(token, pill)| {
                let id = token.as_uuid();
                let actions = [
                    (FailedAction::Retry, Key::ReminderRetry),
                    (FailedAction::Edit, Key::ReminderEdit),
                    (FailedAction::Dismiss, Key::ReminderDismiss),
                ]
                .into_iter()
                .map(|(action, key)| {
                    SemanticNode::new(saved(cx, id, pill_part(action)), Role::Button)
                        .with_label(t(cx.lang, key))
                        .with_bounds(pill)
                        .with_actions([Action::Click, Action::Focus])
                })
                .collect();
                SemanticNode::new(saved(cx, id, part::PILL), Role::Group)
                    .with_label(t(cx.lang, Key::ReminderErrorRejected))
                    .with_bounds(pill)
                    .with_children(actions)
            })
            .collect()
    }

    /// The reminder popover, as the surface that owns everything beneath it.
    ///
    /// `None` when no popover is standing, which is what lets
    /// `menus::blocker_semantics` fall through to the hollow `Dialog` for every
    /// other blocker.
    pub(super) fn reminder_popover_semantics(&self, cx: &Shell) -> Option<SemanticNode> {
        let surface = &self.reminder_creation;
        let panel = surface.panel()?;
        surface.mode()?;
        let instance = surface.instance();
        let mut children = Vec::new();
        // **The status first**, because it is what the panel is about: a
        // person arriving in this dialog wants to hear what their phrase
        // resolved to before they meet the buttons that act on it.
        let (message, _) = surface.message();
        if !message.is_empty() {
            children.push(
                SemanticNode::new(popover(cx, instance, part::MESSAGE), Role::Label)
                    .with_label(message.to_string()),
            );
        }
        if surface.structured_is_invalid() {
            // The red line under the date and time fields, published as the
            // same sentence it draws. It is a *status*, not news: it moves with
            // every keystroke in those fields.
            children.push(
                SemanticNode::new(popover(cx, instance, part::INVALID), Role::Label)
                    .with_label(t(cx.lang, Key::ReminderErrorInvalidData)),
            );
        }
        let order = surface.tab_order();
        for (hit, rect, enabled) in surface.controls() {
            let Some(node) = self.reminder_control_semantics(cx, instance, hit, rect, enabled)
            else {
                continue;
            };
            // Zero-based, and counting what a person can actually reach: a
            // disabled control is published so it can be heard, and is not
            // given a number in a set it is not part of.
            let node = match order.iter().position(|reachable| *reachable == hit) {
                Some(index) => node.with_position(index),
                None => node,
            };
            children.push(node);
        }
        Some(
            SemanticNode::new(popover(cx, instance, parts::ROOT), Role::Dialog)
                .with_label(surface.hit_label(cx.lang, Hit::Panel))
                .with_bounds(panel)
                .with_children(children)
                .modal(),
        )
    }

    /// One control of the open popover.
    fn reminder_control_semantics(
        &self,
        cx: &Shell,
        instance: u64,
        hit: Hit,
        rect: koru_draw::geometry::Rect,
        enabled: bool,
    ) -> Option<SemanticNode> {
        let surface = &self.reminder_creation;
        let label = surface.hit_label(cx.lang, hit);
        // A field: named by the label drawn beside it, carrying what is in it.
        //
        // **No `Action::Click`.** `accesskit_consumer` excludes a text input
        // from `is_invocable` whatever it advertises, and pressing Enter in a
        // field is not what a client means by invoking one. `SetValue` is the
        // action a client has for a field — and it is deliberately *not*
        // offered either, because no behaviour owner here performs it: the
        // phrase is parsed a keystroke at a time by `Surface::key`, and a node
        // advertising an action nothing performs is worse than one that is
        // absent.
        if let Some(text) = surface.field_text(hit) {
            return Some(
                SemanticNode::new(popover(cx, instance, popover_part(hit)?), Role::TextInput)
                    .with_label(label)
                    .with_value(text.to_string())
                    .with_bounds(rect)
                    .with_action(Action::Focus)
                    .disabled(!enabled),
            );
        }
        let occurrence = match hit {
            Hit::Row(index, action) => {
                let id = self.saved_row_id(index)?;
                saved(cx, id, saved_part(action))
            }
            Hit::Recreate(index) => {
                let id = self.saved_history_id(index)?;
                saved(cx, id, part::RECREATE)
            }
            Hit::Failed(token, action) => saved(cx, token.as_uuid(), pill_part(action)),
            other => popover(cx, instance, popover_part(other)?),
        };
        // A row's three actions are about one schedule, and a client reads them
        // one at a time — so the schedule is part of the name rather than a
        // heading above them that a reader may never have passed.
        let label = match surface.hit_schedule(cx.lang, hit) {
            Some(schedule) => format!("{label} \u{00b7} {schedule}"),
            None => label,
        };
        Some(
            SemanticNode::new(occurrence, Role::Button)
                .with_label(label)
                .with_bounds(rect)
                .with_actions([Action::Click, Action::Focus])
                .disabled(!enabled),
        )
    }

    /// Where the keyboard is while the reminder popover is standing.
    ///
    /// `Surface::focus` — the state its own arrows move and its focus ring is
    /// drawn from — rather than a second idea of focus beside it. The panel
    /// itself when nothing inside it has been reached yet, which is what a
    /// freshly opened popover looks like before `sync_creation` seeds the
    /// field.
    pub(super) fn reminder_focus(&self, cx: &Shell) -> Option<Occurrence> {
        let surface = &self.reminder_creation;
        // **A recovery pill is answered for whether or not a popover stands.**
        // It is published either way — `reminder_semantics` says so, because it
        // is *drawn* either way — and `F6` reaches it precisely in the case
        // where no panel stands and there is therefore no `mode`. Asked before
        // the gate below, so the one state the focus ring is painted from is
        // the one state a client is told about.
        if let Some(Hit::Failed(token, action)) = surface.focus() {
            return Some(saved(cx, token.as_uuid(), pill_part(action)));
        }
        surface.mode()?;
        let instance = surface.instance();
        let Some(hit) = surface.focus() else {
            return Some(popover(cx, instance, parts::ROOT));
        };
        match hit {
            Hit::Panel => Some(popover(cx, instance, parts::ROOT)),
            Hit::Row(index, action) => self
                .saved_row_id(index)
                .map(|id| saved(cx, id, saved_part(action))),
            Hit::Recreate(index) => self
                .saved_history_id(index)
                .map(|id| saved(cx, id, part::RECREATE)),
            // `Hit::Failed` is answered above, before the `mode` gate, because
            // a pill stands with no popover. Named here so the arm that would
            // have gone next — `popover_part`, which refuses it — reads as a
            // deliberate absence rather than a hole.
            other => popover_part(other).map(|part| popover(cx, instance, part)),
        }
    }

    /// Fold this surface's inputs into the shell's fingerprint.
    ///
    /// Everything that changes a published label, and nothing that moves on its
    /// own. The popover's panel rectangle is in, because a control's bounds are
    /// relative to it; the caret inside its fields is not, because a caret does
    /// not change what any of these nodes say.
    pub(super) fn reminder_revision(&self, rev: &mut Revision) {
        let surface = &self.reminder_creation;
        rev.byte(match surface.mode() {
            None => 0,
            Some(crate::mainview::reminder_creation::Mode::Creation) => 1,
            Some(crate::mainview::reminder_creation::Mode::Management) => 2,
        });
        rev.u64(surface.instance());
        rev.str(surface.message().0);
        rev.bool(surface.structured_is_invalid());
        // A field can change while focus, validation and every rectangle stay
        // identical. Fingerprint the same borrowed raw text published as Value,
        // or UIA keeps the old value until an unrelated control changes.
        for hit in [Hit::Field, Hit::Date, Hit::Time] {
            let text = surface.field_text(hit);
            rev.bool(text.is_some()).str(text.unwrap_or_default());
        }
        // **Where the keyboard is**, which is what `reminder_focus` answers
        // with — and the one input here that can change while every control's
        // rectangle, words and enabled state stay exactly as they were. `F6`
        // landing on a standing recovery pill moves nothing else at all, so
        // without this line the ring would appear and a client would never be
        // told, which is a focus only a sighted person has. It moves on input
        // and never on its own, so it does not break this fingerprint's rule
        // against folding in anything that moves per frame. `0` is not a
        // `hit_tag`, so it reads as *nowhere*.
        let (kind, index) = surface.focus().map_or((0, 0), hit_tag);
        rev.byte(kind).u64(index);
        // **Which control, where, and whether it works — never its words.**
        // `Surface::hit_label` allocates a `String`, and this runs on the loop
        // every pass a client is attached; a name is a pure function of the
        // control, the language (which cannot change under a running process)
        // and the management rows folded in below, so hashing the inputs says
        // everything hashing the output would.
        for (hit, rect, enabled) in surface.controls() {
            let (kind, index) = hit_tag(hit);
            rev.byte(kind).u64(index).bool(enabled);
            rev.f32(rect.x).f32(rect.y).f32(rect.w).f32(rect.h);
        }
        // The rows the panel is showing, which are what a row control is named
        // after and what its Pause button says.
        if let Some(state) = surface.management() {
            rev.bool(state.loading).bool(state.error.is_some());
            rev.bool(state.history_expanded)
                .bool(state.clear_history_confirmation);
            for item in &state.active {
                rev.str(&item.id).str(&item.revision);
                rev.byte(item_state_tag(&item.state));
                revise_schedule(rev, &item.draft.schedule);
            }
            for item in &state.history {
                rev.str(&item.reminder_id).str(&item.occurrence);
                revise_schedule(rev, &item.draft.schedule);
            }
        }
        for (token, pill) in surface.failed_pills() {
            rev.opt_uuid(Some(token.as_uuid()));
            rev.f32(pill.x).f32(pill.y);
        }
        // What the Store says about this item, which is what `#root` answers.
        // The *state* rather than the sentence: the sentence is a function of
        // it and the display zone, and formatting one per pass to fingerprint
        // it would do the work this number exists to avoid.
        for reminder in self.saved_reminders() {
            rev.str(reminder.id());
            rev.byte(reminder.state().map_or(0, state_tag));
            revise_schedule(rev, &reminder.draft().schedule);
        }
        // The key's own ordinal, plus one so *no news* is distinguishable from
        // the first row in the table.
        rev.u64(self.reminder_news.news.map_or(0, |news| news as u64 + 1));
        rev.byte(self.reminder_news.seen.map_or(0, Submissions::tag));
    }
}

/// Which control a [`Hit`] is, as a number a fingerprint can carry.
///
/// Written out rather than derived, for [`Submissions::tag`]'s reason: an enum
/// somebody reorders must not silently renumber a fingerprint. The second half
/// is the index a row or a history entry carries, and `0` for the controls that
/// have none.
fn hit_tag(hit: Hit) -> (u8, u64) {
    match hit {
        Hit::Panel => (1, 0),
        Hit::Field => (2, 0),
        Hit::Date => (3, 0),
        Hit::Time => (4, 0),
        Hit::More => (5, 0),
        Hit::EndOfDaySettings => (6, 0),
        Hit::Set => (7, 0),
        Hit::Cancel => (8, 0),
        Hit::Preset(Preset::LaterToday) => (9, 0),
        Hit::Preset(Preset::Tomorrow) => (10, 0),
        Hit::Preset(Preset::Custom) => (11, 0),
        Hit::Add => (12, 0),
        Hit::History => (13, 0),
        Hit::Clear => (14, 0),
        Hit::ConfirmClear => (15, 0),
        Hit::CancelClear => (16, 0),
        Hit::RetryCurrent => (17, 0),
        Hit::DiscardCurrent => (18, 0),
        Hit::Row(index, RowAction::Edit) => (19, index as u64),
        Hit::Row(index, RowAction::Pause) => (20, index as u64),
        Hit::Row(index, RowAction::Delete) => (21, index as u64),
        Hit::Recreate(index) => (22, index as u64),
        Hit::Failed(_, FailedAction::Retry) => (23, 0),
        Hit::Failed(_, FailedAction::Edit) => (24, 0),
        Hit::Failed(_, FailedAction::Dismiss) => (25, 0),
    }
}

/// A management row's stored state, as a byte. The pause control's *word*
/// changes with it, so the fingerprint has to.
fn item_state_tag(state: &koru_editor::reminders::ItemState) -> u8 {
    use koru_editor::reminders::ItemState as S;
    match state {
        S::Active => 1,
        S::Paused => 2,
        S::WaitingForDue => 3,
        S::Snoozed { .. } => 4,
    }
}

/// Fold a schedule into the fingerprint **by its parts**, not by a rendering.
///
/// The published sentence is a function of this value and the reader's display
/// zone, so the fingerprint has to move when it does — but formatting one per
/// pass to hash it would do exactly the work the number exists to avoid, and
/// `Debug` would allocate a string on the loop every time a client asked. The
/// three arms are the three temporal shapes a reminder draft can take
/// ([docs/architecture/TIME.md](../../../../../docs/architecture/TIME.md)); each is folded in as the
/// numbers it is made of, with no conversion between them.
fn revise_schedule(rev: &mut Revision, schedule: &ReminderDraft) {
    match schedule {
        ReminderDraft::Floating(value) => {
            rev.byte(1);
            revise_wall(rev, value.wall());
        }
        ReminderDraft::Zoned(value) => {
            rev.byte(2);
            rev.u64(value.instant_ms() as u64);
            rev.str(value.zone().as_str());
        }
        ReminderDraft::Relative {
            relation, preview, ..
        } => {
            rev.byte(3);
            rev.byte(match relation {
                DueRelation::At => 1,
                DueRelation::Before => 2,
            });
            match preview {
                None => rev.byte(0),
                Some(ReminderMoment::Floating(value)) => {
                    rev.byte(1);
                    revise_wall(rev, value.wall());
                    rev
                }
                Some(ReminderMoment::Zoned(value)) => {
                    rev.byte(2);
                    rev.u64(value.instant_ms() as u64);
                    rev.str(value.zone().as_str())
                }
            };
        }
    }
}

/// A wall clock, as the five numbers it is. No zone is involved, and none is
/// invented: a floating value is a reading rather than a moment, and folding
/// one through UTC to get a number would be the oldest bug in `docs/architecture/TIME.md`
/// wearing a fingerprint's clothes.
fn revise_wall(rev: &mut Revision, wall: chrono::NaiveDateTime) {
    use chrono::{Datelike, Timelike};
    rev.u64(wall.year() as u64);
    rev.u64(u64::from(wall.month()));
    rev.u64(u64::from(wall.day()));
    rev.u64(u64::from(wall.hour()));
    rev.u64(u64::from(wall.minute()));
}

/// A byte for the revision fingerprint, written out for [`Submissions::tag`]'s
/// reason.
fn state_tag(state: GlobalItemState) -> u8 {
    match state {
        GlobalItemState::NeedsAttention => 1,
        GlobalItemState::ComingUp => 2,
        GlobalItemState::Snoozed => 3,
        GlobalItemState::Paused => 4,
        GlobalItemState::History => 5,
    }
}

/// Whether a request names something inside the standing reminder popover.
///
/// The other half of publishing a blocker alone: a client may still hold an id
/// from before the popover opened, and performing it would be the shell acting
/// on a control the keyboard cannot currently reach.
pub(super) fn is_popover_target(view: &MainView, target: Occurrence) -> bool {
    if view.reminder_creation.mode().is_none() {
        return false;
    }
    if instance_of(target, POPOVER) == Some(view.reminder_creation.instance()) {
        return true;
    }
    // A management row's and a failed pill's controls hang off the reminder or
    // the request rather than off the panel, so they are recognised by
    // resolving them against what is on screen right now.
    matches!(target.object, ObjectKey::Model(_)) && resolve(view, target).is_some()
}

/// Which control of the open popover a target names, if any.
///
/// **Resolved against current state, every time.** A management row's identity
/// is the reminder's own id rather than its position, so a client holding one
/// from before an asynchronous refresh either finds the same reminder or finds
/// nothing — never the neighbour that moved into its place.
fn resolve(view: &MainView, target: Occurrence) -> Option<Hit> {
    match target.object {
        ObjectKey::Instance { kind, id }
            if kind == POPOVER && id == view.reminder_creation.instance() =>
        {
            popover_hit(target.part)
        }
        ObjectKey::Model(id) => {
            if let Some(action) = saved_action(target.part) {
                return view.saved_row_index(id).map(|i| Hit::Row(i, action));
            }
            if target.part == part::RECREATE {
                return view.saved_history_index(id).map(Hit::Recreate);
            }
            let action = pill_action(target.part)?;
            view.reminder_creation
                .failed_pills()
                .find(|(token, _)| token.as_uuid() == id)
                .map(|(token, _)| Hit::Failed(token, action))
        }
        _ => None,
    }
}

/// Route a request at the reminder surface, through the press that already
/// exists.
pub(super) fn action(
    view: &mut MainView,
    ws: &mut koru_core::schema::WorkspaceDoc,
    target: Occurrence,
    action: Action,
) -> bool {
    if surface_of(target) == Some(REMIND) {
        // The status, the announcement and the group answer questions and
        // perform nothing. Naming one is not a reason to do anything.
        return false;
    }
    if !matches!(action, Action::Click | Action::Focus) {
        return false;
    }
    let Some(hit) = resolve(view, target) else {
        return false;
    };
    if action == Action::Focus {
        return view.reminder_creation.take_focus(hit);
    }
    view.invoke_reminder_control(ws, hit)
}

#[cfg(test)]
mod tests;
