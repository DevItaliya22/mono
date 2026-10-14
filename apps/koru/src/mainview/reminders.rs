// SPDX-License-Identifier: MPL-2.0

//! Serialized asynchronous adapter between reminder Store state and editor UI state.
//!
//! One profile reconciliation fetch supplies settings, global rows, chip
//! summaries, and the current entity's active rows/history. Store calls run
//! only on the worker; input, layout, drawing, and per-row paths only read
//! cached semantic state.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

use koru_core::NodeId;
use koru_editor::{
    reminders::{
        ChipSummary, Creation, CreationFocus, CreationState, Draft, Failure, GlobalAttention,
        GlobalFilter, GlobalItem, GlobalItemState, GlobalManagement, HistoryItem, Item, ItemState,
        Management, ManagementIntent, OptimisticCreation, OptimisticState, Request,
        RequestIdentity, RequestToken, Tone,
    },
    utils::when::{
        DueRelation as UiDueRelation, DurationExpression, DurationOrigin, DurationSign,
        DurationUnit, ReminderDraft, ResolutionContext, WeekStart,
    },
};
use koru_store_core::{
    ActionId, ActionKind, DueRelation, EffectiveState, EntityRef, OccurrenceAction, OccurrenceKey,
    OccurrenceMoment, OneTimeSchedule, ProfileId, ReminderId, ReminderRule, RuleRevisionId,
    RuleStatus, SemanticOffset, ToneId, WeekStartPreference,
};
use koru_store_wire::reminders::{ReminderSnapshot, ReminderSnapshotRow};

type Waker = Arc<dyn Fn() + Send + Sync>;

/// The Store has already spent 100 ms waiting for the requested projection
/// sweep. Give its worker a short clear lane before asking again; unlike the
/// ordinary event subscription this deadline exists only while Store has
/// explicitly reported a non-authoritative snapshot.
const PROJECTION_RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum RemoteScope {
    AllProfiles,
    Profile(ProfileId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub profile: ProfileId,
    pub entity: EntityRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnoozeRequest {
    pub reminder: ReminderId,
    pub occurrence: OccurrenceKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GlobalAction {
    Pause,
    Resume,
    Delete,
    Snooze { until_ms: i64 },
    Handled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlobalEditRequest {
    pub target: Target,
    pub row: GlobalItem,
}

/// Exact History handoff to the shared creation host. The consumer must feed
/// `source.schedule` through the ordinary passed-time resolution path before
/// presenting confirmation; this request never inserts a rule itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlobalRecreateRequest {
    pub target: Target,
    pub source: Draft,
    pub row: GlobalItem,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Mutation variants are the deferred visible surface's typed controller seam.
enum Work {
    Load,
    Create {
        rule: ReminderRule,
        trigger: Option<RequestIdentity>,
    },
    Update {
        rule: ReminderRule,
    },
    Pause {
        id: ReminderId,
        revision: RuleRevisionId,
    },
    Resume {
        id: ReminderId,
        revision: RuleRevisionId,
    },
    Delete {
        id: ReminderId,
    },
    Snooze {
        id: ReminderId,
        action: OccurrenceAction,
    },
    Handle {
        id: ReminderId,
        action: OccurrenceAction,
    },
    ClearHistoryForTarget,
    ClearAllHistory,
}

#[derive(Clone, Debug)]
struct Queued {
    target: Target,
    generation: u64,
    /// Present only for reads. Each invalidation supersedes every older load
    /// even when the visible target generation did not change.
    load_epoch: Option<u64>,
    work: Work,
}

#[derive(Clone, Debug)]
struct FailedMutation {
    queued: Queued,
    reason: Failure,
}

#[derive(Clone, Debug)]
struct OptimisticRecord {
    target: Target,
    context: ResolutionContext,
    default_tone: Option<Tone>,
    presentation: OptimisticCreation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Loaded {
    active: Vec<Item>,
    history: Vec<HistoryItem>,
    global_rows: Vec<GlobalItem>,
    global_updating: bool,
    summaries: std::collections::HashMap<String, ChipSummary>,
    end_of_day: Option<chrono::NaiveTime>,
    default_tone: Tone,
    week_start: WeekStartPreference,
    has_reminders: bool,
    present_ids: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProfileSettings {
    profile: ProfileId,
    end_of_day: Option<chrono::NaiveTime>,
    default_tone: Tone,
    week_start: WeekStartPreference,
}

#[derive(Debug)]
enum Answer {
    Loaded(Loaded),
    Mutated { trigger: Option<RequestIdentity> },
    Failed(Failure),
}

#[derive(Debug)]
struct StampedAnswer {
    target: Target,
    generation: u64,
    load_epoch: Option<u64>,
    answer: Answer,
}

#[derive(Default)]
pub(crate) struct Controller {
    client: Option<koru_api_client::Client>,
    target: Option<Target>,
    generation: u64,
    load_epoch: u64,
    pending: VecDeque<Queued>,
    in_flight: Option<Queued>,
    receiver: Option<Receiver<StampedAnswer>>,
    remote_changes: Arc<Mutex<HashSet<RemoteScope>>>,
    remote_settings_changes: Arc<Mutex<HashSet<RemoteScope>>>,
    subscription_result: Option<Receiver<koru_api_client::Subscription>>,
    subscription: Option<koru_api_client::Subscription>,
    subscription_started: bool,
    subscription_setup_stop: Option<Arc<AtomicBool>>,
    wake: Option<Waker>,
    pub creation: Option<Creation>,
    /// True only for a typed History Recreate draft. Its semantic state has
    /// already been reviewed and must not be replaced by parsing an empty
    /// synthetic phrase when the asynchronous profile load completes.
    creation_seeded: bool,
    pub management: Option<Management>,
    global: GlobalManagement,
    /// Notification-originated request awaiting the management surface's
    /// semantic duration choice. Never converted into a Store mutation until
    /// the ordinary `ManagementIntent::Snooze` path supplies `until_ms`.
    pending_snooze: Option<SnoozeRequest>,
    /// Exact-target handoff to the shared expanded reminder editor. The
    /// integration slice consumes this after navigation lands; Edit is never
    /// collapsed into Open or emitted as a no-op update.
    pending_global_edit: Option<GlobalEditRequest>,
    /// Capability-gated History handoff parallel to Edit. It survives target
    /// navigation until the registered creation host consumes it.
    pending_global_recreate: Option<GlobalRecreateRequest>,
    context: Option<ResolutionContext>,
    failed_mutation: Option<FailedMutation>,
    failed_creations: HashMap<RequestToken, FailedMutation>,
    failed_load: Option<Queued>,
    loaded_settings: Option<ProfileSettings>,
    has_reminders: Option<bool>,
    summaries: std::collections::HashMap<String, ChipSummary>,
    optimistic: Vec<OptimisticRecord>,
    projection_retry_at: Option<Instant>,
    /// A failed create reopened for editing keeps its client-minted id. A
    /// changed retry may be rejected if the original outcome was uncertain,
    /// but it can never mint a duplicate reminder.
    editing_retry: Option<(RequestToken, ReminderId)>,
    submitted_requests: VecDeque<(Target, RequestIdentity)>,
    cancelled_requests: VecDeque<(Target, RequestIdentity)>,
}

// Creation/management intent entry points intentionally precede their deferred
// visual host. Keeping them typed and tested here prevents that future layout
// from becoming a Store client or inventing behavior.
#[allow(dead_code)]
impl Controller {
    fn release_unsubmitted_creation(&mut self) {
        let retry = self.editing_retry.take();
        if let Some(creation) = self.creation.take()
            && retry.is_none()
            && let Some(target) = &self.target
        {
            self.cancelled_requests
                .push_back((target.clone(), creation.request.identity));
        }
        self.creation_seeded = false;
    }

    /// Cache a clone of the already-discovered client. This performs no I/O.
    pub fn set_client(&mut self, client: Option<koru_api_client::Client>) {
        self.client = client;
        self.ensure_event_subscription();
    }

    pub fn set_waker(&mut self, wake: Waker) {
        self.wake = Some(wake);
        self.ensure_event_subscription();
    }

    /// Present only after the current profile's reconciliation was accepted.
    pub fn has_reminders(&self) -> Option<bool> {
        self.has_reminders
    }

    /// The exact profile/entity context represented by object management and
    /// its creation child. Callers use this cached identity to dismiss chrome
    /// when the active document changes; it performs no Store work.
    pub fn current_target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    /// Apply the accepted profile settings to a freshly sampled editor
    /// context. Device clock/zone and the object's due anchor stay current;
    /// only Store-owned preferences come from the cached snapshot.
    pub fn editor_context(&self, mut context: ResolutionContext) -> (ResolutionContext, Tone) {
        let settings = self.loaded_settings.as_ref().filter(|settings| {
            self.target
                .as_ref()
                .is_some_and(|target| target.profile == settings.profile)
        });
        if let Some(settings) = settings {
            context.end_of_day = settings.end_of_day;
            context.week_start_override = week_start_override(settings.week_start);
        }
        let default_tone = settings.map_or(Tone::System, |settings| settings.default_tone);
        (context, default_tone)
    }

    /// Keep a controller-owned creation on the host's freshly sampled clock
    /// and zone. Advancing time alone affects the next phrase the user types,
    /// but does not churn an already resolved preview every maintenance pass.
    pub fn refresh_creation_context(
        &mut self,
        context: ResolutionContext,
        default_tone: Tone,
    ) -> bool {
        let semantic_changed = self.context.as_ref().is_none_or(|old| {
            old.language != context.language
                || old.date_order != context.date_order
                || old.current_zone != context.current_zone
                || old.selection != context.selection
                || old.end_of_day != context.end_of_day
                || old.due_anchor != context.due_anchor
                || old.system_week_start != context.system_week_start
                || old.week_start_override != context.week_start_override
        });
        self.context = Some(context);
        let (Some(creation), Some(context)) = (&mut self.creation, &self.context) else {
            return false;
        };
        if matches!(creation.state, CreationState::Saving(_)) {
            return false;
        }
        let Some(previous_tone) = creation.default_tone else {
            return false;
        };
        let tone_changed = previous_tone != default_tone;
        if !semantic_changed && !tone_changed {
            return false;
        }
        creation.request.due_anchor = context.due_anchor.clone();
        if self.creation_seeded {
            creation.review_typed(context, default_tone);
        } else {
            creation.resolve(context, default_tone);
        }
        true
    }

    pub fn close_management(&mut self) {
        self.release_unsubmitted_creation();
        self.management = None;
        self.pending_snooze = None;
        self.context = None;
    }

    /// Fetch one whole profile snapshot on profile entry. It supplies chips
    /// for every entity, including documents that have just reopened.
    pub fn ensure_profile(&mut self, target: Target) -> bool {
        if self
            .loaded_settings
            .as_ref()
            .is_some_and(|settings| settings.profile == target.profile)
            || self
                .target
                .as_ref()
                .is_some_and(|current| current.profile == target.profile)
        {
            return false;
        }
        self.release_unsubmitted_creation();
        self.target = Some(target);
        self.generation = self.generation.wrapping_add(1);
        self.management = None;
        self.global.clear_profile();
        self.pending_snooze = None;
        self.pending_global_edit = None;
        self.pending_global_recreate = None;
        self.context = None;
        self.summaries.clear();
        self.has_reminders = None;
        self.pending
            .retain(|queued| !matches!(queued.work, Work::Load));
        self.enqueue_current_load();
        true
    }

    /// Open one semantic creation request. Fetching Store settings and entity
    /// rows is deferred to the serialized worker.
    pub fn open(&mut self, target: Target, request: Request, context: ResolutionContext) {
        if let (Some(previous), Some(previous_target)) = (&self.creation, &self.target)
            && !matches!(
                previous.state,
                koru_editor::reminders::CreationState::Saving(_)
            )
        {
            // Superseding an uncommitted flow leaves its text ordinary, but
            // must release the old stable range just like explicit Cancel.
            self.cancelled_requests
                .push_back((previous_target.clone(), previous.request.identity));
        }
        self.target = Some(target.clone());
        self.generation = self.generation.wrapping_add(1);
        self.creation = Some(Creation::loading(request));
        self.creation_seeded = false;
        self.editing_retry = None;
        self.management = Some(Management::loading(target.entity.to_storage()));
        self.pending_snooze = None;
        self.context = Some(context);
        // A target change invalidates old reads, but never drops a mutation
        // whose durable outcome may still be unknown.
        self.pending
            .retain(|queued| !matches!(queued.work, Work::Load));
        self.enqueue_current_load();
    }

    /// Enter management for one validated native snooze gesture. This only
    /// changes cached controller state and queues the same authoritative load
    /// as an ordinary management open; it performs no Store or native call.
    pub fn open_snooze(&mut self, target: Target, reminder: ReminderId, occurrence: OccurrenceKey) {
        self.release_unsubmitted_creation();
        self.target = Some(target.clone());
        self.generation = self.generation.wrapping_add(1);
        self.management = Some(Management::loading(target.entity.to_storage()));
        self.context = None;
        self.pending_snooze = Some(SnoozeRequest {
            reminder,
            occurrence,
        });
        self.pending_global_edit = None;
        self.pending_global_recreate = None;
        self.pending
            .retain(|queued| !matches!(queued.work, Work::Load));
        self.enqueue_current_load();
    }

    /// Open the exact-object management projection. This queues only the
    /// ordinary asynchronous snapshot load; the visible surface reads cached
    /// state from [`Self::management`].
    pub fn open_management(&mut self, target: Target, context: ResolutionContext) {
        self.release_unsubmitted_creation();
        self.target = Some(target.clone());
        self.generation = self.generation.wrapping_add(1);
        self.management = Some(Management::loading(target.entity.to_storage()));
        self.pending_snooze = None;
        self.context = Some(context);
        self.pending
            .retain(|queued| !matches!(queued.work, Work::Load));
        self.enqueue_current_load();
    }

    /// Open a History row as a new reminder through CreationState review.
    /// The caller has already proved that navigation landed on this exact
    /// target; this method only adopts cached identity and queues an async load.
    pub fn open_recreate(
        &mut self,
        target: Target,
        node: NodeId,
        source: Draft,
        context: ResolutionContext,
    ) {
        self.release_unsubmitted_creation();
        let request = Request {
            identity: RequestIdentity {
                token: RequestToken::new(),
                node,
            },
            phrase: String::new(),
            due_anchor: context.due_anchor.clone(),
        };
        self.target = Some(target.clone());
        self.generation = self.generation.wrapping_add(1);
        self.creation = Some(Creation::reviewing(request, source, &context, Tone::System));
        self.creation_seeded = true;
        self.editing_retry = None;
        self.management = Some(Management::loading(target.entity.to_storage()));
        self.pending_snooze = None;
        self.context = Some(context);
        self.pending
            .retain(|queued| !matches!(queued.work, Work::Load));
        self.enqueue_current_load();
    }

    /// Re-resolve the visible natural-language field from cached settings.
    /// No Store call or document edit occurs here.
    pub fn update_creation_phrase(&mut self, phrase: String) -> bool {
        let (Some(creation), Some(context)) = (&mut self.creation, &self.context) else {
            return false;
        };
        if creation.request.phrase == phrase || matches!(creation.state, CreationState::Saving(_)) {
            return false;
        }
        // A typed History Recreate becomes an ordinary natural-language
        // creation as soon as the person edits its field. A late profile load
        // must therefore parse this phrase with the loaded end-of-day/default
        // settings instead of preserving the old semantic seed.
        self.creation_seeded = false;
        creation.request.phrase = phrase;
        if let Some(default_tone) = creation.default_tone {
            creation.resolve(context, default_tone);
        }
        true
    }

    /// Replace the structured one-time value from the expanded editor. This is
    /// cached semantic state; Save remains the only mutation boundary.
    pub fn replace_creation_schedule(&mut self, schedule: ReminderDraft) -> bool {
        let Some(creation) = &mut self.creation else {
            return false;
        };
        if matches!(creation.state, CreationState::Saving(_)) {
            return false;
        }
        creation.replace_draft(schedule);
        true
    }

    pub fn pending_snooze(&self) -> Option<&SnoozeRequest> {
        self.pending_snooze.as_ref()
    }

    pub fn cancel_snooze(&mut self) -> bool {
        self.pending_snooze.take().is_some()
    }

    pub fn begin_global_edit(&mut self, row: &GlobalItem) -> bool {
        let Some(current) = self.target.as_ref() else {
            return false;
        };
        let Some(entity) = EntityRef::parse(&row.target) else {
            return false;
        };
        if !self.global.rows.iter().any(|candidate| candidate == row) {
            return false;
        }
        self.pending_global_edit = Some(GlobalEditRequest {
            target: Target {
                profile: current.profile,
                entity,
            },
            row: row.clone(),
        });
        self.pending_global_recreate = None;
        true
    }

    pub fn take_global_edit(&mut self) -> Option<GlobalEditRequest> {
        self.pending_global_edit.take()
    }

    pub fn pending_global_edit_target(&self) -> Option<&Target> {
        self.pending_global_edit
            .as_ref()
            .map(|request| &request.target)
    }

    pub fn begin_global_recreate(&mut self, row: &GlobalItem) -> bool {
        let Some(current) = self.target.as_ref() else {
            return false;
        };
        let Some(entity) = EntityRef::parse(&row.target) else {
            return false;
        };
        if row.state != GlobalItemState::History
            || !self.global.rows.iter().any(|candidate| candidate == row)
        {
            return false;
        }
        self.pending_global_recreate = Some(GlobalRecreateRequest {
            target: Target {
                profile: current.profile,
                entity,
            },
            source: row.draft.clone(),
            row: row.clone(),
        });
        self.pending_global_edit = None;
        true
    }

    pub fn take_global_recreate(&mut self) -> Option<GlobalRecreateRequest> {
        self.pending_global_recreate.take()
    }

    pub fn pending_global_recreate_target(&self) -> Option<&Target> {
        self.pending_global_recreate
            .as_ref()
            .map(|request| &request.target)
    }

    pub fn cancel_creation(&mut self) -> bool {
        if self.creation.as_ref().is_some_and(|creation| {
            matches!(
                creation.state,
                koru_editor::reminders::CreationState::Saving(_)
            )
        }) {
            return false;
        }
        if let Some(creation) = self.creation.take() {
            let editing_retry = self.editing_retry.take();
            if self.failed_mutation.as_ref().is_some_and(|failed| {
                matches!(
                    failed.queued.work,
                    Work::Create {
                        trigger: Some(identity),
                        ..
                    } if identity == creation.request.identity
                )
            }) {
                self.failed_mutation = None;
            }
            if editing_retry.is_none()
                && let Some(target) = &self.target
            {
                self.cancelled_requests
                    .push_back((target.clone(), creation.request.identity));
            }
        }
        true
    }

    pub fn confirm_creation(&mut self, draft: Draft) {
        let (Some(target), Some(creation), Some(context)) = (
            self.target.clone(),
            self.creation.clone(),
            self.context.clone(),
        ) else {
            return;
        };
        let trigger = Some(creation.request.identity);
        let editing_retry = self
            .editing_retry
            .filter(|(token, _)| *token == creation.request.identity.token);
        if self.pending.iter().any(|queued| matches!(queued.work, Work::Create { trigger: queued_trigger, .. } if queued_trigger == trigger))
            || self.in_flight.as_ref().is_some_and(|queued| matches!(queued.work, Work::Create { trigger: queued_trigger, .. } if queued_trigger == trigger))
            || (editing_retry.is_none()
                && self.optimistic.iter().any(|record| record.presentation.request.identity == creation.request.identity))
        {
            return;
        }
        let id = editing_retry.map_or_else(ReminderId::new, |(_, id)| id);
        self.editing_retry = None;
        if editing_retry.is_some() {
            self.failed_creations
                .remove(&creation.request.identity.token);
            self.optimistic.retain(|record| {
                record.presentation.request.identity.token != creation.request.identity.token
            });
        }
        let Ok(rule) = rule(
            id,
            target.entity.clone(),
            draft.clone(),
            RuleStatus::Enabled,
        ) else {
            if let Some(creation) = &mut self.creation {
                creation.save_failed(Failure::InvalidData);
            }
            return;
        };
        let generation = self.generation;
        self.optimistic.push(OptimisticRecord {
            target: target.clone(),
            context,
            default_tone: creation.default_tone,
            presentation: OptimisticCreation {
                request: creation.request.clone(),
                target: target.entity.to_storage(),
                reminder_id: rule.id.to_storage(),
                draft,
                state: OptimisticState::Pending,
            },
        });
        self.submitted_requests
            .push_back((target.clone(), creation.request.identity));
        self.creation = None;
        self.creation_seeded = false;
        self.context = None;
        self.pending.push_back(Queued {
            target,
            generation,
            load_epoch: None,
            work: Work::Create { rule, trigger },
        });
    }

    pub fn apply_management(&mut self, intent: ManagementIntent) {
        if matches!(
            intent,
            ManagementIntent::AskClearHistory | ManagementIntent::CancelClearHistory
        ) {
            if let Some(management) = &mut self.management {
                management.apply_local(&intent);
            }
            return;
        }
        let optimistic_snooze = match &intent {
            ManagementIntent::Snooze {
                id,
                occurrence,
                until_ms,
            } => Some((id.clone(), occurrence.clone(), *until_ms)),
            _ => None,
        };
        let work = match intent.clone() {
            ManagementIntent::Add(draft) => {
                let Some(target) = self.target.as_ref() else {
                    return;
                };
                let Ok(rule) = rule(
                    ReminderId::new(),
                    target.entity.clone(),
                    draft,
                    RuleStatus::Enabled,
                ) else {
                    return;
                };
                Work::Create {
                    rule,
                    trigger: None,
                }
            }
            ManagementIntent::Edit { id, draft, paused } => {
                let status = if paused {
                    RuleStatus::Paused
                } else {
                    RuleStatus::Enabled
                };
                let Some(target) = self.target.as_ref() else {
                    return;
                };
                let Some(id) = ReminderId::parse(&id) else {
                    return;
                };
                let Ok(rule) = rule(id, target.entity.clone(), draft, status) else {
                    return;
                };
                Work::Update { rule }
            }
            ManagementIntent::Pause { id, .. } => {
                let Some(id) = ReminderId::parse(&id) else {
                    return;
                };
                Work::Pause {
                    id,
                    revision: RuleRevisionId::new(),
                }
            }
            ManagementIntent::Resume { id, .. } => {
                let Some(id) = ReminderId::parse(&id) else {
                    return;
                };
                Work::Resume {
                    id,
                    revision: RuleRevisionId::new(),
                }
            }
            ManagementIntent::Delete { id } => {
                let Some(id) = ReminderId::parse(&id) else {
                    return;
                };
                Work::Delete { id }
            }
            ManagementIntent::Snooze {
                id,
                occurrence,
                until_ms,
            } => {
                let Some(id) = ReminderId::parse(&id) else {
                    return;
                };
                let Some(occurrence) = OccurrenceKey::parse(&occurrence) else {
                    return;
                };
                Work::Snooze {
                    id,
                    action: OccurrenceAction {
                        id: ActionId::new(),
                        occurrence,
                        kind: ActionKind::SnoozedUntil {
                            instant_ms: until_ms,
                        },
                    },
                }
            }
            ManagementIntent::ConfirmClearHistory => Work::ClearHistoryForTarget,
            ManagementIntent::AskClearHistory | ManagementIntent::CancelClearHistory => {
                unreachable!("presentation-only history intent returned above")
            }
        };
        if let Some(management) = &mut self.management {
            management.apply_local(&intent);
        }
        if let Some((id, occurrence, until_ms)) = &optimistic_snooze {
            self.global.snooze_local(id, occurrence, *until_ms);
        }
        self.enqueue_current(work);
        if let ManagementIntent::Snooze { id, occurrence, .. } = intent
            && self.pending_snooze.as_ref().is_some_and(|pending| {
                pending.reminder.to_storage() == id && pending.occurrence.to_storage() == occurrence
            })
        {
            self.pending_snooze = None;
        }
    }

    /// Profile-wide history clearing is intentionally separate from the
    /// object Management intent. The global History surface calls this only
    /// after its own explicit confirmation.
    pub fn clear_all_history(&mut self) {
        self.enqueue_current(Work::ClearAllHistory);
    }

    pub fn take_submitted_request(&mut self) -> Option<(Target, RequestIdentity)> {
        self.submitted_requests.pop_front()
    }

    pub fn take_cancelled_request(&mut self) -> Option<(Target, RequestIdentity)> {
        self.cancelled_requests.pop_front()
    }

    pub fn summaries(&self) -> &std::collections::HashMap<String, ChipSummary> {
        &self.summaries
    }

    /// Stand the controller up over one entity with a known snapshot.
    ///
    /// **For the pure tests, and nothing else.** No client, no queue, no
    /// Store: the three fields a projection reads, set directly, because the
    /// alternative is standing a fake Store up to prove something about a
    /// semantic tree. The distinction the accessibility surface is *about* —
    /// authoritative rows versus submissions still in flight — is exactly the
    /// pair this seeds, so a test can put a reminder in one, in the other, or
    /// in both and see what a screen reader would be told about it.
    #[cfg(test)]
    pub(crate) fn seed_for_test(
        &mut self,
        target: Target,
        rows: Vec<GlobalItem>,
        optimistic: Vec<OptimisticCreation>,
    ) {
        self.global = GlobalManagement::new(rows);
        self.optimistic = optimistic
            .into_iter()
            .map(|presentation| OptimisticRecord {
                target: target.clone(),
                context: ResolutionContext {
                    now_ms: 0,
                    language: koru_editor::utils::when::Language::English,
                    date_order: koru_editor::utils::when::DateOrder::MonthFirst,
                    current_zone: koru_core::time::TimeZoneName::utc(),
                    selection: None,
                    end_of_day: None,
                    due_anchor: None,
                    system_week_start: koru_editor::utils::when::WeekStart::Monday,
                    week_start_override: None,
                },
                default_tone: Some(Tone::System),
                presentation,
            })
            .collect();
        self.target = Some(target);
    }

    /// Profile-global cached projection. Consumers may filter or draw this
    /// value without Store I/O or reference to the currently open entity.
    pub fn global(&self) -> &GlobalManagement {
        &self.global
    }

    pub fn set_global_filter(&mut self, filter: GlobalFilter) -> bool {
        self.global.set_filter(filter)
    }

    /// Apply content-free device delivery identities to the synchronized
    /// profile projection. This is local view state, not a remote-device control.
    pub fn set_global_attention(&mut self, attention: Vec<GlobalAttention>) -> bool {
        self.global.set_needs_attention(attention)
    }

    /// Queue an action against the row's exact entity without changing the
    /// active document or making the global surface a Store client.
    pub fn apply_global(&mut self, row: &GlobalItem, action: GlobalAction) -> bool {
        let Some(current) = self.target.as_ref() else {
            return false;
        };
        let Some(entity) = EntityRef::parse(&row.target) else {
            return false;
        };
        let Some(id) = ReminderId::parse(&row.reminder_id) else {
            return false;
        };
        // Refuse stale or fabricated presentation rows. A fresh projection can
        // replace a row between press and dispatch, and its revision wins.
        if !self.global.rows.iter().any(|candidate| candidate == row) {
            return false;
        }
        let optimistic_snooze = match &action {
            GlobalAction::Snooze { until_ms } => row
                .occurrence
                .as_ref()
                .map(|occurrence| (occurrence.clone(), *until_ms)),
            _ => None,
        };
        let is_snooze = optimistic_snooze.is_some();
        let work = match action {
            GlobalAction::Pause => Work::Pause {
                id,
                revision: RuleRevisionId::new(),
            },
            GlobalAction::Resume => Work::Resume {
                id,
                revision: RuleRevisionId::new(),
            },
            GlobalAction::Delete => Work::Delete { id },
            GlobalAction::Snooze { until_ms } => {
                let Some(occurrence) = row.occurrence.as_deref().and_then(OccurrenceKey::parse)
                else {
                    return false;
                };
                Work::Snooze {
                    id,
                    action: OccurrenceAction {
                        id: ActionId::new(),
                        occurrence,
                        kind: ActionKind::SnoozedUntil {
                            instant_ms: until_ms,
                        },
                    },
                }
            }
            GlobalAction::Handled => {
                let Some(occurrence) = row.occurrence.as_deref().and_then(OccurrenceKey::parse)
                else {
                    return false;
                };
                Work::Handle {
                    id,
                    action: OccurrenceAction {
                        id: ActionId::new(),
                        occurrence,
                        kind: ActionKind::Handled,
                    },
                }
            }
        };
        self.pending.push_back(Queued {
            target: Target {
                profile: current.profile,
                entity,
            },
            generation: self.generation,
            load_epoch: None,
            work,
        });
        if let Some((occurrence, until_ms)) = optimistic_snooze {
            self.global
                .snooze_local(&row.reminder_id, &occurrence, until_ms);
        }
        if is_snooze
            && self.pending_snooze.as_ref().is_some_and(|pending| {
                pending.reminder.to_storage() == row.reminder_id
                    && row.occurrence.as_deref() == Some(pending.occurrence.to_storage().as_str())
            })
        {
            self.pending_snooze = None;
        }
        true
    }

    pub fn creation_owner(&self) -> Option<(Target, RequestIdentity)> {
        Some((
            self.target.clone()?,
            self.creation.as_ref()?.request.identity,
        ))
    }

    pub fn optimistic_creations(&self) -> impl Iterator<Item = &OptimisticCreation> {
        let profile = self.target.as_ref().map(|target| target.profile);
        let editing = self.editing_retry.map(|(token, _)| token);
        self.optimistic
            .iter()
            .filter(move |record| {
                Some(record.target.profile) == profile
                    && Some(record.presentation.request.identity.token) != editing
            })
            .map(|record| &record.presentation)
    }

    pub fn retry_creation(&mut self, token: RequestToken) -> bool {
        let Some(failed) = self.failed_creations.remove(&token) else {
            return false;
        };
        if let Some(record) = self
            .optimistic
            .iter_mut()
            .find(|record| record.presentation.request.identity.token == token)
        {
            record.presentation.state = OptimisticState::Pending;
        }
        self.pending.push_front(failed.queued);
        true
    }

    pub fn edit_failed_creation(&mut self, token: RequestToken) -> bool {
        if !self.failed_creations.contains_key(&token) {
            return false;
        }
        let Some(record) = self
            .optimistic
            .iter()
            .find(|record| record.presentation.request.identity.token == token)
            .cloned()
        else {
            return false;
        };
        let failed = self.failed_creations.get(&token).unwrap();
        let reminder = match &failed.queued.work {
            Work::Create { rule, .. } => rule.id,
            _ => return false,
        };
        if let (Some(previous), Some(previous_target)) = (&self.creation, &self.target) {
            self.cancelled_requests
                .push_back((previous_target.clone(), previous.request.identity));
        }
        self.target = Some(record.target.clone());
        self.generation = self.generation.wrapping_add(1);
        let tone_override = record.presentation.draft.tone_override;
        self.creation = Some(Creation {
            request: record.presentation.request,
            state: CreationState::Confirmation(record.presentation.draft),
            focus: CreationFocus::Editor,
            default_tone: record.default_tone,
            tone_override,
        });
        self.creation_seeded = false;
        self.editing_retry = Some((token, reminder));
        self.context = Some(record.context);
        self.management = Some(Management::loading(record.target.entity.to_storage()));
        self.enqueue_current_load();
        true
    }

    pub fn dismiss_failed_creation(&mut self, token: RequestToken) -> bool {
        if self.failed_creations.remove(&token).is_none() {
            return false;
        }
        let before = self.optimistic.len();
        self.optimistic
            .retain(|record| record.presentation.request.identity.token != token);
        self.optimistic.len() != before
    }

    /// Re-resolve an editable creation when its source due changes. A Saving
    /// creation is immutable: the confirmed Store rule already follows due.
    pub fn refresh_due_anchor(
        &mut self,
        owner: &Target,
        identity: RequestIdentity,
        due_anchor: Option<koru_core::time::Temporal>,
    ) -> bool {
        let (Some(creation), Some(context)) = (&mut self.creation, &mut self.context) else {
            return false;
        };
        if self.target.as_ref() != Some(owner)
            || creation.request.identity != identity
            || creation.request.due_anchor == due_anchor
            || matches!(
                creation.state,
                koru_editor::reminders::CreationState::Saving(_)
            )
        {
            return false;
        }
        creation.request.due_anchor = due_anchor.clone();
        context.due_anchor = due_anchor;
        if let Some(default_tone) = creation.default_tone {
            if self.creation_seeded {
                creation.review_typed(context, default_tone);
            } else {
                creation.resolve(context, default_tone);
            }
        }
        true
    }

    pub fn failed_owner(&self) -> Option<(Target, Failure)> {
        self.failed_mutation
            .as_ref()
            .map(|failed| (failed.queued.target.clone(), failed.reason))
            .or_else(|| {
                self.failed_load.as_ref().map(|queued| {
                    (
                        queued.target.clone(),
                        self.management
                            .as_ref()
                            .and_then(|management| management.error)
                            .unwrap_or(Failure::Unavailable),
                    )
                })
            })
    }

    pub fn retry_failed(&mut self, owner: &Target) -> bool {
        let queued = if self
            .failed_mutation
            .as_ref()
            .is_some_and(|failed| &failed.queued.target == owner)
        {
            self.failed_mutation.take().map(|failed| failed.queued)
        } else if self
            .failed_load
            .as_ref()
            .is_some_and(|queued| &queued.target == owner)
        {
            self.failed_load.take()
        } else {
            None
        };
        let Some(queued) = queued else {
            return false;
        };
        if Some(&queued.target) == self.target.as_ref() && queued.generation == self.generation {
            if let Some(creation) = &mut self.creation {
                match &queued.work {
                    Work::Load
                        if matches!(
                            creation.state,
                            koru_editor::reminders::CreationState::LoadFailed { .. }
                        ) =>
                    {
                        creation.state = koru_editor::reminders::CreationState::Loading;
                    }
                    Work::Create {
                        rule,
                        trigger: Some(_),
                    } => {
                        if let Ok(schedule) = schedule_from_store(&rule.schedule) {
                            creation.state =
                                koru_editor::reminders::CreationState::Saving(Draft::preserving(
                                    schedule,
                                    rule.tone_override.map(tone_from_store),
                                ));
                        }
                    }
                    _ => {}
                }
            }
            if let Some(management) = &mut self.management {
                management.loading = true;
                management.error = None;
            }
        }
        self.pending.push_front(queued);
        true
    }

    /// Explicitly dismiss a failed management mutation so later queued intent
    /// can proceed. Creation cancellation uses the narrower generation-safe
    /// path above.
    pub fn discard_failed(&mut self, owner: &Target) -> bool {
        let discarded = if self
            .failed_mutation
            .as_ref()
            .is_some_and(|failed| &failed.queued.target == owner)
        {
            self.failed_mutation = None;
            true
        } else if self
            .failed_load
            .as_ref()
            .is_some_and(|queued| &queued.target == owner)
        {
            self.failed_load = None;
            true
        } else {
            false
        };
        if discarded && self.target.as_ref() == Some(owner) {
            if let Some(management) = &mut self.management {
                management.loading = false;
                management.error = None;
            }
        }
        discarded
    }

    pub fn pending(&self) -> bool {
        self.receiver.is_some() || !self.pending.is_empty()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.projection_retry_at
    }

    pub fn maintain(&mut self) -> bool {
        let mut changed = false;
        if self
            .projection_retry_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.projection_retry_at = None;
            self.enqueue_current_load();
            changed = true;
        }
        if let Some(receiver) = &self.subscription_result
            && let Ok(result) = receiver.try_recv()
        {
            self.subscription_result = None;
            self.subscription = Some(result);
            // Strong refresh after the live tail is active closes both setup
            // races: if the initial snapshot won, this sees any change before
            // head capture; if the tail won, its event also coalesces here.
            if let Some(profile) = self.target.as_ref().map(|target| target.profile) {
                changed |= self.invalidate_profile(profile);
            }
        }
        let remote_profiles = self
            .remote_changes
            .lock()
            .map(|mut profiles| profiles.drain().collect::<Vec<_>>())
            .unwrap_or_default();
        for scope in remote_profiles {
            let profile = match scope {
                RemoteScope::AllProfiles => self.target.as_ref().map(|target| target.profile),
                RemoteScope::Profile(profile) => Some(profile),
            };
            if let Some(profile) = profile {
                changed |= self.invalidate_profile(profile);
            }
        }
        if let Some(receiver) = &self.receiver
            && let Ok(stamped) = receiver.try_recv()
        {
            self.receiver = None;
            let completed = self.in_flight.take();
            if let Answer::Failed(reason) = &stamped.answer
                && let Some(queued) = completed
            {
                // Create failures belong to their request-token pill and must
                // not serialize unrelated creates behind management failure.
                self.retain_failure(queued, *reason);
            }
            changed |= self.apply_stamped(stamped);
        }
        // A failed mutation serializes later mutations until explicit retry or
        // dismissal, but never blocks an unrelated authoritative read.
        if self.receiver.is_none() {
            changed |= self.start_next();
        }
        changed
    }

    /// Consume remote Profile-settings changes noticed by the shared Store
    /// event tail. The Settings controller uses this signal to reload its
    /// authoritative snapshot; it deliberately owns no second subscription.
    pub fn take_settings_change(&self, current: Option<ProfileId>) -> bool {
        self.remote_settings_changes
            .lock()
            .map(|mut changes| {
                changes.drain().any(|scope| match scope {
                    RemoteScope::AllProfiles => current.is_some(),
                    RemoteScope::Profile(profile) => current == Some(profile),
                })
            })
            .unwrap_or(false)
    }

    fn apply_stamped(&mut self, stamped: StampedAnswer) -> bool {
        if stamped.load_epoch.is_some() && stamped.load_epoch != Some(self.load_epoch) {
            return false;
        }
        // A mutation may have become durable even if the user changed target
        // while its response was in flight. Update only its request-token pill;
        // the exact command text was already removed at submission.
        if let Answer::Mutated {
            trigger: Some(identity),
        } = &stamped.answer
        {
            self.failed_creations.remove(&identity.token);
            if let Some(record) = self
                .optimistic
                .iter_mut()
                .find(|record| record.presentation.request.identity == *identity)
            {
                record.presentation.state = OptimisticState::Succeeded;
            }
        }
        let mutated = matches!(stamped.answer, Answer::Mutated { .. });
        if mutated
            && self
                .target
                .as_ref()
                .is_some_and(|target| target.profile == stamped.target.profile)
        {
            // One profile snapshot covers every chip. This also reconciles a
            // mutation whose UI target changed while its response was away.
            self.enqueue_current_load();
        }
        if Some(&stamped.target) != self.target.as_ref() || stamped.generation != self.generation {
            return false;
        }
        match stamped.answer {
            Answer::Loaded(loaded) => {
                let projection_updating = loaded.global_updating;
                self.failed_load = None;
                let retained_failure = self
                    .failed_mutation
                    .as_ref()
                    .filter(|failed| failed.queued.target == stamped.target);
                let retained_reason = retained_failure.map(|failed| failed.reason);
                let retained_creation = retained_failure.is_some_and(|failed| {
                    matches!(
                        failed.queued.work,
                        Work::Create {
                            trigger: Some(identity),
                            ..
                        } if self.creation.as_ref().is_some_and(|creation| creation.request.identity == identity)
                    )
                });
                if let Some(context) = &mut self.context {
                    context.end_of_day = loaded.end_of_day;
                    context.week_start_override = week_start_override(loaded.week_start);
                    if !retained_creation && let Some(creation) = &mut self.creation {
                        if self.creation_seeded {
                            creation.default_tone = Some(loaded.default_tone);
                        } else {
                            creation.resolve(context, loaded.default_tone);
                        }
                    }
                }
                self.loaded_settings = Some(ProfileSettings {
                    profile: stamped.target.profile,
                    end_of_day: loaded.end_of_day,
                    default_tone: loaded.default_tone,
                    week_start: loaded.week_start,
                });
                self.has_reminders = Some(loaded.has_reminders);
                self.summaries = loaded.summaries;
                if let Some(management) = &mut self.management {
                    management.replace(loaded.active, loaded.history);
                    management.error = retained_reason;
                }
                if projection_updating {
                    self.global.mark_updating();
                } else {
                    self.global.replace_rows(loaded.global_rows, false);
                }
                self.projection_retry_at =
                    projection_updating.then(|| Instant::now() + PROJECTION_RETRY_DELAY);
                // The exact client-minted id is authoritative even when the
                // create response was lost. Reconcile pending and failed pills,
                // and remove their retry state before it can issue a conflict.
                let present_ids = &loaded.present_ids;
                self.failed_creations.retain(|_, failed| {
                    !matches!(
                        &failed.queued.work,
                        Work::Create { rule, .. }
                            if present_ids.contains(&rule.id.to_storage())
                    )
                });
                self.optimistic.retain_mut(|record| {
                    if !present_ids.contains(&record.presentation.reminder_id) {
                        return true;
                    }
                    // Store presence confirms a lost create response, but an
                    // updating projection has not published replacement rows.
                    // Keep the saved pill until that same publication can take
                    // over; retiring it earlier leaves neither source visible.
                    record.presentation.state = OptimisticState::Succeeded;
                    projection_updating
                });
                true
            }
            Answer::Mutated { trigger } => {
                let _ = trigger;
                true
            }
            Answer::Failed(reason) => {
                if stamped.load_epoch.is_some() {
                    self.global.load_failed(reason);
                }
                let creation_failed = self.failed_creations.values().any(|failed| {
                    failed.queued.target == stamped.target
                        && failed.queued.generation == stamped.generation
                });
                if let Some(creation) = &mut self.creation {
                    if matches!(
                        creation.state,
                        koru_editor::reminders::CreationState::Loading
                    ) {
                        creation.load_failed(reason);
                    }
                }
                if !creation_failed && let Some(management) = &mut self.management {
                    management.loading = false;
                    management.error = Some(reason);
                }
                true
            }
        }
    }

    fn start_next(&mut self) -> bool {
        let Some(queued) = self.dequeue_next() else {
            return false;
        };
        let Some(client) = self.client.clone() else {
            self.retain_failure(queued.clone(), Failure::Unavailable);
            return self.apply_stamped(StampedAnswer {
                target: queued.target,
                generation: queued.generation,
                load_epoch: queued.load_epoch,
                answer: Answer::Failed(Failure::Unavailable),
            });
        };
        let generation = queued.generation;
        let load_epoch = queued.load_epoch;
        let worker_work = queued.work.clone();
        let worker_target = queued.target.clone();
        let wake = self.wake.clone();
        let (send, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("koru-reminder-ui".into())
            .spawn(move || {
                let answer =
                    run(&client, &worker_target, worker_work).unwrap_or_else(Answer::Failed);
                deliver(
                    &send,
                    wake.as_ref(),
                    StampedAnswer {
                        target: worker_target,
                        generation,
                        load_epoch,
                        answer,
                    },
                );
            }) {
            Ok(_) => {
                self.in_flight = Some(queued);
                self.receiver = Some(receiver);
                false
            }
            Err(_error) => {
                self.retain_failure(queued.clone(), Failure::Unavailable);
                self.apply_stamped(StampedAnswer {
                    target: queued.target,
                    generation,
                    load_epoch,
                    answer: Answer::Failed(Failure::Unavailable),
                })
            }
        }
    }

    fn dequeue_next(&mut self) -> Option<Queued> {
        if let Some(failed) = &self.failed_mutation {
            // An uncertain mutation serializes later writes for the same
            // object, not unrelated work after the person changes documents.
            // The failed owner remains retryable/discardable when revisited.
            self.pending
                .iter()
                .position(|queued| {
                    queued.target != failed.queued.target || matches!(queued.work, Work::Load)
                })
                .and_then(|index| self.pending.remove(index))
        } else {
            self.pending.pop_front()
        }
    }

    fn enqueue_current(&mut self, work: Work) {
        let Some(target) = self.target.clone() else {
            return;
        };
        self.pending.push_back(Queued {
            target,
            generation: self.generation,
            load_epoch: None,
            work,
        });
    }

    fn enqueue_current_load(&mut self) {
        let Some(target) = self.target.clone() else {
            return;
        };
        self.load_epoch = self.load_epoch.wrapping_add(1).max(1);
        self.projection_retry_at = None;
        self.failed_load = None;
        self.global.begin_loading();
        self.pending.retain(|queued| {
            !(queued.target == target
                && queued.generation == self.generation
                && matches!(queued.work, Work::Load))
        });
        self.pending.push_back(Queued {
            target,
            generation: self.generation,
            load_epoch: Some(self.load_epoch),
            work: Work::Load,
        });
    }

    fn retain_failure(&mut self, queued: Queued, reason: Failure) {
        if matches!(queued.work, Work::Load) {
            if Some(&queued.target) == self.target.as_ref()
                && queued.generation == self.generation
                && queued.load_epoch == Some(self.load_epoch)
            {
                self.failed_load = Some(queued);
            }
        } else if let Work::Create {
            trigger: Some(identity),
            ..
        } = queued.work
        {
            if let Some(record) = self
                .optimistic
                .iter_mut()
                .find(|record| record.presentation.request.identity == identity)
            {
                record.presentation.state = OptimisticState::Failed { reason };
            }
            self.failed_creations
                .insert(identity.token, FailedMutation { queued, reason });
        } else if self.failed_mutation.is_none() {
            self.failed_mutation = Some(FailedMutation { queued, reason });
        }
    }

    /// Content-free Store events invalidate one authoritative profile batch.
    /// The next maintenance pass queues a reconciliation; it never reads Store.
    fn invalidate_profile(&mut self, profile: ProfileId) -> bool {
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.profile != profile)
        {
            return false;
        }
        self.loaded_settings = None;
        self.has_reminders = None;
        self.enqueue_current_load();
        true
    }

    fn ensure_event_subscription(&mut self) {
        if self.subscription_started || self.subscription.is_some() {
            return;
        }
        let (Some(client), Some(wake)) = (self.client.clone(), self.wake.clone()) else {
            return;
        };
        if !client.events().available() {
            return;
        }
        let (result_send, result_receive) = mpsc::channel();
        self.subscription_result = Some(result_receive);
        self.subscription_started = true;
        let remote_changes = self.remote_changes.clone();
        let remote_settings_changes = self.remote_settings_changes.clone();
        let setup_wake = wake.clone();
        let setup_stop = Arc::new(AtomicBool::new(false));
        self.subscription_setup_stop = Some(setup_stop.clone());
        if std::thread::Builder::new()
            .name("koru-reminder-events-setup".into())
            .spawn(move || {
                let subscription = retry_with_backoff(
                    &setup_stop,
                    || {
                        let event_wake = wake.clone();
                        let remote_changes = remote_changes.clone();
                        let remote_settings_changes = remote_settings_changes.clone();
                        client.events().on(
                            // Capture the live head before reconciliation is
                            // serviced: the later snapshot includes pre-head
                            // state and this tail catches post-head changes.
                            // Replaying from zero would make startup O(history).
                            None,
                            koru_api_client::Filter::all(),
                            move |event| {
                                deliver_remote_event(
                                    &remote_changes,
                                    &remote_settings_changes,
                                    Some(&event_wake),
                                    event.envelope.name.as_str(),
                                    event
                                        .envelope
                                        .scope
                                        .profile_id
                                        .map(|profile| ProfileId::from_uuid(profile.0)),
                                );
                            },
                        )
                    },
                    std::thread::sleep,
                );
                if let Some(subscription) = subscription
                    && result_send.send(subscription).is_ok()
                {
                    setup_wake();
                }
            })
            .is_err()
        {
            self.subscription_started = false;
            self.subscription_result = None;
            self.subscription_setup_stop = None;
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(stop) = &self.subscription_setup_stop {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

fn retry_with_backoff<T, E>(
    stop: &AtomicBool,
    mut attempt: impl FnMut() -> Result<T, E>,
    mut wait: impl FnMut(Duration),
) -> Option<T> {
    let mut delay = Duration::from_millis(250);
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        match attempt() {
            Ok(value) => return Some(value),
            Err(_) => {
                wait(delay);
                delay = (delay * 2).min(Duration::from_secs(5));
            }
        }
    }
}

fn deliver(send: &Sender<StampedAnswer>, wake: Option<&Waker>, answer: StampedAnswer) {
    if send.send(answer).is_ok()
        && let Some(wake) = wake
    {
        wake();
    }
}

fn reminder_invalidation_scope(name: &str, profile: Option<ProfileId>) -> Option<RemoteScope> {
    match name {
        "reminder.rule.changed"
        | "reminder.action.appended"
        | "reminder.history.cleared"
        | "reminder.settings.changed" => {
            Some(profile.map_or(RemoteScope::AllProfiles, RemoteScope::Profile))
        }
        // These change the source or liveness of relative reminder rows. They
        // are content-free global wakes; reconciliation remains the truth.
        "due.changed" | "entity.removed" | "document.created" | "document.trashed"
        | "document.restored" | "document.removed" | "workspace.created" | "workspace.deleted"
        | "workspace.observed" => Some(RemoteScope::AllProfiles),
        _ => None,
    }
}

#[cfg(test)]
fn deliver_remote_change(
    changes: &Mutex<HashSet<RemoteScope>>,
    wake: Option<&Waker>,
    scope: RemoteScope,
) {
    if let Ok(mut changes) = changes.lock()
        && changes.insert(scope)
        && let Some(wake) = wake
    {
        wake();
    }
}

fn deliver_remote_event(
    changes: &Mutex<HashSet<RemoteScope>>,
    settings_changes: &Mutex<HashSet<RemoteScope>>,
    wake: Option<&Waker>,
    name: &str,
    profile: Option<ProfileId>,
) {
    let Some(scope) = reminder_invalidation_scope(name, profile) else {
        return;
    };
    let mut changed = changes
        .lock()
        .map(|mut changes| changes.insert(scope))
        .unwrap_or(false);
    if name == "reminder.settings.changed" {
        changed |= settings_changes
            .lock()
            .map(|mut changes| changes.insert(scope))
            .unwrap_or(false);
    }
    if changed && let Some(wake) = wake {
        wake();
    }
}

fn run(client: &koru_api_client::Client, target: &Target, work: Work) -> Result<Answer, Failure> {
    match work {
        Work::Load => client
            .store()
            .reminder_reconciliation(target.profile)
            .map_err(store_failure)
            .and_then(|snapshot| project(snapshot, &target.entity).map(Answer::Loaded)),
        Work::Create { rule, trigger } => {
            client
                .store()
                .create_reminder(target.profile, rule)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger })
        }
        Work::Update { rule } => {
            let id = rule.id;
            client
                .store()
                .update_reminder(target.profile, id, rule)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
        Work::Pause { id, revision } => {
            client
                .store()
                .pause_reminder(target.profile, id, revision)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
        Work::Resume { id, revision } => {
            client
                .store()
                .resume_reminder(target.profile, id, revision)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
        Work::Delete { id } => {
            client
                .store()
                .delete_reminder(target.profile, id)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
        Work::Snooze { id, action } => {
            client
                .store()
                .snooze_reminder(target.profile, id, action)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
        Work::Handle { id, action } => {
            client
                .store()
                .handle_reminder(target.profile, id, action)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
        Work::ClearHistoryForTarget => {
            client
                .store()
                .clear_reminder_history_for_target(target.profile, &target.entity)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
        Work::ClearAllHistory => {
            client
                .store()
                .clear_reminder_history(target.profile)
                .map_err(store_failure)?;
            Ok(Answer::Mutated { trigger: None })
        }
    }
}

fn store_failure(error: koru_store_core::errors::Error) -> Failure {
    use koru_store_core::errors::Code;

    match error.code() {
        Code::InvalidRequest | Code::InvalidId | Code::InvalidLocale => Failure::InvalidData,
        Code::StoreUnreachable
        | Code::StorageFailed
        | Code::ProjectionUnavailable
        | Code::Internal => Failure::Unavailable,
        _ => Failure::Rejected,
    }
}

#[allow(dead_code)] // Used by the deferred visible creation surface through Controller.
fn rule(
    id: ReminderId,
    target: EntityRef,
    draft: Draft,
    status: RuleStatus,
) -> Result<ReminderRule, Failure> {
    Ok(ReminderRule {
        id,
        target,
        revision: RuleRevisionId::new(),
        status,
        schedule: schedule_to_store(&draft.schedule)?,
        tone_override: draft.tone_override.map(tone_to_store),
    })
}

fn project(snapshot: ReminderSnapshot, target: &EntityRef) -> Result<Loaded, Failure> {
    let global_updating = !snapshot.updating_workspaces.is_empty();
    let present_ids = snapshot
        .reminders
        .iter()
        .map(|row| row.stored.rule.id.to_storage())
        .collect();
    let has_reminders = snapshot
        .reminders
        .iter()
        .any(|row| stored_reminder_is_present(row.stored.deleted, row.stored.rule.status));
    let mut active = Vec::new();
    let mut history = Vec::new();
    let mut global_rows = Vec::new();
    let mut summaries = std::collections::HashMap::<String, ChipSummary>::new();
    for row in &snapshot.reminders {
        let live = !matches!(
            row.reduced.effective,
            EffectiveState::Deleted | EffectiveState::Handled { .. }
        );
        if !live {
            continue;
        }
        let summary = summaries
            .entry(row.stored.rule.target.to_storage())
            .or_default();
        summary.active_count += 1;
        summary.waiting |= matches!(row.reduced.effective, EffectiveState::WaitingForDue);
        summary.snoozed |= matches!(row.reduced.effective, EffectiveState::Snoozed { .. });
    }
    for row in snapshot.reminders {
        project_global_row(&row, &mut global_rows)?;
        if row.stored.rule.target == *target {
            project_row(row, &mut active, &mut history)?;
        }
    }
    let end_of_day = snapshot
        .settings
        .end_of_day
        .and_then(|time| chrono::NaiveTime::parse_from_str(&time.to_storage(), "%H:%M:%S").ok());
    Ok(Loaded {
        active,
        history,
        global_rows,
        global_updating,
        summaries,
        end_of_day,
        default_tone: tone_from_store(snapshot.settings.default_tone),
        week_start: snapshot.settings.week_start,
        has_reminders,
        present_ids,
    })
}

fn project_global_row(
    row: &ReminderSnapshotRow,
    rows: &mut Vec<GlobalItem>,
) -> Result<(), Failure> {
    // A deleted rule keeps its handled occurrences in the reduction, but it is
    // gone: projecting them as History rows would redraw the exact pill the
    // delete just removed, which presented as Delete doing nothing.
    if matches!(row.reduced.effective, EffectiveState::Deleted) {
        return Ok(());
    }
    let draft = Draft::preserving(
        schedule_from_store(&row.stored.rule.schedule)?,
        row.stored.rule.tone_override.map(tone_from_store),
    );
    let common = |occurrence: Option<String>, sort_at_ms: Option<i64>, state| GlobalItem {
        target: row.stored.rule.target.to_storage(),
        title: row.target.title.clone(),
        source_location: row.target.source_location.clone(),
        reminder_id: row.stored.rule.id.to_storage(),
        revision: row.stored.rule.revision.to_storage(),
        occurrence,
        sort_at_ms,
        draft: draft.clone(),
        state,
        snoozed: state != GlobalItemState::History
            && matches!(&row.reduced.effective, EffectiveState::Snoozed { .. }),
        rule_paused: row.stored.rule.status == RuleStatus::Paused,
    };
    for occurrence in &row.reduced.handled_history {
        rows.push(common(
            Some(occurrence.to_storage()),
            None,
            GlobalItemState::History,
        ));
    }
    let (state, occurrence, sort_at_ms) = match &row.reduced.effective {
        EffectiveState::Deleted | EffectiveState::Handled { .. } => return Ok(()),
        EffectiveState::Paused(_) => (GlobalItemState::Paused, None, None),
        EffectiveState::WaitingForDue => (GlobalItemState::ComingUp, None, None),
        EffectiveState::Scheduled { occurrence, moment } => {
            let sort_at_ms = match moment {
                OccurrenceMoment::Zoned { value } => Some(value.instant_ms()),
                OccurrenceMoment::Floating { .. } => None,
            };
            (
                GlobalItemState::ComingUp,
                Some(occurrence.to_storage()),
                sort_at_ms,
            )
        }
        EffectiveState::Snoozed {
            occurrence,
            until_ms,
        } => (
            GlobalItemState::Snoozed,
            Some(occurrence.to_storage()),
            Some(*until_ms),
        ),
    };
    rows.push(common(occurrence, sort_at_ms, state));
    Ok(())
}

fn stored_reminder_is_present(deleted: bool, status: RuleStatus) -> bool {
    !deleted && status != RuleStatus::Deleted
}

fn project_row(
    row: ReminderSnapshotRow,
    active: &mut Vec<Item>,
    history: &mut Vec<HistoryItem>,
) -> Result<(), Failure> {
    // Same rule as the global projection: a deleted rule's occurrences are
    // not history of anything still visible.
    if matches!(row.reduced.effective, EffectiveState::Deleted) {
        return Ok(());
    }
    let draft = Draft::preserving(
        schedule_from_store(&row.stored.rule.schedule)?,
        row.stored.rule.tone_override.map(tone_from_store),
    );
    for occurrence in &row.reduced.handled_history {
        history.push(HistoryItem {
            reminder_id: row.stored.rule.id.to_storage(),
            occurrence: occurrence.to_storage(),
            draft: draft.clone(),
        });
    }
    let (state, occurrence) = match row.reduced.effective {
        EffectiveState::Deleted | EffectiveState::Handled { .. } => return Ok(()),
        EffectiveState::Paused(_) => (ItemState::Paused, None),
        EffectiveState::WaitingForDue => (ItemState::WaitingForDue, None),
        EffectiveState::Scheduled { occurrence, .. } => {
            (ItemState::Active, Some(occurrence.to_storage()))
        }
        EffectiveState::Snoozed {
            occurrence,
            until_ms,
        } => (
            ItemState::Snoozed { until_ms },
            Some(occurrence.to_storage()),
        ),
    };
    active.push(Item {
        id: row.stored.rule.id.to_storage(),
        revision: row.stored.rule.revision.to_storage(),
        occurrence,
        draft,
        state,
    });
    Ok(())
}

#[allow(dead_code)] // Used by the deferred visible creation surface through Controller.
fn schedule_to_store(schedule: &ReminderDraft) -> Result<OneTimeSchedule, Failure> {
    Ok(match schedule {
        ReminderDraft::Floating(wall) => OneTimeSchedule::Floating {
            wall: koru_store_core::temporal::FloatingDateTime::parse(&wall.to_storage())
                .ok_or(Failure::InvalidData)?,
        },
        ReminderDraft::Zoned(value) => OneTimeSchedule::Zoned {
            value: koru_store_core::temporal::ZonedDateTime::new(
                value.instant_ms(),
                value.zone().as_str(),
            )
            .map_err(|_| Failure::InvalidData)?,
        },
        ReminderDraft::Relative {
            relation, offset, ..
        } => OneTimeSchedule::RelativeToDue {
            relation: match relation {
                UiDueRelation::At => DueRelation::At,
                UiDueRelation::Before => DueRelation::Before,
            },
            offset: offset.map(offset_to_store).transpose()?,
        },
    })
}

fn schedule_from_store(schedule: &OneTimeSchedule) -> Result<ReminderDraft, Failure> {
    Ok(match schedule {
        OneTimeSchedule::Floating { wall } => ReminderDraft::Floating(
            koru_core::time::FloatingDateTime::parse(wall.as_str()).ok_or(Failure::InvalidData)?,
        ),
        OneTimeSchedule::Zoned { value } => {
            ReminderDraft::Zoned(koru_core::time::ZonedDateTime::new(
                value.instant_ms(),
                koru_core::time::TimeZoneName::parse(value.zone()).ok_or(Failure::InvalidData)?,
            ))
        }
        OneTimeSchedule::RelativeToDue { relation, offset } => ReminderDraft::Relative {
            relation: match relation {
                DueRelation::At => UiDueRelation::At,
                DueRelation::Before => UiDueRelation::Before,
            },
            offset: offset.map(offset_from_store),
            anchor: None,
            preview: None,
        },
    })
}

#[allow(dead_code)] // Used by the deferred visible creation surface through Controller.
fn offset_to_store(offset: DurationExpression) -> Result<SemanticOffset, Failure> {
    let amount = NonZeroU32::new(offset.amount).ok_or(Failure::InvalidData)?;
    match offset.unit {
        DurationUnit::Minute => Ok(SemanticOffset::ExactMinutes(amount)),
        DurationUnit::Hour => Ok(SemanticOffset::ExactHours(amount)),
        DurationUnit::Day => Ok(SemanticOffset::CalendarDays(amount)),
        DurationUnit::Week => Ok(SemanticOffset::CalendarWeeks(amount)),
        DurationUnit::Month => Err(Failure::InvalidData),
    }
}

fn offset_from_store(offset: SemanticOffset) -> DurationExpression {
    let (amount, unit) = match offset {
        SemanticOffset::ExactMinutes(n) => (n.get(), DurationUnit::Minute),
        SemanticOffset::ExactHours(n) => (n.get(), DurationUnit::Hour),
        SemanticOffset::CalendarDays(n) => (n.get(), DurationUnit::Day),
        SemanticOffset::CalendarWeeks(n) => (n.get(), DurationUnit::Week),
    };
    DurationExpression {
        amount,
        unit,
        origin: DurationOrigin::Now,
        sign: DurationSign::Forward,
    }
}

#[allow(dead_code)] // Used by the deferred visible creation surface through Controller.
fn tone_to_store(tone: Tone) -> ToneId {
    match tone {
        Tone::System => ToneId::System,
        Tone::KoruDefault => ToneId::KoruDefault,
        Tone::Soft => ToneId::Soft,
        Tone::Chime => ToneId::Chime,
        Tone::Pulse => ToneId::Pulse,
        Tone::Bright => ToneId::Bright,
        Tone::Calm => ToneId::Calm,
        Tone::Drop => ToneId::Drop,
        Tone::Knock => ToneId::Knock,
        Tone::Signal => ToneId::Signal,
        Tone::Urgent => ToneId::Urgent,
    }
}

fn tone_from_store(tone: ToneId) -> Tone {
    match tone {
        ToneId::System => Tone::System,
        ToneId::KoruDefault => Tone::KoruDefault,
        ToneId::Soft => Tone::Soft,
        ToneId::Chime => Tone::Chime,
        ToneId::Pulse => Tone::Pulse,
        ToneId::Bright => Tone::Bright,
        ToneId::Calm => Tone::Calm,
        ToneId::Drop => Tone::Drop,
        ToneId::Knock => Tone::Knock,
        ToneId::Signal => Tone::Signal,
        ToneId::Urgent => Tone::Urgent,
    }
}

fn week_start_override(preference: WeekStartPreference) -> Option<WeekStart> {
    match preference {
        WeekStartPreference::System => None,
        WeekStartPreference::Monday => Some(WeekStart::Monday),
        WeekStartPreference::Sunday => Some(WeekStart::Sunday),
    }
}

#[cfg(test)]
mod tests {
    mod projection_handoff;

    use super::*;
    use chrono::{NaiveDate, NaiveTime};
    use koru_core::time::{DateOnly, FloatingDateTime, Temporal, TimeZoneName};
    use koru_editor::reminders::{CreationFocus, CreationState};
    use koru_editor::utils::when::{DateOrder, Language};

    fn target() -> Target {
        let workspace = koru_store_core::WorkspaceId::new();
        let document = koru_store_core::DocumentId::new();
        Target {
            profile: ProfileId::new(),
            entity: EntityRef::new("block", workspace, document, uuid::Uuid::new_v4()).unwrap(),
        }
    }

    fn sibling(target: &Target) -> Target {
        Target {
            profile: target.profile,
            entity: EntityRef::new(
                "block",
                target.entity.workspace(),
                target.entity.document(),
                uuid::Uuid::new_v4(),
            )
            .unwrap(),
        }
    }

    fn request(generation: u64) -> Request {
        let _ = generation;
        Request {
            identity: RequestIdentity {
                token: koru_editor::reminders::RequestToken::new(),
                node: koru_core::NodeId::new(),
            },
            phrase: "tomorrow at 9am".into(),
            due_anchor: None,
        }
    }

    fn context() -> ResolutionContext {
        ResolutionContext {
            now_ms: 0,
            language: Language::English,
            date_order: DateOrder::MonthFirst,
            current_zone: TimeZoneName::utc(),
            selection: None,
            end_of_day: None,
            due_anchor: None,
            system_week_start: WeekStart::Monday,
            week_start_override: None,
        }
    }

    fn draft() -> Draft {
        Draft::new(ReminderDraft::Floating(FloatingDateTime::new(
            NaiveDate::from_ymd_opt(2026, 9, 3)
                .unwrap()
                .and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
        )))
    }

    fn loaded(summaries: std::collections::HashMap<String, ChipSummary>) -> Loaded {
        Loaded {
            active: Vec::new(),
            history: Vec::new(),
            global_rows: Vec::new(),
            global_updating: false,
            summaries,
            end_of_day: None,
            default_tone: Tone::System,
            week_start: WeekStartPreference::System,
            has_reminders: false,
            present_ids: HashSet::new(),
        }
    }

    #[test]
    fn an_updating_projection_arms_a_bounded_authoritative_retry() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 1;
        controller.load_epoch = 1;
        let authoritative = GlobalItem {
            target: current.entity.to_storage(),
            title: "last authoritative".to_owned(),
            source_location: String::new(),
            reminder_id: ReminderId::new().to_storage(),
            revision: RuleRevisionId::new().to_storage(),
            occurrence: Some("authoritative-occurrence".to_owned()),
            sort_at_ms: Some(1),
            draft: draft(),
            state: GlobalItemState::ComingUp,
            snoozed: false,
            rule_paused: false,
        };
        let mut first = loaded(Default::default());
        first.global_rows.push(authoritative.clone());
        assert!(controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 1,
            load_epoch: Some(1),
            answer: Answer::Loaded(first),
        }));

        controller.load_epoch = 2;
        let mut answer = loaded(Default::default());
        answer.global_updating = true;
        answer.global_rows.push(GlobalItem {
            title: "transient missing target".to_owned(),
            state: GlobalItemState::Paused,
            ..authoritative.clone()
        });

        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 1,
            load_epoch: Some(2),
            answer: Answer::Loaded(answer),
        }));
        assert!(controller.deadline().is_some());
        assert_eq!(controller.global.rows, vec![authoritative]);
        assert!(controller.global.updating);

        controller.projection_retry_at = Some(Instant::now() - Duration::from_millis(1));
        let prior_epoch = controller.load_epoch;
        assert!(controller.maintain());
        assert!(
            controller.load_epoch > prior_epoch,
            "a fresh read was attempted"
        );
        assert!(
            controller.failed_load.is_some(),
            "the no-client fixture proves the queued retry reached the worker seam"
        );
    }

    #[test]
    fn all_tones_map_exhaustively_both_ways() {
        for tone in Tone::ALL {
            assert_eq!(tone_from_store(tone_to_store(tone)), tone);
        }
    }

    #[test]
    fn editor_context_applies_only_an_authored_week_start_override() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.loaded_settings = Some(ProfileSettings {
            profile: current.profile,
            end_of_day: None,
            default_tone: Tone::System,
            week_start: WeekStartPreference::Sunday,
        });
        let (resolved, _) = controller.editor_context(context());
        assert_eq!(resolved.system_week_start, WeekStart::Monday);
        assert_eq!(resolved.week_start_override, Some(WeekStart::Sunday));

        controller.loaded_settings.as_mut().unwrap().week_start = WeekStartPreference::System;
        let (resolved, _) = controller.editor_context(context());
        assert_eq!(resolved.system_week_start, WeekStart::Monday);
        assert_eq!(resolved.week_start_override, None);
    }

    #[test]
    fn stale_target_or_generation_answers_change_nothing() {
        let mut controller = Controller::default();
        let current = target();
        controller.target = Some(current.clone());
        controller.generation = 4;
        controller.load_epoch = 1;
        controller.management = Some(Management::loading(current.entity.to_storage()));
        let loaded = Loaded {
            active: Vec::new(),
            history: Vec::new(),
            global_rows: Vec::new(),
            global_updating: false,
            summaries: std::collections::HashMap::new(),
            end_of_day: None,
            default_tone: Tone::System,
            week_start: WeekStartPreference::System,
            has_reminders: false,
            present_ids: HashSet::new(),
        };
        assert!(!controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 3,
            load_epoch: Some(1),
            answer: Answer::Loaded(loaded.clone()),
        }));
        assert!(controller.management.as_ref().unwrap().loading);
        assert!(!controller.apply_stamped(StampedAnswer {
            target: target(),
            generation: 4,
            load_epoch: Some(1),
            answer: Answer::Loaded(loaded),
        }));
        assert!(controller.management.as_ref().unwrap().loading);
    }

    #[test]
    fn presence_counts_paused_and_handled_rules_but_not_deleted_rules() {
        assert!(!stored_reminder_is_present(true, RuleStatus::Enabled));
        assert!(!stored_reminder_is_present(false, RuleStatus::Deleted));
        assert!(stored_reminder_is_present(false, RuleStatus::Paused));
        // Handling is an occurrence action/effective state. The synchronized
        // rule remains enabled, so it still counts for the one-time consent.
        assert!(stored_reminder_is_present(false, RuleStatus::Enabled));
    }

    #[test]
    fn only_an_accepted_current_load_can_publish_profile_presence() {
        let current = target();
        let stale = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 4;
        controller.load_epoch = 2;

        let mut current_loaded = loaded(Default::default());
        current_loaded.has_reminders = true;
        assert!(controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 4,
            load_epoch: Some(2),
            answer: Answer::Loaded(current_loaded),
        }));
        assert_eq!(controller.has_reminders(), Some(true));

        assert!(!controller.apply_stamped(StampedAnswer {
            target: stale,
            generation: 4,
            load_epoch: Some(2),
            answer: Answer::Loaded(loaded(Default::default())),
        }));
        assert!(!controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 3,
            load_epoch: Some(2),
            answer: Answer::Loaded(loaded(Default::default())),
        }));
        assert_eq!(controller.has_reminders(), Some(true));
    }

    #[test]
    fn event_during_load_discards_the_old_success_and_failure_epoch() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 5;
        controller.load_epoch = 2;
        controller.management = Some(Management::loading(current.entity.to_storage()));

        assert!(!controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 5,
            load_epoch: Some(1),
            answer: Answer::Loaded(loaded(std::collections::HashMap::from([(
                "old".into(),
                ChipSummary {
                    active_count: 1,
                    waiting: false,
                    snoozed: false,
                },
            )]))),
        }));
        controller.retain_failure(
            Queued {
                target: current.clone(),
                generation: 5,
                load_epoch: Some(1),
                work: Work::Load,
            },
            Failure::Unavailable,
        );
        assert!(controller.failed_load.is_none());
        assert!(controller.summaries.is_empty());

        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 5,
            load_epoch: Some(2),
            answer: Answer::Loaded(loaded(Default::default())),
        }));
        assert!(!controller.management.as_ref().unwrap().loading);
    }

    #[test]
    fn due_change_re_resolves_the_open_relative_creation() {
        let old_due = Temporal::Date(DateOnly::new(NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()));
        let new_due = Temporal::Date(DateOnly::new(NaiveDate::from_ymd_opt(2026, 9, 12).unwrap()));
        let mut request = request(1);
        request.phrase = "1 day before due".into();
        request.due_anchor = Some(old_due.clone());
        let identity = request.identity;
        let mut resolution = context();
        resolution.due_anchor = Some(old_due.clone());
        let mut creation = Creation::loading(request);
        creation.resolve(&resolution, Tone::System);
        let before = creation.editable_draft().cloned().unwrap();

        let mut controller = Controller::default();
        let owner = target();
        controller.target = Some(owner.clone());
        controller.creation = Some(creation);
        controller.context = Some(resolution);
        assert!(controller.refresh_due_anchor(&owner, identity, Some(new_due.clone())));
        let after = controller.creation.as_ref().unwrap();
        assert_eq!(after.request.due_anchor, Some(new_due.clone()));
        assert_ne!(after.editable_draft(), Some(&before));
        assert!(matches!(
            after.editable_draft().map(|draft| &draft.schedule),
            Some(ReminderDraft::Relative {
                anchor: Some(anchor),
                ..
            }) if *anchor == new_due
        ));
    }

    #[test]
    fn typed_recreate_due_change_reanchors_without_losing_relation_preview_or_tone() {
        let old_due = Temporal::Date(DateOnly::new(NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()));
        let new_due = Temporal::Date(DateOnly::new(NaiveDate::from_ymd_opt(2026, 9, 12).unwrap()));
        let mut resolution = context();
        resolution.due_anchor = Some(old_due.clone());
        let source = Draft::preserving(
            ReminderDraft::Relative {
                relation: UiDueRelation::Before,
                offset: Some(DurationExpression {
                    amount: 1,
                    unit: DurationUnit::Day,
                    origin: DurationOrigin::Now,
                    sign: DurationSign::Forward,
                }),
                anchor: Some(old_due),
                preview: None,
            },
            Some(Tone::Soft),
        );
        let owner = target();
        let node = NodeId::from_uuid(owner.entity.node());
        let mut controller = Controller::default();
        controller.open_recreate(owner.clone(), node, source, resolution);
        let identity = controller.creation.as_ref().unwrap().request.identity;

        assert!(controller.refresh_due_anchor(&owner, identity, Some(new_due.clone())));
        let draft = controller
            .creation
            .as_ref()
            .and_then(Creation::editable_draft)
            .unwrap();
        assert_eq!(draft.tone_override, Some(Tone::Soft));
        assert!(matches!(
            &draft.schedule,
            ReminderDraft::Relative {
                relation: UiDueRelation::Before,
                offset: Some(offset),
                anchor: Some(anchor),
                preview: Some(_),
            } if offset.amount == 1 && offset.unit == DurationUnit::Day && anchor == &new_due
        ));
    }

    #[test]
    fn typing_leaves_typed_recreate_and_late_settings_resolve_end_of_day() {
        let owner = target();
        let node = NodeId::from_uuid(owner.entity.node());
        let source = Draft::preserving(draft().schedule, Some(Tone::Soft));
        let mut controller = Controller::default();
        controller.open_recreate(owner, node, source, context());
        assert!(controller.creation_seeded);
        assert!(controller.update_creation_phrase("end of day".to_owned()));
        assert!(!controller.creation_seeded);
        assert!(matches!(
            controller.creation.as_ref().map(|creation| &creation.state),
            Some(CreationState::EndOfDay)
        ));

        let queued = controller
            .pending
            .iter()
            .find(|queued| matches!(queued.work, Work::Load))
            .cloned()
            .unwrap();
        let mut answer = loaded(Default::default());
        answer.end_of_day = Some(NaiveTime::from_hms_opt(17, 30, 0).unwrap());
        answer.default_tone = Tone::Calm;
        assert!(controller.apply_stamped(StampedAnswer {
            target: queued.target,
            generation: queued.generation,
            load_epoch: queued.load_epoch,
            answer: Answer::Loaded(answer),
        }));
        let creation = controller.creation.as_ref().unwrap();
        assert_eq!(creation.default_tone, Some(Tone::Calm));
        let draft = creation.confirmable_draft().unwrap();
        assert_eq!(draft.tone_override, Some(Tone::Soft));
        assert!(matches!(
            &draft.schedule,
            ReminderDraft::Floating(value) if value.wall().time() == NaiveTime::from_hms_opt(17, 30, 0).unwrap()
        ));
    }

    #[test]
    fn controller_creation_uses_fresh_clock_and_zone_when_typing_begins() {
        let owner = target();
        let node = NodeId::from_uuid(owner.entity.node());
        let mut initial = context();
        initial.now_ms = NaiveDate::from_ymd_opt(2026, 9, 3)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let mut controller = Controller::default();
        controller.open_recreate(owner, node, draft(), initial);

        let before = controller.creation.as_ref().unwrap().state.clone();
        let mut advanced = context();
        advanced.now_ms = NaiveDate::from_ymd_opt(2026, 9, 3)
            .unwrap()
            .and_hms_opt(13, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert!(!controller.refresh_creation_context(advanced.clone(), Tone::System));
        assert_eq!(controller.creation.as_ref().unwrap().state, before);
        assert_eq!(controller.context.as_ref().unwrap().now_ms, advanced.now_ms);

        let mut fresh = context();
        fresh.now_ms = NaiveDate::from_ymd_opt(2026, 9, 3)
            .unwrap()
            .and_hms_opt(17, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        fresh.current_zone = TimeZoneName::parse("Etc/GMT+5").unwrap();
        assert!(controller.refresh_creation_context(fresh, Tone::System));
        assert!(controller.update_creation_phrase("in 10 minutes".to_owned()));

        let draft = controller
            .creation
            .as_ref()
            .and_then(Creation::confirmable_draft)
            .expect("fresh relative phrase resolves");
        assert!(matches!(
            &draft.schedule,
            ReminderDraft::Floating(value)
                if value.wall()
                    == NaiveDate::from_ymd_opt(2026, 9, 3)
                        .unwrap()
                        .and_hms_opt(12, 10, 0)
                        .unwrap()
        ));
    }

    #[test]
    fn another_document_cannot_clear_a_background_creations_due_anchor() {
        let owner = target();
        let other = sibling(&owner);
        let due = Temporal::Date(DateOnly::new(NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()));
        let mut request = request(1);
        request.due_anchor = Some(due.clone());
        let identity = request.identity;
        let mut controller = Controller::default();
        controller.target = Some(owner.clone());
        controller.creation = Some(Creation::loading(request));
        controller.context = Some(context());

        assert!(!controller.refresh_due_anchor(&other, identity, None));
        assert_eq!(controller.creation_owner(), Some((owner, identity)));
        assert_eq!(
            controller.creation.as_ref().unwrap().request.due_anchor,
            Some(due)
        );
    }

    #[test]
    fn submission_is_emitted_once_and_success_updates_its_own_pill() {
        let mut controller = Controller::default();
        let current = target();
        let request = request(21);
        let identity = request.identity;
        controller.open(current.clone(), request, context());
        controller.pending.clear();
        controller.confirm_creation(draft());
        assert_eq!(
            controller.take_submitted_request(),
            Some((current.clone(), identity))
        );
        assert_eq!(controller.take_submitted_request(), None);
        assert!(controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: controller.generation,
            load_epoch: None,
            answer: Answer::Mutated {
                trigger: Some(identity),
            },
        }));
        assert!(matches!(
            controller
                .optimistic_creations()
                .find(|pill| pill.request.identity == identity)
                .map(|pill| pill.state),
            Some(OptimisticState::Succeeded)
        ));
    }

    #[test]
    fn durable_create_publishes_presence_only_after_its_authoritative_reload() {
        let mut controller = Controller::default();
        let current = target();
        controller.target = Some(current.clone());
        controller.generation = 8;
        let identity = request(22).identity;
        assert!(controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 8,
            load_epoch: None,
            answer: Answer::Mutated {
                trigger: Some(identity),
            },
        }));
        assert_eq!(controller.has_reminders(), None);

        let mut reconciled = loaded(Default::default());
        reconciled.has_reminders = true;
        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 8,
            load_epoch: Some(controller.load_epoch),
            answer: Answer::Loaded(reconciled),
        }));
        assert_eq!(controller.has_reminders(), Some(true));
    }

    #[test]
    fn remote_first_reminder_reconciliation_publishes_presence() {
        let mut controller = Controller::default();
        let current = target();
        controller.target = Some(current.clone());
        controller.generation = 9;
        controller.load_epoch = 3;
        let mut reconciled = loaded(Default::default());
        reconciled.has_reminders = true;

        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 9,
            load_epoch: Some(3),
            answer: Answer::Loaded(reconciled),
        }));
        assert_eq!(controller.has_reminders(), Some(true));
    }

    #[test]
    fn an_in_flight_worker_has_no_timer_and_completion_wakes_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (send, receive) = mpsc::channel();
        let mut controller = Controller::default();
        controller.receiver = Some(receive);
        assert_eq!(controller.deadline(), None);

        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        let wake: Waker = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        deliver(
            &send,
            Some(&wake),
            StampedAnswer {
                target: target(),
                generation: 1,
                load_epoch: None,
                answer: Answer::Failed(Failure::Unavailable),
            },
        );
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn double_confirm_queues_only_one_client_minted_rule() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current);
        controller.generation = 2;
        controller.creation = Some(Creation {
            request: request(9),
            state: CreationState::Saving(draft()),
            focus: CreationFocus::Confirm,
            default_tone: Some(Tone::System),
            tone_override: None,
        });
        controller.context = Some(context());

        controller.confirm_creation(draft());
        controller.confirm_creation(draft());
        assert_eq!(controller.pending.len(), 1);
        assert!(matches!(
            controller.pending.front().map(|queued| &queued.work),
            Some(Work::Create {
                trigger: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn pending_creations_fail_retry_and_succeed_independently() {
        let current = target();
        let mut controller = Controller::default();
        let first = request(101);
        let first_identity = first.identity;
        controller.open(current.clone(), first, context());
        controller.pending.clear();
        controller.confirm_creation(draft());
        let first_work = controller.pending.pop_front().unwrap();

        let second = request(102);
        let second_identity = second.identity;
        controller.open(current.clone(), second, context());
        controller
            .pending
            .retain(|queued| !matches!(queued.work, Work::Load));
        controller.confirm_creation(draft());
        let second_work = controller.pending.pop_front().unwrap();

        controller.retain_failure(first_work.clone(), Failure::Unavailable);
        assert!(matches!(
            controller
                .optimistic
                .iter()
                .find(|record| record.presentation.request.identity == first_identity)
                .map(|record| record.presentation.state),
            Some(OptimisticState::Failed {
                reason: Failure::Unavailable
            })
        ));
        assert!(matches!(
            controller
                .optimistic
                .iter()
                .find(|record| record.presentation.request.identity == second_identity)
                .map(|record| record.presentation.state),
            Some(OptimisticState::Pending)
        ));
        assert!(controller.failed_mutation.is_none());

        assert!(controller.retry_creation(first_identity.token));
        let retry = controller.pending.pop_front().unwrap();
        assert_eq!(
            match (&first_work.work, &retry.work) {
                (Work::Create { rule: first, .. }, Work::Create { rule: retry, .. }) => {
                    (first.id, first.revision) == (retry.id, retry.revision)
                }
                _ => false,
            },
            true
        );

        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: second_work.generation,
            load_epoch: None,
            answer: Answer::Mutated {
                trigger: Some(second_identity),
            },
        }));
        assert!(matches!(
            controller
                .optimistic
                .iter()
                .find(|record| record.presentation.request.identity == second_identity)
                .map(|record| record.presentation.state),
            Some(OptimisticState::Succeeded)
        ));
    }

    #[test]
    fn failed_creation_edit_reuses_identity_and_dismiss_restores_no_text() {
        let current = target();
        let mut controller = Controller::default();
        let request = request(103);
        let identity = request.identity;
        controller.open(current, request, context());
        controller.pending.clear();
        controller.confirm_creation(draft());
        let failed = controller.pending.pop_front().unwrap();
        let original_id = match &failed.work {
            Work::Create { rule, .. } => rule.id,
            _ => unreachable!(),
        };
        controller.retain_failure(failed, Failure::Unavailable);

        assert!(controller.edit_failed_creation(identity.token));
        assert_eq!(
            controller
                .creation
                .as_ref()
                .map(|value| value.request.identity),
            Some(identity)
        );
        assert!(
            controller
                .optimistic_creations()
                .all(|pill| pill.request.identity != identity),
            "the failed pill is hidden only while its editor owns it"
        );
        assert!(controller.cancel_creation());
        assert!(controller.creation.is_none());
        assert!(matches!(
            controller
                .optimistic_creations()
                .find(|pill| pill.request.identity == identity)
                .map(|pill| pill.state),
            Some(OptimisticState::Failed {
                reason: Failure::Unavailable
            })
        ));
        assert!(
            controller.take_cancelled_request().is_none(),
            "canceling a failed edit restores the pill, not submitted command text"
        );
        assert!(controller.edit_failed_creation(identity.token));
        controller.confirm_creation(draft());
        let edited = controller
            .pending
            .iter()
            .find(|queued| matches!(queued.work, Work::Create { .. }))
            .unwrap()
            .clone();
        assert!(matches!(edited.work, Work::Create { ref rule, .. } if rule.id == original_id));
        controller
            .pending
            .retain(|queued| !matches!(queued.work, Work::Create { .. }));
        controller.retain_failure(edited, Failure::Rejected);
        assert!(controller.dismiss_failed_creation(identity.token));
        assert!(controller.optimistic.is_empty());
        assert!(controller.take_cancelled_request().is_none());
        assert!(controller.take_submitted_request().is_some());
    }

    #[test]
    fn response_lost_retry_reuses_rule_revision_and_action_ids() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 3;
        controller.creation = Some(Creation {
            request: request(12),
            state: CreationState::Saving(draft()),
            focus: CreationFocus::Confirm,
            default_tone: Some(Tone::System),
            tone_override: None,
        });
        controller.context = Some(context());
        let token = controller.creation.as_ref().unwrap().request.identity.token;
        controller.confirm_creation(draft());
        let create = controller.pending.pop_front().unwrap();
        let (id, revision) = match &create.work {
            Work::Create { rule, .. } => (rule.id, rule.revision),
            _ => panic!("create was not queued"),
        };
        controller.retain_failure(create, Failure::Unavailable);
        assert!(controller.retry_creation(token));
        assert!(matches!(
            controller
                .optimistic_creations()
                .find(|pill| pill.request.identity.token == token)
                .map(|pill| pill.state),
            Some(OptimisticState::Pending)
        ));
        controller.confirm_creation(draft());
        assert_eq!(controller.pending.len(), 1);
        match &controller.pending.front().unwrap().work {
            Work::Create { rule, .. } => {
                assert_eq!(rule.id, id);
                assert_eq!(rule.revision, revision);
            }
            _ => panic!("retry changed work kind"),
        }

        controller.pending.clear();
        let reminder = ReminderId::new();
        let occurrence = OccurrenceKey::derive(
            reminder,
            RuleRevisionId::new(),
            &koru_store_core::OccurrenceSource::OneTime {
                moment: koru_store_core::OccurrenceMoment::Zoned {
                    value: koru_store_core::ZonedDateTime::new(10, "UTC").unwrap(),
                },
            },
        );
        controller.apply_management(ManagementIntent::Snooze {
            id: reminder.to_storage(),
            occurrence: occurrence.to_storage(),
            until_ms: 20,
        });
        let snooze = controller.pending.pop_front().unwrap();
        let action_id = match &snooze.work {
            Work::Snooze { action, .. } => action.id,
            _ => panic!("snooze was not queued"),
        };
        controller.failed_mutation = Some(FailedMutation {
            queued: snooze,
            reason: Failure::Unavailable,
        });
        assert!(controller.retry_failed(&current));
        assert!(matches!(
            &controller.pending.front().unwrap().work,
            Work::Snooze { action, .. } if action.id == action_id
        ));
    }

    #[test]
    fn authoritative_client_id_reconciles_a_failed_create_after_its_response_was_lost() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 4;
        controller.load_epoch = 2;
        controller.creation = Some(Creation {
            request: request(13),
            state: CreationState::Saving(draft()),
            focus: CreationFocus::Confirm,
            default_tone: Some(Tone::System),
            tone_override: None,
        });
        controller.context = Some(context());
        let token = controller.creation.as_ref().unwrap().request.identity.token;
        controller.confirm_creation(draft());
        let create = controller.pending.pop_front().unwrap();
        let id = match &create.work {
            Work::Create { rule, .. } => rule.id.to_storage(),
            _ => panic!("create was not queued"),
        };
        controller.retain_failure(create, Failure::Unavailable);
        assert!(controller.failed_creations.contains_key(&token));

        let mut authoritative = loaded(Default::default());
        authoritative.present_ids.insert(id);
        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 4,
            load_epoch: Some(2),
            answer: Answer::Loaded(authoritative),
        }));

        assert!(controller.optimistic_creations().next().is_none());
        assert!(!controller.failed_creations.contains_key(&token));
        assert!(!controller.retry_creation(token));
        assert!(!controller.edit_failed_creation(token));
    }

    #[test]
    fn cancel_is_suppressed_while_saving_and_target_switch_keeps_its_optimistic_pill() {
        let first = target();
        let second = sibling(&first);
        let mut controller = Controller::default();
        controller.target = Some(first.clone());
        controller.generation = 1;
        controller.load_epoch = 1;
        let first_request = request(31);
        let first_identity = first_request.identity;
        controller.creation = Some(Creation {
            request: first_request,
            state: CreationState::Saving(draft()),
            focus: CreationFocus::Confirm,
            default_tone: Some(Tone::System),
            tone_override: None,
        });
        controller.context = Some(context());
        assert!(!controller.cancel_creation());
        controller.confirm_creation(draft());

        let second_request = request(32);
        let second_identity = second_request.identity;
        controller.open(second.clone(), second_request, context());
        assert!(controller.pending.iter().any(|queued| {
            queued.target == first && matches!(queued.work, Work::Create { .. })
        }));
        assert!(
            controller
                .pending
                .iter()
                .any(|queued| { queued.target == second && matches!(queued.work, Work::Load) })
        );
        assert!(!controller.apply_stamped(StampedAnswer {
            target: first.clone(),
            generation: 1,
            load_epoch: None,
            answer: Answer::Mutated {
                trigger: Some(first_identity),
            },
        }));
        assert!(controller.optimistic.iter().any(|record| {
            record.target == first
                && record.presentation.request.identity == first_identity
                && record.presentation.state == OptimisticState::Succeeded
        }));
        assert_eq!(
            controller.creation.as_ref().unwrap().request.identity,
            second_identity
        );
    }

    #[test]
    fn opening_a_second_node_releases_the_first_uncommitted_generation() {
        let first = target();
        let second = sibling(&first);
        let mut controller = Controller::default();
        let first_request = request(60);
        let first_identity = first_request.identity;
        let second_request = request(61);
        let second_identity = second_request.identity;
        controller.open(first.clone(), first_request, context());
        controller.open(second.clone(), second_request, context());

        assert_eq!(
            controller.take_cancelled_request(),
            Some((first, first_identity))
        );
        assert_eq!(controller.take_cancelled_request(), None);
        assert_eq!(controller.target, Some(second));
        assert_eq!(
            controller.creation.as_ref().unwrap().request.identity,
            second_identity
        );
    }

    #[test]
    fn management_and_profile_replacement_release_unsubmitted_command_ownership() {
        let first = target();
        let second = sibling(&first);
        let mut controller = Controller::default();
        let first_request = request(62);
        let first_identity = first_request.identity;
        controller.open(first.clone(), first_request, context());
        controller.open_management(second.clone(), context());
        assert_eq!(
            controller.take_cancelled_request(),
            Some((first.clone(), first_identity))
        );
        assert!(controller.creation.is_none());

        let second_request = request(63);
        let second_identity = second_request.identity;
        controller.open(second.clone(), second_request, context());
        let mut other_profile = target();
        other_profile.profile = koru_store_core::ProfileId::new();
        assert!(controller.ensure_profile(other_profile));
        assert_eq!(
            controller.take_cancelled_request(),
            Some((second, second_identity))
        );
        assert!(controller.creation.is_none());
    }

    #[test]
    fn unavailable_and_failed_initial_load_are_retryable_without_new_generation() {
        let current = target();
        let mut controller = Controller::default();
        controller.open(current.clone(), request(40), context());
        // A failed/pending event-tail setup is availability enrichment, not a
        // prerequisite for reading the current authoritative Store snapshot.
        controller.subscription_started = true;
        let generation = controller.generation;
        assert!(controller.maintain());
        assert!(matches!(
            controller.creation.as_ref().map(|creation| &creation.state),
            Some(CreationState::LoadFailed {
                reason: Failure::Unavailable
            })
        ));
        assert_eq!(
            controller.failed_owner(),
            Some((current.clone(), Failure::Unavailable))
        );
        assert!(controller.retry_failed(&current));
        let queued = controller.pending.pop_front().unwrap();
        assert_eq!(queued.target, current);
        assert_eq!(queued.generation, generation);
        assert!(matches!(queued.work, Work::Load));

        controller.apply_stamped(StampedAnswer {
            target: queued.target,
            generation: queued.generation,
            load_epoch: queued.load_epoch,
            answer: Answer::Loaded(loaded(Default::default())),
        });
        assert!(matches!(
            controller.creation.as_ref().map(|creation| &creation.state),
            Some(CreationState::Confirmation(_))
        ));
    }

    #[test]
    fn discarding_a_failed_management_load_clears_its_visible_error() {
        let current = target();
        let mut controller = Controller::default();
        controller.open_management(current.clone(), context());
        controller.subscription_started = true;
        assert!(controller.maintain());
        assert_eq!(
            controller
                .management
                .as_ref()
                .and_then(|management| management.error),
            Some(Failure::Unavailable)
        );
        assert_eq!(
            controller.failed_owner(),
            Some((current.clone(), Failure::Unavailable))
        );
        assert!(controller.discard_failed(&current));
        assert!(controller.failed_owner().is_none());
        assert_eq!(
            controller
                .management
                .as_ref()
                .and_then(|management| management.error),
            None
        );
    }

    #[test]
    fn failed_mutation_preserves_its_draft() {
        let current = target();
        let expected = draft();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 7;
        controller.creation = Some(Creation {
            request: request(50),
            state: CreationState::Saving(expected.clone()),
            focus: CreationFocus::Confirm,
            default_tone: Some(Tone::System),
            tone_override: None,
        });
        controller.context = Some(context());
        let identity = controller.creation.as_ref().unwrap().request.identity;
        controller.confirm_creation(expected.clone());
        let failed = controller.pending.pop_front().unwrap();
        controller.retain_failure(failed, Failure::Unavailable);
        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 7,
            load_epoch: None,
            answer: Answer::Failed(Failure::Unavailable),
        }));
        assert_eq!(
            controller
                .optimistic
                .iter()
                .find(|record| record.presentation.request.identity == identity)
                .map(|record| (&record.presentation.draft, record.presentation.state)),
            Some((
                &expected,
                OptimisticState::Failed {
                    reason: Failure::Unavailable
                }
            ))
        );
    }

    #[test]
    fn remote_load_preserves_failed_create_and_cannot_mint_a_second_rule() {
        let current = target();
        let expected = draft();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 7;
        controller.load_epoch = 3;
        controller.creation = Some(Creation {
            request: request(51),
            state: CreationState::Saving(expected.clone()),
            focus: CreationFocus::Confirm,
            default_tone: Some(Tone::System),
            tone_override: None,
        });
        controller.context = Some(context());
        let token = controller.creation.as_ref().unwrap().request.identity.token;
        controller.management = Some(Management::loading(current.entity.to_storage()));
        controller.confirm_creation(expected.clone());
        let failed = controller.pending.pop_front().unwrap();
        controller.retain_failure(failed, Failure::Unavailable);

        let summary = ChipSummary {
            active_count: 1,
            waiting: false,
            snoozed: false,
        };
        assert!(controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 7,
            load_epoch: Some(3),
            answer: Answer::Loaded(loaded(std::collections::HashMap::from([(
                current.entity.to_storage(),
                summary,
            )]))),
        }));
        assert!(matches!(
            controller
                .optimistic_creations()
                .find(|pill| pill.request.identity.token == token)
                .map(|pill| pill.state),
            Some(OptimisticState::Failed {
                reason: Failure::Unavailable,
            })
        ));
        assert_eq!(controller.management.as_ref().unwrap().error, None);
        assert_eq!(
            controller.summaries.get(&current.entity.to_storage()),
            Some(&summary)
        );

        controller.confirm_creation(expected);
        assert!(controller.pending.is_empty());
        assert_eq!(controller.failed_owner(), None);
        assert!(controller.failed_creations.contains_key(&token));
    }

    #[test]
    fn cross_target_failure_keeps_its_owner_and_does_not_block_current_actions() {
        let first = target();
        let second = sibling(&first);
        let first_work = Queued {
            target: first.clone(),
            generation: 1,
            load_epoch: None,
            work: Work::Pause {
                id: ReminderId::new(),
                revision: RuleRevisionId::new(),
            },
        };
        let second_work = Queued {
            target: second.clone(),
            generation: 2,
            load_epoch: None,
            work: Work::Resume {
                id: ReminderId::new(),
                revision: RuleRevisionId::new(),
            },
        };
        let mut controller = Controller::default();
        controller.target = Some(second.clone());
        controller.generation = 2;
        controller.load_epoch = 4;
        controller.retain_failure(first_work.clone(), Failure::Unavailable);
        controller.pending.push_back(second_work);
        controller.pending.push_back(Queued {
            target: second.clone(),
            generation: 2,
            load_epoch: Some(4),
            work: Work::Load,
        });

        let mutation = controller
            .dequeue_next()
            .expect("current target mutation can pass");
        assert!(matches!(mutation.work, Work::Resume { .. }));
        assert_eq!(
            controller.failed_owner(),
            Some((first.clone(), Failure::Unavailable))
        );

        let load = controller.dequeue_next().expect("current read can pass");
        assert!(matches!(load.work, Work::Load));
        assert!(controller.apply_stamped(StampedAnswer {
            target: second.clone(),
            generation: 2,
            load_epoch: Some(4),
            answer: Answer::Loaded(loaded(Default::default())),
        }));
        assert_eq!(
            controller.failed_owner(),
            Some((first.clone(), Failure::Unavailable))
        );
        assert!(!controller.retry_failed(&second));
        assert!(controller.retry_failed(&first));
        assert!(matches!(
            controller.pending.front().map(|queued| &queued.work),
            Some(Work::Pause { .. })
        ));

        let retry = controller.pending.pop_front().unwrap();
        controller.retain_failure(retry, Failure::Unavailable);
        assert!(!controller.discard_failed(&second));
        assert!(controller.discard_failed(&first));
        assert!(controller.failed_owner().is_none());
    }

    #[test]
    fn stale_load_failure_is_disposable_and_does_not_block_the_new_target() {
        let first = target();
        let second = sibling(&first);
        let (send, receive) = mpsc::channel();
        send.send(StampedAnswer {
            target: first.clone(),
            generation: 1,
            load_epoch: Some(1),
            answer: Answer::Failed(Failure::Unavailable),
        })
        .unwrap();
        let mut controller = Controller::default();
        controller.target = Some(second.clone());
        controller.generation = 2;
        controller.load_epoch = 2;
        controller.creation = Some(Creation::loading(request(72)));
        controller.in_flight = Some(Queued {
            target: first,
            generation: 1,
            load_epoch: Some(1),
            work: Work::Load,
        });
        controller.receiver = Some(receive);

        assert!(!controller.maintain());
        assert!(controller.failed_load.is_none());
        assert!(matches!(
            controller.creation.as_ref().map(|creation| &creation.state),
            Some(CreationState::Loading)
        ));
        controller.enqueue_current_load();
        assert!(
            controller
                .pending
                .iter()
                .any(|queued| queued.target == second)
        );
    }

    #[test]
    fn invalid_management_intent_does_not_leave_the_surface_loading() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.management = Some(Management {
            target: current.entity.to_storage(),
            loading: false,
            active: Vec::new(),
            history: Vec::new(),
            history_expanded: false,
            clear_history_confirmation: false,
            error: None,
        });

        controller.apply_management(ManagementIntent::Pause {
            id: "not-a-reminder-id".into(),
            revision: "ignored-stale-revision".into(),
        });
        assert!(!controller.management.as_ref().unwrap().loading);
        assert!(controller.pending.is_empty());
    }

    #[test]
    fn every_management_intent_dispatches_one_exact_store_operation() {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        enum Kind {
            Create,
            Update,
            Pause,
            Resume,
            Delete,
            Snooze,
            Handle,
            ClearHistory,
            None,
        }

        fn kind(work: Option<&Work>) -> Kind {
            match work {
                Some(Work::Create { .. }) => Kind::Create,
                Some(Work::Update { .. }) => Kind::Update,
                Some(Work::Pause { .. }) => Kind::Pause,
                Some(Work::Resume { .. }) => Kind::Resume,
                Some(Work::Delete { .. }) => Kind::Delete,
                Some(Work::Snooze { .. }) => Kind::Snooze,
                Some(Work::Handle { .. }) => Kind::Handle,
                Some(Work::ClearHistoryForTarget | Work::ClearAllHistory) => Kind::ClearHistory,
                Some(Work::Load) | None => Kind::None,
            }
        }

        let reminder = ReminderId::new();
        let occurrence = OccurrenceKey::derive(
            reminder,
            RuleRevisionId::new(),
            &koru_store_core::OccurrenceSource::OneTime {
                moment: koru_store_core::OccurrenceMoment::Zoned {
                    value: koru_store_core::ZonedDateTime::new(10, "UTC").unwrap(),
                },
            },
        );
        let cases = [
            ("add", ManagementIntent::Add(draft()), Kind::Create),
            (
                "edit",
                ManagementIntent::Edit {
                    id: reminder.to_storage(),
                    draft: draft(),
                    paused: false,
                },
                Kind::Update,
            ),
            (
                "pause",
                ManagementIntent::Pause {
                    id: reminder.to_storage(),
                    revision: "stale-view-revision".into(),
                },
                Kind::Pause,
            ),
            (
                "resume",
                ManagementIntent::Resume {
                    id: reminder.to_storage(),
                    revision: "stale-view-revision".into(),
                },
                Kind::Resume,
            ),
            (
                "delete",
                ManagementIntent::Delete {
                    id: reminder.to_storage(),
                },
                Kind::Delete,
            ),
            (
                "snooze",
                ManagementIntent::Snooze {
                    id: reminder.to_storage(),
                    occurrence: occurrence.to_storage(),
                    until_ms: 20,
                },
                Kind::Snooze,
            ),
            (
                "confirm clear history",
                ManagementIntent::ConfirmClearHistory,
                Kind::ClearHistory,
            ),
            (
                "ask clear history",
                ManagementIntent::AskClearHistory,
                Kind::None,
            ),
            (
                "cancel clear history",
                ManagementIntent::CancelClearHistory,
                Kind::None,
            ),
        ];

        for (label, intent, expected) in cases {
            let current = target();
            let mut controller = Controller::default();
            controller.target = Some(current.clone());
            controller.management = Some(Management {
                target: current.entity.to_storage(),
                loading: false,
                active: Vec::new(),
                history: Vec::new(),
                history_expanded: false,
                clear_history_confirmation: false,
                error: None,
            });

            controller.apply_management(intent);
            assert_eq!(
                kind(controller.pending.front().map(|queued| &queued.work)),
                expected,
                "{label}"
            );
            assert_eq!(
                controller.pending.len(),
                usize::from(expected != Kind::None),
                "{label} queued extra work"
            );
        }
    }

    #[test]
    fn paused_edit_stays_paused_before_management_refresh_arrives() {
        let owner = target();
        let mut controller = Controller::default();
        controller.target = Some(owner);
        controller.management = Some(Management::loading("target".to_owned()));
        controller.apply_management(ManagementIntent::Edit {
            id: ReminderId::new().to_storage(),
            draft: draft(),
            paused: true,
        });
        assert!(matches!(
            controller.pending.back().map(|queued| &queued.work),
            Some(Work::Update { rule }) if rule.status == RuleStatus::Paused
        ));
    }

    #[test]
    fn past_recreate_queues_only_a_load_until_creation_is_confirmed() {
        let owner = target();
        let node = NodeId::from_uuid(owner.entity.node());
        let mut current = context();
        current.now_ms = NaiveDate::from_ymd_opt(2026, 9, 2)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let source = Draft::preserving(
            ReminderDraft::Floating(FloatingDateTime::new(
                NaiveDate::from_ymd_opt(2026, 9, 1)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
            )),
            Some(Tone::Soft),
        );
        let mut controller = Controller::default();
        controller.open_recreate(owner, node, source, current);
        assert!(matches!(
            controller.creation.as_ref().map(|creation| &creation.state),
            Some(CreationState::Passed { .. })
        ));
        assert!(
            controller
                .pending
                .iter()
                .all(|queued| matches!(queued.work, Work::Load))
        );

        let draft = controller
            .creation
            .as_ref()
            .and_then(Creation::confirmable_draft)
            .cloned()
            .unwrap();
        controller.confirm_creation(draft);
        assert!(
            controller
                .pending
                .iter()
                .any(|queued| matches!(queued.work, Work::Create { .. }))
        );
    }

    #[test]
    fn global_recreate_handoff_survives_wrong_target_without_mutating() {
        let owner = target();
        let other = sibling(&owner);
        let row = GlobalItem {
            target: owner.entity.to_storage(),
            title: "history".to_owned(),
            source_location: String::new(),
            reminder_id: ReminderId::new().to_storage(),
            revision: RuleRevisionId::new().to_storage(),
            occurrence: Some("handled-occurrence".to_owned()),
            sort_at_ms: None,
            draft: draft(),
            state: GlobalItemState::History,
            snoozed: false,
            rule_paused: false,
        };
        let mut controller = Controller::default();
        controller.target = Some(owner.clone());
        controller.global = GlobalManagement::new(vec![row.clone()]);

        assert!(controller.begin_global_recreate(&row));
        assert_eq!(controller.pending_global_recreate_target(), Some(&owner));
        assert!(controller.pending.is_empty());
        controller.open_management(other, context());
        assert_eq!(controller.pending_global_recreate_target(), Some(&owner));
        assert!(
            controller
                .pending
                .iter()
                .all(|queued| matches!(queued.work, Work::Load))
        );
        assert_eq!(controller.take_global_recreate().unwrap().source, row.draft);
    }

    #[test]
    fn object_clear_and_global_clear_all_are_separately_named_work() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.management = Some(Management {
            target: current.entity.to_storage(),
            loading: false,
            active: Vec::new(),
            history: Vec::new(),
            history_expanded: true,
            clear_history_confirmation: true,
            error: None,
        });

        controller.apply_management(ManagementIntent::ConfirmClearHistory);
        assert!(matches!(
            controller.pending.pop_front().map(|queued| queued.work),
            Some(Work::ClearHistoryForTarget)
        ));
        controller.clear_all_history();
        assert!(matches!(
            controller.pending.pop_front().map(|queued| queued.work),
            Some(Work::ClearAllHistory)
        ));
    }

    #[test]
    fn object_history_confirmation_is_local_until_confirmed() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.management = Some(Management {
            target: current.entity.to_storage(),
            loading: false,
            active: Vec::new(),
            history: Vec::new(),
            history_expanded: true,
            clear_history_confirmation: false,
            error: None,
        });

        controller.apply_management(ManagementIntent::AskClearHistory);
        assert!(
            controller
                .management
                .as_ref()
                .unwrap()
                .clear_history_confirmation
        );
        assert!(controller.pending.is_empty());

        controller.apply_management(ManagementIntent::CancelClearHistory);
        assert!(
            !controller
                .management
                .as_ref()
                .unwrap()
                .clear_history_confirmation
        );
        assert!(controller.pending.is_empty());
    }

    #[test]
    fn notification_snooze_waits_for_an_exact_duration_choice() {
        let current = target();
        let reminder = ReminderId::new();
        let other = ReminderId::new();
        let occurrence = OccurrenceKey::derive(
            reminder,
            RuleRevisionId::new(),
            &koru_store_core::OccurrenceSource::OneTime {
                moment: koru_store_core::OccurrenceMoment::Zoned {
                    value: koru_store_core::ZonedDateTime::new(10, "UTC").unwrap(),
                },
            },
        );
        let mut controller = Controller::default();
        controller.open_snooze(current, reminder, occurrence);
        assert_eq!(
            controller.pending_snooze(),
            Some(&SnoozeRequest {
                reminder,
                occurrence,
            })
        );
        assert!(
            controller
                .pending
                .iter()
                .all(|queued| matches!(queued.work, Work::Load)),
            "opening management must not invent a duration or Store action"
        );

        controller.apply_management(ManagementIntent::Snooze {
            id: other.to_storage(),
            occurrence: occurrence.to_storage(),
            until_ms: 20,
        });
        assert!(controller.pending_snooze().is_some());
        controller.apply_management(ManagementIntent::Snooze {
            id: reminder.to_storage(),
            occurrence: occurrence.to_storage(),
            until_ms: 30,
        });
        assert!(controller.pending_snooze().is_none());
    }

    #[test]
    fn profile_summary_batch_survives_reopen_and_replaces_removals() {
        let first = target();
        let second = sibling(&first);
        let mut controller = Controller::default();
        controller.target = Some(first.clone());
        controller.generation = 1;
        controller.load_epoch = 1;
        let summaries = std::collections::HashMap::from([
            (
                first.entity.to_storage(),
                ChipSummary {
                    active_count: 1,
                    waiting: true,
                    snoozed: false,
                },
            ),
            (
                second.entity.to_storage(),
                ChipSummary {
                    active_count: 2,
                    waiting: false,
                    snoozed: true,
                },
            ),
        ]);
        assert!(controller.apply_stamped(StampedAnswer {
            target: first.clone(),
            generation: 1,
            load_epoch: Some(1),
            answer: Answer::Loaded(loaded(summaries.clone())),
        }));
        assert_eq!(controller.summaries, summaries);

        // Reopening another document in this loaded profile retains the one
        // profile-wide cache rather than loading rows one entity at a time.
        controller.target = Some(second.clone());
        assert!(!controller.ensure_profile(second.clone()));
        assert_eq!(controller.summaries.len(), 2);

        controller.generation = 2;
        controller.load_epoch = 2;
        let remaining = std::collections::HashMap::from([(
            second.entity.to_storage(),
            ChipSummary {
                active_count: 1,
                waiting: false,
                snoozed: false,
            },
        )]);
        assert!(controller.apply_stamped(StampedAnswer {
            target: second.clone(),
            generation: 2,
            load_epoch: Some(2),
            answer: Answer::Loaded(loaded(remaining.clone())),
        }));
        assert_eq!(controller.summaries, remaining);

        assert!(!controller.apply_stamped(StampedAnswer {
            target: second,
            generation: 1,
            load_epoch: Some(2),
            answer: Answer::Loaded(loaded(Default::default())),
        }));
        assert_eq!(controller.summaries, remaining);
    }

    #[test]
    fn profile_snapshot_populates_filterable_global_rows_across_targets() {
        let current = target();
        let other = sibling(&current);
        let make_row = |owner: &Target, title: &str, kind: u8| {
            let mut rule = rule(
                ReminderId::new(),
                owner.entity.clone(),
                draft(),
                RuleStatus::Enabled,
            )
            .unwrap();
            if kind == 3 {
                rule.status = RuleStatus::Paused;
            }
            let source = koru_store_core::OccurrenceSource::OneTime {
                moment: koru_store_core::OccurrenceMoment::Floating {
                    wall: koru_store_core::FloatingDateTime::parse("2026-09-03T09:00:00").unwrap(),
                },
            };
            let occurrence = OccurrenceKey::derive(rule.id, rule.revision, &source);
            let (effective, handled_history) = match kind {
                0 => (
                    EffectiveState::Scheduled {
                        occurrence,
                        moment: source.moment().clone(),
                    },
                    Default::default(),
                ),
                1 => (EffectiveState::WaitingForDue, Default::default()),
                2 => (
                    EffectiveState::Snoozed {
                        occurrence,
                        until_ms: 50,
                    },
                    Default::default(),
                ),
                3 => (
                    EffectiveState::Paused(koru_store_core::PauseReason::RulePaused),
                    Default::default(),
                ),
                4 => (
                    EffectiveState::Handled { occurrence },
                    std::collections::BTreeSet::from([occurrence]),
                ),
                5 => (
                    EffectiveState::Paused(koru_store_core::PauseReason::TargetMissing),
                    Default::default(),
                ),
                _ => unreachable!(),
            };
            ReminderSnapshotRow {
                stored: koru_store_core::StoredReminder {
                    rule,
                    deleted: false,
                    actions: Vec::new(),
                },
                target: koru_store_wire::reminders::ReminderTarget {
                    target: owner.entity.clone(),
                    title: title.into(),
                    source_location: "Acme / Notes".into(),
                    state: if kind == 5 {
                        koru_store_core::TargetState::Missing
                    } else {
                        koru_store_core::TargetState::Active
                    },
                    due: None,
                    projected: true,
                },
                occurrence_source: Some(source),
                reduced: koru_store_core::ReducedReminder {
                    effective,
                    handled_history,
                },
            }
        };
        let snapshot = ReminderSnapshot {
            reminders: vec![
                make_row(&current, "attention", 0),
                make_row(&other, "waiting elsewhere", 1),
                make_row(&other, "snoozed elsewhere", 2),
                make_row(&current, "paused", 3),
                make_row(&other, "history elsewhere", 4),
                make_row(&current, "target missing", 5),
            ],
            settings: koru_store_core::ReminderSettings {
                end_of_day: None,
                default_tone: ToneId::System,
                week_start: WeekStartPreference::System,
            },
            updating_workspaces: vec![other.entity.workspace()],
        };
        let loaded = project(snapshot, &current.entity).unwrap();
        let attention = loaded
            .global_rows
            .iter()
            .find(|row| row.title == "attention")
            .map(|row| GlobalAttention {
                reminder_id: row.reminder_id.clone(),
                occurrence: row.occurrence.clone().unwrap(),
                fired_at_ms: 1_700_000_000_000,
            })
            .unwrap();

        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 1;
        controller.load_epoch = 1;
        let mut authoritative = loaded.clone();
        authoritative.global_updating = false;
        assert!(controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 1,
            load_epoch: Some(1),
            answer: Answer::Loaded(authoritative),
        }));
        controller.load_epoch = 2;
        assert!(controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 1,
            load_epoch: Some(2),
            answer: Answer::Loaded(loaded),
        }));
        assert_eq!(controller.global().rows.len(), 6);
        assert!(controller.global().updating);
        assert_eq!(
            controller.global().visible().count(),
            5,
            "All excludes History"
        );
        assert!(controller.set_global_attention(vec![attention]));

        for (filter, expected) in [
            (GlobalFilter::NeedsAttention, vec!["attention"]),
            (GlobalFilter::ComingUp, vec!["waiting elsewhere"]),
            (GlobalFilter::Snoozed, vec!["snoozed elsewhere"]),
            (GlobalFilter::Paused, vec!["paused", "target missing"]),
            (GlobalFilter::History, vec!["history elsewhere"]),
        ] {
            controller.set_global_filter(filter);
            assert_eq!(
                controller
                    .global()
                    .visible()
                    .map(|row| row.title.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert!(
            controller
                .global()
                .rows
                .iter()
                .find(|row| row.title == "paused")
                .unwrap()
                .rule_paused
        );
        assert!(
            !controller
                .global()
                .rows
                .iter()
                .find(|row| row.title == "target missing")
                .unwrap()
                .rule_paused
        );

        let row = controller
            .global()
            .rows
            .iter()
            .find(|row| row.title == "attention")
            .unwrap()
            .clone();
        assert!(controller.pending.is_empty(), "Edit must not dispatch Open");
        assert!(controller.begin_global_edit(&row));
        let navigation_target = controller
            .pending_global_edit
            .as_ref()
            .unwrap()
            .target
            .clone();
        controller.open(navigation_target, request(99), context());
        let edit = controller.take_global_edit().unwrap();
        assert_eq!(edit.target.entity.to_storage(), row.target);
        assert_eq!(edit.row, row);
        let history = controller
            .global()
            .rows
            .iter()
            .find(|candidate| candidate.state == GlobalItemState::History)
            .unwrap()
            .clone();
        assert!(controller.begin_global_recreate(&history));
        let navigation_target = controller
            .pending_global_recreate
            .as_ref()
            .unwrap()
            .target
            .clone();
        controller.open(navigation_target, request(100), context());
        let recreate = controller.take_global_recreate().unwrap();
        assert_eq!(recreate.target.entity.to_storage(), history.target);
        assert_eq!(recreate.source, history.draft);
        assert_eq!(recreate.row, history);
        assert!(
            controller
                .pending
                .iter()
                .all(|queued| matches!(queued.work, Work::Load)),
            "navigation must preserve Edit as a distinct handoff"
        );
    }

    #[test]
    fn a_deleted_reminders_handled_history_projects_nothing() {
        let current = target();
        let source = koru_store_core::OccurrenceSource::OneTime {
            moment: koru_store_core::OccurrenceMoment::Floating {
                wall: koru_store_core::FloatingDateTime::parse("2026-09-03T09:00:00").unwrap(),
            },
        };
        let make_row = |title: &str, deleted: bool| {
            let rule = rule(
                ReminderId::new(),
                current.entity.clone(),
                draft(),
                RuleStatus::Enabled,
            )
            .unwrap();
            let occurrence = OccurrenceKey::derive(rule.id, rule.revision, &source);
            ReminderSnapshotRow {
                stored: koru_store_core::StoredReminder {
                    rule,
                    deleted,
                    actions: Vec::new(),
                },
                target: koru_store_wire::reminders::ReminderTarget {
                    target: current.entity.clone(),
                    title: title.into(),
                    source_location: "Acme / Notes".into(),
                    state: koru_store_core::TargetState::Active,
                    due: None,
                    projected: true,
                },
                occurrence_source: Some(source.clone()),
                reduced: koru_store_core::ReducedReminder {
                    effective: if deleted {
                        EffectiveState::Deleted
                    } else {
                        EffectiveState::Handled { occurrence }
                    },
                    handled_history: std::collections::BTreeSet::from([occurrence]),
                },
            }
        };
        let snapshot = ReminderSnapshot {
            reminders: vec![
                make_row("deleted with history", true),
                make_row("handled and live", false),
            ],
            settings: koru_store_core::ReminderSettings {
                end_of_day: None,
                default_tone: ToneId::System,
                week_start: WeekStartPreference::System,
            },
            updating_workspaces: Vec::new(),
        };
        let loaded = project(snapshot, &current.entity).unwrap();
        assert_eq!(
            loaded
                .global_rows
                .iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec!["handled and live"],
            "a deleted rule's handled occurrences are not history"
        );
        assert_eq!(loaded.history.len(), 1);
        assert!(loaded.active.is_empty());
    }

    #[test]
    fn remote_profile_change_is_coalesced_wakes_once_and_queues_a_refresh() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 5;
        controller.loaded_settings = Some(ProfileSettings {
            profile: current.profile,
            end_of_day: None,
            default_tone: Tone::System,
            week_start: WeekStartPreference::System,
        });
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        let wake: Waker = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        deliver_remote_change(
            &controller.remote_changes,
            Some(&wake),
            RemoteScope::Profile(current.profile),
        );
        deliver_remote_change(
            &controller.remote_changes,
            Some(&wake),
            RemoteScope::Profile(current.profile),
        );
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        let scopes = controller
            .remote_changes
            .lock()
            .unwrap()
            .drain()
            .collect::<Vec<_>>();
        for scope in scopes {
            let RemoteScope::Profile(profile) = scope else {
                panic!("profile event changed scope");
            };
            assert!(controller.invalidate_profile(profile));
        }
        assert_eq!(controller.loaded_settings, None);
        assert!(
            controller
                .pending
                .iter()
                .any(|queued| { queued.target == current && matches!(queued.work, Work::Load) })
        );
        assert_eq!(controller.deadline(), None);
    }

    #[test]
    fn settings_event_wakes_once_and_reaches_both_reminder_consumers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        let wake: Waker = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });

        deliver_remote_event(
            &controller.remote_changes,
            &controller.remote_settings_changes,
            Some(&wake),
            "reminder.settings.changed",
            Some(current.profile),
        );

        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(controller.remote_changes.lock().unwrap().len(), 1);
        assert!(controller.take_settings_change(Some(current.profile)));
        assert!(!controller.take_settings_change(Some(current.profile)));
    }

    #[test]
    fn event_tail_activation_queues_a_strong_refresh_after_the_initial_snapshot() {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 1;
        controller.loaded_settings = Some(ProfileSettings {
            profile: current.profile,
            end_of_day: None,
            default_tone: Tone::System,
            week_start: WeekStartPreference::System,
        });
        assert!(controller.invalidate_profile(current.profile));
        assert_eq!(controller.pending.len(), 1);
        assert!(matches!(
            controller.pending.front().unwrap().work,
            Work::Load
        ));
        assert_eq!(controller.deadline(), None);
    }

    #[test]
    fn source_and_liveness_events_refresh_waiting_and_remove_or_restore_chips() {
        let current = target();
        let key = current.entity.to_storage();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.generation = 4;
        controller.load_epoch = 1;

        assert_eq!(
            reminder_invalidation_scope("due.changed", None),
            Some(RemoteScope::AllProfiles)
        );
        assert_eq!(
            reminder_invalidation_scope("document.restored", None),
            Some(RemoteScope::AllProfiles)
        );
        assert_eq!(
            reminder_invalidation_scope("document.removed", None),
            Some(RemoteScope::AllProfiles)
        );
        assert_eq!(reminder_invalidation_scope("unrelated.event", None), None);

        let waiting = ChipSummary {
            active_count: 1,
            waiting: true,
            snoozed: false,
        };
        controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 4,
            load_epoch: Some(1),
            answer: Answer::Loaded(loaded(std::collections::HashMap::from([(
                key.clone(),
                waiting,
            )]))),
        });
        assert!(controller.invalidate_profile(current.profile));
        let due_set_epoch = controller.load_epoch;
        let active = ChipSummary {
            active_count: 1,
            waiting: false,
            snoozed: false,
        };
        controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 4,
            load_epoch: Some(due_set_epoch),
            answer: Answer::Loaded(loaded(std::collections::HashMap::from([(
                key.clone(),
                active,
            )]))),
        });
        assert_eq!(controller.summaries.get(&key), Some(&active), "due was set");

        assert!(controller.invalidate_profile(current.profile));
        let due_clear_epoch = controller.load_epoch;
        controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 4,
            load_epoch: Some(due_clear_epoch),
            answer: Answer::Loaded(loaded(std::collections::HashMap::from([(
                key.clone(),
                waiting,
            )]))),
        });
        assert_eq!(
            controller.summaries.get(&key),
            Some(&waiting),
            "due was cleared"
        );

        controller.invalidate_profile(current.profile);
        let removed_epoch = controller.load_epoch;
        controller.apply_stamped(StampedAnswer {
            target: current.clone(),
            generation: 4,
            load_epoch: Some(removed_epoch),
            answer: Answer::Loaded(loaded(Default::default())),
        });
        assert!(
            !controller.summaries.contains_key(&key),
            "target was removed"
        );

        controller.invalidate_profile(current.profile);
        let restored_epoch = controller.load_epoch;
        controller.apply_stamped(StampedAnswer {
            target: current,
            generation: 4,
            load_epoch: Some(restored_epoch),
            answer: Answer::Loaded(loaded(std::collections::HashMap::from([(
                key.clone(),
                active,
            )]))),
        });
        assert_eq!(
            controller.summaries.get(&key),
            Some(&active),
            "target was restored"
        );
    }

    #[test]
    fn event_subscription_setup_backs_off_then_recovers_without_a_ui_deadline() {
        let stop = AtomicBool::new(false);
        let mut attempts = 0;
        let mut waits = Vec::new();
        let answer = retry_with_backoff(
            &stop,
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(())
                } else {
                    Ok("connected")
                }
            },
            |delay| waits.push(delay),
        );
        assert_eq!(answer, Some("connected"));
        assert_eq!(attempts, 3);
        assert_eq!(
            waits,
            [Duration::from_millis(250), Duration::from_millis(500)]
        );
        assert_eq!(Controller::default().deadline(), None);
    }

    #[test]
    fn event_subscription_setup_honors_cancellation() {
        let stop = AtomicBool::new(true);
        let mut attempted = false;
        let answer = retry_with_backoff(
            &stop,
            || {
                attempted = true;
                Ok::<_, ()>(())
            },
            |_| {},
        );
        assert_eq!(answer, None);
        assert!(!attempted);
    }

    #[test]
    fn store_errors_cross_the_ui_boundary_as_closed_failure_codes() {
        use koru_store_core::errors::{Code, Error};

        assert_eq!(
            store_failure(Error::bare(Code::InvalidId)),
            Failure::InvalidData
        );
        assert_eq!(
            store_failure(Error::bare(Code::StoreUnreachable)),
            Failure::Unavailable
        );
        assert_eq!(
            store_failure(Error::bare(Code::Forbidden)),
            Failure::Rejected
        );
    }
}
