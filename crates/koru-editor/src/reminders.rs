// SPDX-License-Identifier: MPL-2.0

//! Design-neutral reminder interaction state.
//!
//! The editor owns what a reminder interaction *means*, not where a future
//! design places it. [`Creation`] models the compact confirmation flow and
//! [`Management`] models one entity's rows and intents. Neither type reads the
//! Store, a clock, settings, or the document. The host supplies resolved data
//! and drains intents, keeping every potentially blocking call out of input,
//! layout, and drawing.

use std::collections::{BTreeMap, BTreeSet};

use chrono::TimeDelta;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::utils::when::{
    ReminderDraft, Resolution, ResolutionContext, ResolutionIssue, parse_reminder,
    review_reminder_draft,
};
use koru_core::{NodeId, node::Attr, schema::WorkspaceDoc};

/// Globally unique ownership token for one materialized `/remind` command.
///
/// Unlike an editor-local counter, this remains collision-safe when a tab is
/// closed and a new editor is constructed for the same document.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestToken(uuid::Uuid);

impl RequestToken {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// The token as the uuid it is, so a presentation identity can be scoped
    /// by the request that failed rather than by its position among the pills.
    /// Nothing persists this.
    pub fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl Default for RequestToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Exact editor ownership carried through cancellation and Store submission.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestIdentity {
    pub token: RequestToken,
    pub node: NodeId,
}

/// The exact visible command text retained until the request is submitted.
///
/// Its CRDT-stable range is deletion authority only for the matching request.
/// Pre-submit cancellation removes synthetic text and preserves person-authored
/// slash text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingText {
    pub identity: RequestIdentity,
    pub text: String,
    pub range: koru_core::StableTextRange,
    /// Only a plus/context action is synthetic. Person-authored slash text is
    /// never removed when a pre-submit surface is cancelled.
    pub origin: RequestOrigin,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RequestOrigin {
    PersonAuthored,
    Synthetic,
}

/// The stable editor-to-host request produced by `/remind`.
///
/// `identity` identifies the exact pending trigger text across editor
/// lifetimes. The host returns it at submission so the editor can remove only
/// that request's exact command. A cancelled person-authored flow leaves the
/// typed command as ordinary document text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub identity: RequestIdentity,
    pub phrase: String,
    pub due_anchor: Option<koru_core::time::Temporal>,
}

/// Read the exact source due value used by relative reminder resolution.
/// This is an O(1) semantic lookup for the one open creation, not a row walk.
pub fn due_anchor(ws: &WorkspaceDoc, node: NodeId) -> Option<koru_core::time::Temporal> {
    ws.read(node)
        .ok()?
        .content
        .attrs()
        .iter()
        .find(|attr| attr.role == koru_core::attributes::KEY_DUE)
        .and_then(Attr::temporal)
}

/// Why natural-language input needs the structured fallback.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FallbackReason {
    Unsupported,
    UnknownZone,
}

/// Stable failure categories. User-visible wording is localized by the host.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Unavailable,
    InvalidData,
    Rejected,
}

/// The temporary presentation that replaces submitted command text.
///
/// Each instance is keyed by its globally unique request identity. A durable
/// answer therefore cannot accidentally update whichever creation happens to
/// be open when it arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptimisticCreation {
    pub request: Request,
    pub target: String,
    pub reminder_id: String,
    pub draft: Draft,
    pub state: OptimisticState,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OptimisticState {
    Pending,
    Succeeded,
    Failed { reason: Failure },
}

/// Closed reminder-tone vocabulary shared semantically with Store without
/// coupling the editor to Store types.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    System,
    KoruDefault,
    Soft,
    Chime,
    Pulse,
    Bright,
    Calm,
    Drop,
    Knock,
    Signal,
    Urgent,
}

impl Tone {
    pub const ALL: [Self; 11] = [
        Self::System,
        Self::KoruDefault,
        Self::Soft,
        Self::Chime,
        Self::Pulse,
        Self::Bright,
        Self::Calm,
        Self::Drop,
        Self::Knock,
        Self::Signal,
        Self::Urgent,
    ];

    pub const fn token(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::KoruDefault => "koru_default",
            Self::Soft => "soft",
            Self::Chime => "chime",
            Self::Pulse => "pulse",
            Self::Bright => "bright",
            Self::Calm => "calm",
            Self::Drop => "drop",
            Self::Knock => "knock",
            Self::Signal => "signal",
            Self::Urgent => "urgent",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tone| tone.token() == token)
    }
}

/// A value ready to become a new Store rule.
///
/// New reminders always begin without a per-reminder tone override. The
/// override is deliberately present in the draft because edit and recreate
/// preserve an existing value even though phase one does not expose a control
/// for changing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub schedule: ReminderDraft,
    pub tone_override: Option<Tone>,
}

impl Draft {
    pub fn new(schedule: ReminderDraft) -> Self {
        Self {
            schedule,
            tone_override: None,
        }
    }

    pub fn preserving(schedule: ReminderDraft, tone_override: Option<Tone>) -> Self {
        Self {
            schedule,
            tone_override,
        }
    }
}

/// The visible semantic state of `/remind`; presentation chooses its geometry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationState {
    /// Settings are loaded and the natural-language field is empty. The quick
    /// surface remains on its preset choices rather than treating absence of
    /// input as a parse failure.
    Empty,
    /// The host is fetching settings needed for deterministic resolution.
    Loading,
    /// Settings/rows could not be fetched. Retrying keeps the same request and
    /// generation; cancellation still leaves the typed command as text.
    LoadFailed { reason: Failure },
    /// Fully resolved input. `/remind` always lands here rather than committing.
    Confirmation(Draft),
    /// The requested time passed; `proposal` is selected but remains uncommitted.
    Passed { requested: Draft, proposal: Draft },
    /// Input needs an explicit answer. Ambiguous zones are never guessed.
    Ambiguity,
    /// “End of day” was understood, but its profile setting is absent.
    EndOfDay,
    /// Parsing could not produce a safe draft; structured controls remain usable.
    Structured {
        reason: FallbackReason,
        phrase: String,
    },
    /// A durable mutation is in flight. Its optimistic pill replaces the text.
    Saving(Draft),
    /// The host reported a failure and the draft remains editable/retryable.
    Failed { draft: Draft, reason: Failure },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CreationFocus {
    Editor,
    Confirm,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationIntent {
    Confirm(Draft),
    Cancel,
}

/// One open `/remind` interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Creation {
    pub request: Request,
    pub state: CreationState,
    pub focus: CreationFocus,
    /// The profile tone shown in confirmation. It is not copied into a new
    /// rule: `tone_override = None` means “follow this setting”.
    pub default_tone: Option<Tone>,
    /// Existing/Recreate flows preserve a per-rule tone even while natural
    /// language is temporarily unresolved and therefore carries no Draft.
    pub tone_override: Option<Tone>,
}

impl Creation {
    pub fn loading(request: Request) -> Self {
        Self {
            request,
            state: CreationState::Loading,
            focus: CreationFocus::Editor,
            default_tone: None,
            tone_override: None,
        }
    }

    /// Open an existing semantic draft in the same passed-time review used by
    /// typed creation. No display text is reparsed, so Relative schedules,
    /// canonical zones, and an existing tone override remain exact.
    pub fn reviewing(
        request: Request,
        draft: Draft,
        context: &ResolutionContext,
        default_tone: Tone,
    ) -> Self {
        let tone_override = draft.tone_override;
        let state = Self::reviewed_state(draft, context, &request.phrase);
        Self {
            request,
            state,
            focus: CreationFocus::Editor,
            default_tone: Some(default_tone),
            tone_override,
        }
    }

    fn reviewed_state(draft: Draft, context: &ResolutionContext, phrase: &str) -> CreationState {
        let tone_override = draft.tone_override;
        match review_reminder_draft(draft.schedule, context) {
            Resolution::Resolved(schedule) => {
                CreationState::Confirmation(Draft::preserving(schedule, tone_override))
            }
            Resolution::Passed {
                requested,
                proposal,
            } => CreationState::Passed {
                requested: Draft::preserving(requested, tone_override),
                proposal: Draft::preserving(proposal, tone_override),
            },
            Resolution::Needs(ResolutionIssue::AmbiguousZone) => CreationState::Ambiguity,
            Resolution::Needs(ResolutionIssue::MissingEndOfDay) => CreationState::EndOfDay,
            Resolution::Needs(ResolutionIssue::UnknownZone) => CreationState::Structured {
                reason: FallbackReason::UnknownZone,
                phrase: phrase.to_owned(),
            },
            Resolution::Needs(ResolutionIssue::Unsupported) => CreationState::Structured {
                reason: FallbackReason::Unsupported,
                phrase: phrase.to_owned(),
            },
        }
    }

    /// Re-review a semantic draft after its due anchor or cached profile
    /// settings change. Passed review keeps the authored request, not its
    /// suggested future proposal, so Relative relation/offset and tone remain
    /// the source of truth.
    pub fn review_typed(&mut self, context: &ResolutionContext, default_tone: Tone) {
        let source = match &self.state {
            CreationState::Passed { requested, .. } => Some(requested.clone()),
            CreationState::Confirmation(draft) | CreationState::Failed { draft, .. } => {
                Some(draft.clone())
            }
            _ => None,
        };
        self.default_tone = Some(default_tone);
        if let Some(source) = source {
            self.tone_override = source.tone_override;
            self.state = Self::reviewed_state(source, context, &self.request.phrase);
        }
        self.focus = CreationFocus::Editor;
    }

    /// Resolve after the host has asynchronously fetched reminder settings.
    pub fn resolve(&mut self, context: &ResolutionContext, default_tone: Tone) {
        self.default_tone = Some(default_tone);
        if self.request.phrase.trim().is_empty() {
            self.state = CreationState::Empty;
            self.focus = CreationFocus::Editor;
            return;
        }
        self.state = match parse_reminder(&self.request.phrase, context) {
            Resolution::Resolved(schedule) => {
                CreationState::Confirmation(Draft::preserving(schedule, self.tone_override))
            }
            Resolution::Passed {
                requested,
                proposal,
            } => CreationState::Passed {
                requested: Draft::preserving(requested, self.tone_override),
                proposal: Draft::preserving(proposal, self.tone_override),
            },
            Resolution::Needs(ResolutionIssue::AmbiguousZone) => CreationState::Ambiguity,
            Resolution::Needs(ResolutionIssue::MissingEndOfDay) => CreationState::EndOfDay,
            Resolution::Needs(ResolutionIssue::UnknownZone) => CreationState::Structured {
                reason: FallbackReason::UnknownZone,
                phrase: self.request.phrase.clone(),
            },
            Resolution::Needs(ResolutionIssue::Unsupported) => CreationState::Structured {
                reason: FallbackReason::Unsupported,
                phrase: self.request.phrase.clone(),
            },
        };
        self.focus = CreationFocus::Editor;
    }

    /// Resolve edited natural language without losing an existing reminder's
    /// per-rule tone. The creation surface does not expose tone in phase one,
    /// so an edit must carry it through every parse outcome.
    pub fn resolve_preserving(
        &mut self,
        context: &ResolutionContext,
        default_tone: Tone,
        tone_override: Option<Tone>,
    ) {
        self.tone_override = tone_override;
        self.resolve(context, default_tone);
    }

    pub fn editable_draft(&self) -> Option<&Draft> {
        match &self.state {
            CreationState::Confirmation(draft) | CreationState::Failed { draft, .. } => Some(draft),
            CreationState::Passed { proposal, .. } => Some(proposal),
            _ => None,
        }
    }

    pub fn confirmable_draft(&self) -> Option<&Draft> {
        match &self.state {
            CreationState::Confirmation(draft) | CreationState::Failed { draft, .. } => Some(draft),
            CreationState::Passed { proposal, .. } => Some(proposal),
            CreationState::Empty
            | CreationState::Loading
            | CreationState::LoadFailed { .. }
            | CreationState::Ambiguity
            | CreationState::EndOfDay
            | CreationState::Structured { .. }
            | CreationState::Saving(_) => None,
        }
    }

    pub fn replace_draft(&mut self, schedule: ReminderDraft) {
        if matches!(self.state, CreationState::Saving(_)) {
            return;
        }
        let tone = self
            .editable_draft()
            .and_then(|draft| draft.tone_override)
            .or(self.tone_override);
        self.tone_override = tone;
        self.state = CreationState::Confirmation(Draft::preserving(schedule, tone));
        self.focus = CreationFocus::Editor;
    }

    /// Keyboard semantics independent of the eventual layout.
    pub fn key(&mut self, key: &Key, mods: ModifiersState) -> Option<CreationIntent> {
        // A submitted draft has one owner until its durable answer arrives.
        // It cannot be edited, cancelled, or submitted a second time.
        if matches!(self.state, CreationState::Saving(_)) {
            return None;
        }
        match key {
            Key::Named(NamedKey::Escape) => return Some(CreationIntent::Cancel),
            Key::Named(NamedKey::Tab) => {
                self.focus = if mods.shift_key() {
                    match self.focus {
                        CreationFocus::Editor => CreationFocus::Cancel,
                        CreationFocus::Confirm => CreationFocus::Editor,
                        CreationFocus::Cancel => CreationFocus::Confirm,
                    }
                } else {
                    match self.focus {
                        CreationFocus::Editor => CreationFocus::Confirm,
                        CreationFocus::Confirm => CreationFocus::Cancel,
                        CreationFocus::Cancel => CreationFocus::Editor,
                    }
                };
                return None;
            }
            Key::Named(NamedKey::Enter) => match self.focus {
                CreationFocus::Cancel => return Some(CreationIntent::Cancel),
                CreationFocus::Confirm => {
                    if let Some(draft) = self.confirmable_draft().cloned() {
                        self.state = CreationState::Saving(draft.clone());
                        return Some(CreationIntent::Confirm(draft));
                    }
                }
                CreationFocus::Editor => {}
            },
            _ => {}
        }
        None
    }

    pub fn save_failed(&mut self, reason: Failure) {
        if let CreationState::Saving(draft) = &self.state {
            let draft = draft.clone();
            self.state = CreationState::Failed { draft, reason };
        }
    }

    pub fn load_failed(&mut self, reason: Failure) {
        if matches!(self.state, CreationState::Loading) {
            self.state = CreationState::LoadFailed { reason };
        }
    }
}

/// Semantic state shown by an entity's reminder chip.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ChipSummary {
    /// Active includes scheduled, snoozed, waiting-for-due, and explicitly
    /// paused rules; handled history never contributes.
    pub active_count: usize,
    pub waiting: bool,
    pub snoozed: bool,
}

impl ChipSummary {
    pub fn visible(self) -> bool {
        self.active_count > 0
    }
}

/// One host-supplied reminder rendered after an object's authored text and
/// attribute chips. The Store identity travels with the label so a press can
/// address exactly one reminder rather than reopening an object-level list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineChip {
    pub reminder_id: String,
    pub label: String,
    pub paused: bool,
    /// The active occurrence is deferred until the time in `label`.
    pub snoozed: bool,
    /// This occurrence has fired on this device and still needs attention.
    /// Task checkboxes use it for the small red indicator that mirrors the
    /// bell badge; scheduled reminders do not make a task look urgent.
    pub attention: bool,
    /// This occurrence has been handled and remains visible as history.
    /// Presentation strikes the schedule label without changing authored text.
    pub handled: bool,
}

/// An action chosen from one exact inline reminder pill's context menu.
///
/// The editor names intent and identity only. The host owns the cached rule,
/// Store mutation, and edit surface, so right-click input never performs I/O.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContextAction {
    Edit,
    Pause,
    Resume,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextRequest {
    pub node: NodeId,
    pub reminder_id: String,
    pub action: ContextAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemState {
    Active,
    Paused,
    WaitingForDue,
    Snoozed { until_ms: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub revision: String,
    /// Present when this row currently names an actionable occurrence.
    pub occurrence: Option<String>,
    pub draft: Draft,
    pub state: ItemState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryItem {
    pub reminder_id: String,
    pub occurrence: String,
    pub draft: Draft,
}

/// Phase-one global management filters. Recurrence is deliberately absent.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum GlobalFilter {
    #[default]
    All,
    NeedsAttention,
    ComingUp,
    Snoozed,
    Paused,
    History,
}

impl GlobalFilter {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::NeedsAttention,
        Self::ComingUp,
        Self::Snoozed,
        Self::Paused,
        Self::History,
    ];

    pub fn includes(self, state: GlobalItemState) -> bool {
        match self {
            Self::All => state != GlobalItemState::History,
            Self::NeedsAttention => state == GlobalItemState::NeedsAttention,
            Self::ComingUp => state == GlobalItemState::ComingUp,
            Self::Snoozed => state == GlobalItemState::Snoozed,
            Self::Paused => state == GlobalItemState::Paused,
            Self::History => state == GlobalItemState::History,
        }
    }
}

/// A row's already-resolved global grouping. Device delivery supplies
/// NeedsAttention; Store projection supplies the other synchronized states.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GlobalItemState {
    NeedsAttention,
    ComingUp,
    Snoozed,
    Paused,
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalItem {
    pub target: String,
    pub title: String,
    /// Cached root-first workspace/document location. A document row excludes
    /// its own title from the path because `title` already presents it.
    pub source_location: String,
    pub reminder_id: String,
    pub revision: String,
    pub occurrence: Option<String>,
    /// Resolved device-local ordering instant for this projected state. This
    /// is presentation metadata only; the authored schedule remains `draft`.
    pub sort_at_ms: Option<i64>,
    pub draft: Draft,
    pub state: GlobalItemState,
    /// The effective schedule is a snooze even after delivery changes its
    /// attention category. Its deadline remains in `sort_at_ms`.
    pub snoozed: bool,
    /// Authored rule status, distinct from an effective Paused state caused by
    /// the target being binned or temporarily unavailable.
    pub rule_paused: bool,
}

/// Device-local delivery identity used to refine a synchronized ComingUp row
/// into NeedsAttention. It carries no content and is never persisted here.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GlobalAttention {
    pub reminder_id: String,
    pub occurrence: String,
    /// The ledger occurrence's delivery instant, carried without authored
    /// content so the attention surface can show when it fired.
    pub fired_at_ms: i64,
}

/// Design-neutral state for the All Reminders overlay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalManagement {
    pub filter: GlobalFilter,
    pub rows: Vec<GlobalItem>,
    pub loading: bool,
    /// At least one workspace projection has not reached the requested sweep;
    /// absence is therefore not yet authoritative for that workspace.
    pub updating: bool,
    pub error: Option<Failure>,
    attention: BTreeSet<GlobalAttention>,
}

impl GlobalManagement {
    pub fn new(rows: Vec<GlobalItem>) -> Self {
        Self {
            rows,
            ..Self::default()
        }
    }

    /// Replace the authoritative profile rows without losing the selected
    /// view or device-local attention classifications.
    pub fn replace_rows(&mut self, rows: Vec<GlobalItem>, updating: bool) {
        self.rows = rows;
        self.loading = false;
        self.updating = updating;
        self.error = None;
        self.classify_attention();
    }

    pub fn begin_loading(&mut self) {
        self.loading = true;
        self.error = None;
    }

    /// Record projection catch-up without replacing the last authoritative
    /// rows. An updating snapshot can report targets missing or effectively
    /// paused only because its requested sweep has not finished yet.
    pub fn mark_updating(&mut self) {
        self.loading = false;
        self.updating = true;
        self.error = None;
    }

    pub fn load_failed(&mut self, reason: Failure) {
        self.loading = false;
        self.error = Some(reason);
    }

    /// Clear profile-owned data while retaining the person's filter choice.
    pub fn clear_profile(&mut self) {
        self.rows.clear();
        self.attention.clear();
        self.updating = false;
        self.error = None;
    }

    pub fn set_filter(&mut self, filter: GlobalFilter) -> bool {
        if self.filter == filter {
            return false;
        }
        self.filter = filter;
        true
    }

    pub fn set_needs_attention(
        &mut self,
        attention: impl IntoIterator<Item = GlobalAttention>,
    ) -> bool {
        let attention = attention.into_iter().collect();
        if self.attention == attention {
            return false;
        }
        self.attention = attention;
        self.classify_attention();
        true
    }

    pub fn visible(&self) -> impl Iterator<Item = &GlobalItem> {
        self.rows
            .iter()
            .filter(|row| self.filter.includes(row.state))
    }

    fn classify_attention(&mut self) {
        for row in &mut self.rows {
            if row.state == GlobalItemState::NeedsAttention {
                row.state = if row.snoozed {
                    GlobalItemState::Snoozed
                } else {
                    GlobalItemState::ComingUp
                };
            }
            if matches!(
                row.state,
                GlobalItemState::ComingUp | GlobalItemState::Snoozed
            ) && row.occurrence.as_ref().is_some_and(|occurrence| {
                self.attention.iter().any(|identity| {
                    identity.reminder_id == row.reminder_id
                        && identity.occurrence == *occurrence
                        && (!row.snoozed
                            || row
                                .sort_at_ms
                                .is_some_and(|until| identity.fired_at_ms >= until))
                })
            }) {
                row.state = GlobalItemState::NeedsAttention;
            }
        }
    }

    pub fn fired_at_ms(&self, row: &GlobalItem) -> Option<i64> {
        let occurrence = row.occurrence.as_ref()?;
        self.attention
            .iter()
            .find(|identity| {
                identity.reminder_id == row.reminder_id && identity.occurrence == *occurrence
            })
            .map(|identity| identity.fired_at_ms)
    }

    /// Reflect a chosen snooze immediately while the Store remains the
    /// authority that will replace this cache on the next profile load.
    /// Removing the matching device-local attention identity is part of the
    /// same presentation transition; otherwise `classify_attention` could
    /// immediately turn the optimistic Snoozed row back into NeedsAttention.
    pub fn snooze_local(&mut self, reminder_id: &str, occurrence: &str, until_ms: i64) -> bool {
        self.attention.retain(|identity| {
            identity.reminder_id != reminder_id || identity.occurrence != occurrence
        });
        let Some(row) = self.rows.iter_mut().find(|row| {
            row.reminder_id == reminder_id
                && row.occurrence.as_deref() == Some(occurrence)
                && row.state != GlobalItemState::History
        }) else {
            return false;
        };
        row.snoozed = true;
        row.state = GlobalItemState::Snoozed;
        row.sort_at_ms = Some(until_ms);
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagementIntent {
    Add(Draft),
    Edit {
        id: String,
        draft: Draft,
        /// The stored rule status, carried by the row so an edit confirmed
        /// before an asynchronous refresh cannot accidentally resume it.
        paused: bool,
    },
    Pause {
        id: String,
        revision: String,
    },
    Resume {
        id: String,
        revision: String,
    },
    Delete {
        id: String,
    },
    Snooze {
        id: String,
        occurrence: String,
        until_ms: i64,
    },
    AskClearHistory,
    ConfirmClearHistory,
    CancelClearHistory,
}

/// Store-independent projection for one entity's management surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Management {
    pub target: String,
    pub loading: bool,
    pub active: Vec<Item>,
    pub history: Vec<HistoryItem>,
    pub history_expanded: bool,
    pub clear_history_confirmation: bool,
    pub error: Option<Failure>,
}

impl Management {
    pub fn loading(target: String) -> Self {
        Self {
            target,
            loading: true,
            active: Vec::new(),
            history: Vec::new(),
            history_expanded: false,
            clear_history_confirmation: false,
            error: None,
        }
    }

    pub fn replace(&mut self, active: Vec<Item>, history: Vec<HistoryItem>) {
        self.loading = false;
        self.active = active;
        self.history = history;
        self.error = None;
    }

    pub fn summary(&self) -> ChipSummary {
        ChipSummary {
            active_count: self.active.len(),
            waiting: self
                .active
                .iter()
                .any(|item| item.state == ItemState::WaitingForDue),
            snoozed: self
                .active
                .iter()
                .any(|item| matches!(item.state, ItemState::Snoozed { .. })),
        }
    }

    pub fn toggle_history(&mut self) {
        self.history_expanded = !self.history_expanded;
    }

    pub fn apply_local(&mut self, intent: &ManagementIntent) {
        match intent {
            ManagementIntent::AskClearHistory => self.clear_history_confirmation = true,
            ManagementIntent::CancelClearHistory => self.clear_history_confirmation = false,
            ManagementIntent::ConfirmClearHistory => {
                self.clear_history_confirmation = false;
                self.loading = true;
            }
            _ => self.loading = true,
        }
    }
}

/// The exact selectable increments approved for phase one.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnoozeIncrement {
    OneMinute,
    FiveMinutes,
    TenMinutes,
    FifteenMinutes,
    ThirtyMinutes,
    OneHour,
}

impl SnoozeIncrement {
    pub const ALL: [Self; 6] = [
        Self::OneMinute,
        Self::FiveMinutes,
        Self::TenMinutes,
        Self::FifteenMinutes,
        Self::ThirtyMinutes,
        Self::OneHour,
    ];

    pub const fn minutes(self) -> u32 {
        match self {
            Self::OneMinute => 1,
            Self::FiveMinutes => 5,
            Self::TenMinutes => 10,
            Self::FifteenMinutes => 15,
            Self::ThirtyMinutes => 30,
            Self::OneHour => 60,
        }
    }
}

/// An accumulated snooze. Preview and commit are both grounded in an injected
/// current instant, so the durable value is exactly the final preview rather
/// than a time captured when the chooser happened to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snooze {
    counts: BTreeMap<SnoozeIncrement, u32>,
}

impl Snooze {
    pub fn new() -> Self {
        Self {
            counts: BTreeMap::new(),
        }
    }

    pub fn count(&self, increment: SnoozeIncrement) -> u32 {
        self.counts.get(&increment).copied().unwrap_or(0)
    }

    pub fn add(&mut self, increment: SnoozeIncrement) {
        let count = self.count(increment).saturating_add(1);
        self.counts.insert(increment, count);
    }

    pub fn subtract(&mut self, increment: SnoozeIncrement) {
        let count = self.count(increment).saturating_sub(1);
        if count == 0 {
            self.counts.remove(&increment);
        } else {
            self.counts.insert(increment, count);
        }
    }

    pub fn total_minutes(&self) -> Option<u32> {
        self.counts
            .iter()
            .try_fold(0u32, |total, (increment, count)| {
                increment
                    .minutes()
                    .checked_mul(*count)
                    .and_then(|part| total.checked_add(part))
            })
    }

    /// Checked all the way through. An overflowing preview is not confirmable.
    pub fn preview_ms(&self, now_ms: i64) -> Option<i64> {
        let minutes = i64::from(self.total_minutes()?);
        let delta = TimeDelta::minutes(minutes).num_milliseconds();
        now_ms.checked_add(delta)
    }

    pub fn can_confirm(&self, now_ms: i64) -> bool {
        self.total_minutes().is_some_and(|minutes| minutes > 0) && self.preview_ms(now_ms).is_some()
    }

    /// The caller injects the contemporaneous confirmation instant and can
    /// persist this exact answer.
    pub fn confirm(&self, now_ms: i64) -> Option<i64> {
        self.can_confirm(now_ms)
            .then(|| self.preview_ms(now_ms))
            .flatten()
    }
}

impl Default for Snooze {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
