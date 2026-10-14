// SPDX-License-Identifier: MPL-2.0

use std::collections::{HashMap, HashSet};

use koru_native_notifications::{
    Action, ActionLabels, ActionSet, Activation, BundledTone, Content, FailureCode,
    NativeNotifications, RegistrationId, Schedule, Sound,
};
use koru_store_core::ids::{ActionId, ProfileId, ReminderId};
use koru_store_core::personal_attributes::EntityRef;
use koru_store_core::reminders::{
    ActionKind, EffectiveState, OccurrenceAction, OccurrenceKey, OccurrenceMoment, ReminderRule,
    TargetState, ToneId,
};

use super::model::{
    ActivationTarget, FailureCategory, LedgerEntry, LedgerState, QuietSummary,
    QuietSummaryRegistration, RegistrationStatus, ScheduleFingerprint, SchedulerFailure,
    SchedulerReport, Trigger, WaitPlan,
};
use super::{Clock, EventCursor, EventWake, Ledger, ReminderStore, ZoneProvider};

const MISSED_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Debug)]
pub enum ActivationResult {
    Open(ActivationTarget),
    OpenProfile(ProfileId),
    SnoozeManagement(ActivationTarget),
    Handled(SchedulerReport),
}

struct Desired {
    profile: ProfileId,
    rule: ReminderRule,
    occurrence: OccurrenceKey,
    registration: RegistrationId,
    fingerprint: ScheduleFingerprint,
    title: String,
}

struct ReconcileResult {
    report: SchedulerReport,
    projection_updating: bool,
}

#[derive(Clone, Copy)]
struct RegistrationFailure {
    profile: ProfileId,
    reminder: ReminderId,
    category: FailureCategory,
}

pub struct Scheduler<S, N, L, C, Z, E> {
    store: S,
    native: N,
    ledger: L,
    clock: C,
    zone: Z,
    events: E,
    action_labels: ActionLabels,
    // Delivery rejection belongs to one opaque native registration, not to
    // the durable reminder model. This cache is rebuilt by every full repair;
    // keeping it beside the single scheduler owner lets an unrelated wake
    // retain honest diagnostics without adding a Failed ledger state.
    registration_failures: HashMap<RegistrationId, RegistrationFailure>,
}

impl<S, N, L, C, Z, E> Scheduler<S, N, L, C, Z, E>
where
    S: ReminderStore,
    N: NativeNotifications,
    L: Ledger,
    C: Clock,
    Z: ZoneProvider,
    E: EventCursor,
{
    pub fn new(store: S, native: N, ledger: L, clock: C, zone: Z, events: E) -> Self {
        Self {
            store,
            native,
            ledger,
            clock,
            zone,
            events,
            action_labels: ActionLabels::default(),
            registration_failures: HashMap::new(),
        }
    }

    /// Supply the labels rendered by platform-native action buttons.
    pub fn with_action_labels(mut self, action_labels: ActionLabels) -> Self {
        self.action_labels = action_labels;
        self
    }

    pub fn into_parts(self) -> (S, N, L, C, Z, E) {
        (
            self.store,
            self.native,
            self.ledger,
            self.clock,
            self.zone,
            self.events,
        )
    }

    pub(crate) fn notification_control(
        &mut self,
        request: super::NotificationControl,
    ) -> super::NotificationControlResponse {
        use super::{NotificationControl, NotificationControlResponse};
        let result = match request {
            NotificationControl::Inspect => self
                .native
                .capability()
                .map(NotificationControlResponse::Capability),
            NotificationControl::RequestAuthorization => self
                .native
                .request_authorization()
                .map(NotificationControlResponse::Authorization),
            NotificationControl::OpenSettings => self
                .native
                .open_settings()
                .map(|()| NotificationControlResponse::Done),
            NotificationControl::SendTest { tone, content } => self
                .native
                .send_test(content, sound(tone))
                .map(|()| NotificationControlResponse::Done),
            NotificationControl::SetDeliveryEnabled { .. } | NotificationControl::Repair => {
                return NotificationControlResponse::Failed(FailureCode::InvalidRequest);
            }
        };
        result.unwrap_or_else(|failure| NotificationControlResponse::Failed(failure.code))
    }

    /// Re-read authority for a wake. No event body becomes reminder truth, and
    /// the candidate cursor is committed only after native changes, target
    /// cleanup, and the content-free ledger replacement all succeed.
    pub fn run(&mut self, trigger: Trigger) -> Result<SchedulerReport, SchedulerFailure> {
        let mut state = self.ledger.load()?;
        let full_snapshot = trigger.is_full_repair()
            || state.event_cursor.is_none()
            || trigger == Trigger::Deadline;
        let mut rebuild_registration_failures = full_snapshot;
        if self.ledger.native_reset_required() {
            ensure_delivery_support(&self.native.delivery_support())?;
            self.native.reset_namespace().map_err(native_failure)?;
            // A recovered backup can still describe registrations that were
            // replaced after that commit. Once the namespace is empty, force
            // a full authoritative rebuild instead of trusting its statuses.
            state = LedgerState::default();
            rebuild_registration_failures = true;
        }
        let mut registration_failures = if rebuild_registration_failures {
            HashMap::new()
        } else {
            self.registration_failures.clone()
        };
        let durable_cursor = state.event_cursor;
        let mut profiles = Vec::new();
        let mut candidate_cursor = state.event_cursor;
        let mut deferred_failure = None;
        let mut authoritative_profile_inventory = false;

        if full_snapshot {
            // Capture the head before the snapshot. A change racing after this
            // read either appears in the snapshot or remains after the seeded
            // cursor; capturing it after the snapshot could skip both.
            let head = match self.events.head() {
                Ok(head) => Some(head),
                Err(failure) => {
                    // Store authority can still repair native state while the
                    // journal worker is restarting. Keep the cursor exactly
                    // where it was and report the failure after committing the
                    // independently successful repair, so bounded retry later
                    // captures/replays a safe boundary.
                    deferred_failure = Some(failure);
                    None
                }
            };
            profiles = self.store.profiles()?;
            authoritative_profile_inventory = true;
            if let Some(head) = head {
                candidate_cursor = Some(head);
            }
        } else if trigger == Trigger::Events {
            let mut after = state.event_cursor.unwrap_or_default();
            let mut all = false;
            let mut selected = HashSet::new();
            loop {
                let page = self.events.read_after(after)?;
                if page.next_cursor < after || (page.has_more && page.next_cursor == after) {
                    return Err(SchedulerFailure::new(FailureCategory::Events));
                }
                after = page.next_cursor;
                for wake in page.wakes {
                    match wake {
                        EventWake::Profile(profile) => {
                            selected.insert(profile);
                        }
                        EventWake::AllProfiles => all = true,
                        EventWake::Irrelevant => {}
                    }
                }
                if !page.has_more {
                    break;
                }
            }
            candidate_cursor = Some(after);
            profiles = if all {
                authoritative_profile_inventory = true;
                self.store.profiles()?
            } else {
                selected.into_iter().collect()
            };
        }

        let outcome = self.reconcile(
            &profiles,
            &mut state,
            &mut registration_failures,
            trigger,
            authoritative_profile_inventory,
        )?;
        // An uncertain projection is not an authoritative observation of the
        // event boundary. Persist independent safe repairs, but replay the
        // same journal range after the sweep becomes current.
        state.event_cursor = if outcome.projection_updating {
            durable_cursor
        } else {
            candidate_cursor
        };
        self.ledger.replace(&state)?;
        self.registration_failures = registration_failures;
        if let Some(failure) = deferred_failure {
            return Err(failure);
        }
        if outcome.projection_updating {
            return Err(SchedulerFailure::new(FailureCategory::ProjectionUpdating));
        }
        Ok(finalize_report(
            outcome.report,
            &state,
            &self.registration_failures,
            self.clock.now_ms(),
        ))
    }

    /// Withdraw every native registration owned by this root while retaining
    /// no authored content or synchronized reminder mutation. Enabling later
    /// uses a full authoritative repair rather than trusting this cache.
    pub(crate) fn disable_delivery(&mut self) -> Result<SchedulerReport, SchedulerFailure> {
        let mut state = self.ledger.load()?;
        let withdrawn = state
            .entries
            .iter()
            .filter(|entry| entry.native_handle.is_some())
            .count()
            + state.quiet_summaries.len();
        self.native.reset_namespace().map_err(native_failure)?;
        state.entries.clear();
        state.quiet_summaries.clear();
        self.ledger.replace(&state)?;
        self.registration_failures.clear();
        Ok(SchedulerReport {
            delivery_enabled: false,
            withdrawn,
            cursor: state.event_cursor,
            ..SchedulerReport::default()
        })
    }

    pub fn wait_plan(&mut self) -> Result<WaitPlan, SchedulerFailure> {
        let state = self.ledger.load()?;
        Ok(match next_deadline(&state, self.clock.now_ms()) {
            Some(deadline) => WaitPlan::UntilUnixMs(deadline),
            None => WaitPlan::Indefinite,
        })
    }

    pub(crate) fn wait_duration(
        &mut self,
    ) -> Result<Option<std::time::Duration>, SchedulerFailure> {
        let state = self.ledger.load()?;
        let now = self.clock.now_ms();
        Ok(next_deadline(&state, now).map(|deadline| {
            std::time::Duration::from_millis(
                deadline.saturating_sub(now).try_into().unwrap_or(u64::MAX),
            )
        }))
    }

    pub fn activate(
        &mut self,
        activation: Activation,
    ) -> Result<ActivationResult, SchedulerFailure> {
        activation
            .validate()
            .map_err(|_| SchedulerFailure::new(FailureCategory::UnsupportedActivation))?;
        let state = self.ledger.load()?;
        if let Some(summary) = state
            .quiet_summaries
            .iter()
            .find(|summary| summary.registration == activation.registration)
        {
            return match activation.action {
                Action::Open => Ok(ActivationResult::OpenProfile(summary.profile)),
                Action::Snooze | Action::Handled => Err(SchedulerFailure::new(
                    FailureCategory::UnsupportedActivation,
                )),
            };
        }
        let entry = state
            .entry(&activation.registration)
            .cloned()
            .ok_or_else(|| SchedulerFailure::new(FailureCategory::StaleActivation))?;
        if entry.status == RegistrationStatus::QuietSummarized {
            return Err(SchedulerFailure::new(FailureCategory::StaleActivation));
        }
        let snapshot = self.store.snapshot(entry.profile)?;
        let row = snapshot
            .reminders
            .iter()
            .find(|row| row.stored.rule.id == entry.reminder)
            .ok_or_else(|| SchedulerFailure::new(FailureCategory::StaleActivation))?;
        // A workspace projection in flight is temporary, not evidence that a
        // notification is obsolete. Preserve Open/Snooze at the worker and
        // retry after an authoritative reconciliation instead of consuming the
        // person's one Windows gesture as a terminal stale activation.
        if snapshot
            .updating_workspaces
            .contains(&row.target.target.workspace())
        {
            return Err(SchedulerFailure::new(FailureCategory::ProjectionUpdating));
        }
        if !row.target.projected
            || row.target.state != TargetState::Active
            || row.stored.rule.revision != entry.revision
        {
            return Err(SchedulerFailure::new(FailureCategory::StaleActivation));
        }
        if actionable_occurrence(&row.reduced.effective) != Some(entry.occurrence) {
            return Err(SchedulerFailure::new(FailureCategory::StaleActivation));
        }
        let target = ActivationTarget {
            profile: entry.profile,
            reminder: entry.reminder,
            occurrence: entry.occurrence,
            entity: row.target.target.clone(),
        };
        match activation.action {
            Action::Open => Ok(ActivationResult::Open(target)),
            Action::Snooze => Ok(ActivationResult::SnoozeManagement(target)),
            Action::Handled => {
                self.store.handle(
                    target.profile,
                    target.reminder,
                    OccurrenceAction {
                        id: ActionId::new(),
                        occurrence: target.occurrence,
                        kind: ActionKind::Handled,
                    },
                )?;
                let mut state = self.ledger.load()?;
                let mut registration_failures = self.registration_failures.clone();
                let outcome = self.reconcile(
                    &[target.profile],
                    &mut state,
                    &mut registration_failures,
                    Trigger::Events,
                    false,
                )?;
                self.ledger.replace(&state)?;
                self.registration_failures = registration_failures;
                if outcome.projection_updating {
                    return Err(SchedulerFailure::new(FailureCategory::ProjectionUpdating));
                }
                Ok(ActivationResult::Handled(finalize_report(
                    outcome.report,
                    &state,
                    &self.registration_failures,
                    self.clock.now_ms(),
                )))
            }
        }
    }

    fn reconcile(
        &mut self,
        profiles: &[ProfileId],
        state: &mut LedgerState,
        registration_failures: &mut HashMap<RegistrationId, RegistrationFailure>,
        trigger: Trigger,
        authoritative_profile_inventory: bool,
    ) -> Result<ReconcileResult, SchedulerFailure> {
        let now = self.clock.now_ms();
        let mut report = SchedulerReport::default();
        let mut desired = Vec::new();
        let mut protected = HashSet::new();
        let mut cleanup = Vec::<(ProfileId, EntityRef)>::new();
        let mut projection_updating = false;

        for profile in profiles {
            let snapshot = self.store.snapshot(*profile)?;
            for row in &snapshot.reminders {
                let uncertain = !row.target.projected
                    || snapshot
                        .updating_workspaces
                        .contains(&row.target.target.workspace());
                if uncertain {
                    projection_updating = true;
                    protected.insert((*profile, row.stored.rule.id));
                    continue;
                }
                if row.target.state == TargetState::Missing
                    && !cleanup
                        .iter()
                        .any(|(owner, target)| owner == profile && target == &row.target.target)
                {
                    cleanup.push((*profile, row.target.target.clone()));
                }
                // The reconciliation response carries the Store's domain
                // reduction. Do not reconstruct it from visible history:
                // Clear history deliberately hides handled actions while its
                // terminal occurrence fact must continue suppressing delivery.
                if let Some((occurrence, moment)) = desired_moment(&row.reduced.effective) {
                    desired.push(self.desired(
                        *profile,
                        &row.stored.rule,
                        occurrence,
                        moment,
                        &row.target.title,
                        &snapshot.settings,
                    )?);
                }
            }
        }

        // Target cleanup is an authoritative Store mutation and must not be
        // coupled to this device's notification capability. It is idempotent,
        // so perform it before any fallible native withdrawal or scheduling;
        // otherwise one unrelated delivery failure can leave the Missing
        // reminder live indefinitely.
        for (profile, target) in cleanup {
            self.store.delete_target(profile, &target)?;
        }

        let desired_ids = desired
            .iter()
            .map(|item| item.registration.clone())
            .collect::<HashSet<_>>();
        let selected = profiles.iter().copied().collect::<HashSet<_>>();
        registration_failures.retain(|registration, failure| {
            if !selected.contains(&failure.profile) && !authoritative_profile_inventory {
                return true;
            }
            desired_ids.contains(registration)
                || protected.contains(&(failure.profile, failure.reminder))
        });
        let stale = state
            .entries
            .iter()
            .filter(|entry| {
                (selected.contains(&entry.profile) || authoritative_profile_inventory)
                    && !desired_ids.contains(&entry.registration)
                    && !protected.contains(&(entry.profile, entry.reminder))
            })
            .map(|entry| entry.registration.clone())
            .collect::<Vec<_>>();
        for registration in stale {
            self.withdraw(&registration)?;
            state.remove(&registration);
            report.withdrawn += 1;
        }

        let mut quiet = HashMap::<ProfileId, Vec<LedgerEntry>>::new();
        let mut native_capability = None;
        for item in desired {
            let existing = state.entry(&item.registration).cloned();
            if let Some(existing) = &existing
                && existing.revision == item.rule.revision
                && existing.occurrence == item.occurrence
                && existing.fingerprint == item.fingerprint
            {
                match existing.status {
                    RegistrationStatus::Delivered | RegistrationStatus::QuietSummarized => {
                        if existing.fired_at_ms.is_none() {
                            let mut repaired = existing.clone();
                            repaired.fired_at_ms = Some(now);
                            state.replace(repaired);
                        }
                        registration_failures.remove(&item.registration);
                        report.unchanged += 1;
                        continue;
                    }
                    RegistrationStatus::Registered if item.fingerprint.deliver_at_ms > now => {
                        if trigger.may_replace_future() && existing.native_handle.is_none() {
                            // A torn/corrupt device ledger can retain intent
                            // without a usable platform handle. Cold repair may
                            // safely replace the same stable registration id.
                        } else {
                            registration_failures.remove(&item.registration);
                            report.unchanged += 1;
                            continue;
                        }
                    }
                    RegistrationStatus::Registered
                        if !matches!(trigger, Trigger::Startup | Trigger::Resume) =>
                    {
                        // A worker already running at the deadline must not
                        // fire a second notification: the OS owned the
                        // registration. Mark it processed. Startup/resume are
                        // different: the platform may have dropped the toast
                        // while Koru was absent/asleep, so they deliberately
                        // fall through to the missed-item policy below.
                        let mut delivered = existing.clone();
                        delivered.status = RegistrationStatus::Delivered;
                        delivered.fired_at_ms = Some(now);
                        state.replace(delivered);
                        registration_failures.remove(&item.registration);
                        report.unchanged += 1;
                        continue;
                    }
                    RegistrationStatus::Registered => {}
                }
            }
            if existing.is_some() {
                self.withdraw(&item.registration)?;
                state.remove(&item.registration);
                report.withdrawn += 1;
            }

            let late_by = now.saturating_sub(item.fingerprint.deliver_at_ms);
            if item.fingerprint.deliver_at_ms <= now && late_by > MISSED_WINDOW_MS {
                registration_failures.remove(&item.registration);
                quiet.entry(item.profile).or_default().push(LedgerEntry {
                    profile: item.profile,
                    reminder: item.rule.id,
                    revision: item.rule.revision,
                    occurrence: item.occurrence,
                    registration: item.registration,
                    native_handle: None,
                    fingerprint: item.fingerprint,
                    status: RegistrationStatus::QuietSummarized,
                    fired_at_ms: Some(now),
                });
                continue;
            }

            if native_capability.is_none() {
                native_capability = Some(self.native.capability().map_err(native_failure)?);
            }
            let capability = native_capability
                .as_ref()
                .expect("capability was just inspected");
            ensure_capability(capability)?;
            if item.fingerprint.tone != ToneId::System && !capability.custom_sound {
                registration_failures.insert(
                    item.registration,
                    RegistrationFailure {
                        profile: item.profile,
                        reminder: item.rule.id,
                        category: FailureCategory::NativeUnsupported,
                    },
                );
                continue;
            }
            let status = if item.fingerprint.deliver_at_ms <= now {
                RegistrationStatus::Delivered
            } else {
                RegistrationStatus::Registered
            };
            let schedule = Schedule {
                registration: item.registration.clone(),
                // Keep `now` out of the fingerprint. It is only the native
                // delivery instruction for an already-missed occurrence.
                deliver_at_unix_ms: item.fingerprint.deliver_at_ms.max(now),
                content: Content {
                    title: item.title,
                    body: None,
                },
                action_labels: self.action_labels.clone(),
                sound: sound(item.fingerprint.tone),
                actions: ActionSet::Reminder,
            };
            let native_started = std::time::Instant::now();
            tracing::info!(
                target: "koru::notification_activation",
                registration = schedule.registration.as_str(),
                deliver_at_unix_ms = schedule.deliver_at_unix_ms,
                due_now = schedule.deliver_at_unix_ms <= now,
                "Agent asked Windows to register reminder delivery"
            );
            let native_handle = match self.native.schedule(schedule) {
                Ok(handle) => {
                    tracing::info!(
                        target: "koru::notification_activation",
                        registration = item.registration.as_str(),
                        elapsed_ms = native_started.elapsed().as_millis(),
                        "Windows accepted reminder delivery registration"
                    );
                    handle
                }
                Err(failure)
                    if item.fingerprint.tone != ToneId::System
                        && matches!(
                            failure.code,
                            FailureCode::Unsupported | FailureCode::AssetMissing
                        ) =>
                {
                    registration_failures.insert(
                        item.registration,
                        RegistrationFailure {
                            profile: item.profile,
                            reminder: item.rule.id,
                            category: native_failure_category(failure.code),
                        },
                    );
                    continue;
                }
                Err(failure) => return Err(native_failure(failure)),
            };
            registration_failures.remove(&item.registration);
            state.replace(LedgerEntry {
                profile: item.profile,
                reminder: item.rule.id,
                revision: item.rule.revision,
                occurrence: item.occurrence,
                registration: item.registration,
                native_handle: Some(native_handle),
                fingerprint: item.fingerprint,
                status,
                fired_at_ms: (status == RegistrationStatus::Delivered).then_some(now),
            });
            report.registered += 1;
        }

        let mut quiet_profiles = selected.iter().copied().collect::<Vec<_>>();
        if authoritative_profile_inventory {
            quiet_profiles.extend(state.quiet_summaries.iter().map(|summary| summary.profile));
        }
        quiet_profiles.sort();
        quiet_profiles.dedup();
        for profile in quiet_profiles {
            let pending = quiet.remove(&profile).unwrap_or_default();
            let count = state
                .entries
                .iter()
                .filter(|entry| {
                    entry.profile == profile && entry.status == RegistrationStatus::QuietSummarized
                })
                .count()
                + pending.len();
            let existing = state.quiet_summary(profile).cloned();
            if pending.is_empty() && existing.as_ref().is_some_and(|value| value.count == count) {
                continue;
            }
            if count == 0 {
                if let Some(existing) = existing {
                    self.withdraw(&existing.registration)?;
                    state.remove_quiet_summary(profile);
                    report.withdrawn += 1;
                }
                continue;
            }
            if native_capability.is_none() {
                native_capability = Some(self.native.capability().map_err(native_failure)?);
            }
            ensure_capability(
                native_capability
                    .as_ref()
                    .expect("capability was just inspected"),
            )?;
            let registration = summary_registration_id(profile);
            let locale = self.store.profile_locale(profile)?;
            let native_handle = self
                .native
                .schedule(Schedule {
                    registration: registration.clone(),
                    deliver_at_unix_ms: now,
                    content: quiet_summary_content(&locale, count),
                    action_labels: self.action_labels.clone(),
                    sound: Sound::Silent,
                    actions: ActionSet::OpenOnly,
                })
                .map_err(native_failure)?;
            for entry in pending {
                state.replace(entry);
            }
            state.replace_quiet_summary(QuietSummaryRegistration {
                profile,
                registration,
                native_handle,
                count,
            });
            report.quiet_summaries.push(QuietSummary { profile, count });
        }

        report
            .quiet_summaries
            .sort_by_key(|summary| summary.profile);
        Ok(ReconcileResult {
            report,
            projection_updating,
        })
    }

    fn desired(
        &mut self,
        profile: ProfileId,
        rule: &ReminderRule,
        occurrence: OccurrenceKey,
        moment: DesiredMoment<'_>,
        title: &str,
        settings: &koru_store_core::reminders::ReminderSettings,
    ) -> Result<Desired, SchedulerFailure> {
        let (deliver_at_ms, floating_rules) = match moment {
            DesiredMoment::Floating(wall) => {
                let resolved = self.zone.resolve_wall(wall)?;
                (resolved.instant_ms, Some(resolved.rules))
            }
            DesiredMoment::Instant(instant) => (instant, None),
        };
        let tone = rule.tone_override.unwrap_or(settings.default_tone);
        Ok(Desired {
            profile,
            rule: rule.clone(),
            occurrence,
            registration: registration_id(occurrence),
            fingerprint: ScheduleFingerprint {
                deliver_at_ms,
                tone,
                floating_rules,
            },
            title: title.to_owned(),
        })
    }

    fn withdraw(&mut self, registration: &RegistrationId) -> Result<(), SchedulerFailure> {
        self.native.withdraw(registration).map_err(native_failure)
    }
}

fn ensure_capability(
    capability: &koru_native_notifications::Capability,
) -> Result<(), SchedulerFailure> {
    match capability.authorization {
        koru_native_notifications::Authorization::Denied => {
            return Err(SchedulerFailure::new(FailureCategory::NativePermission));
        }
        koru_native_notifications::Authorization::NotDetermined
        | koru_native_notifications::Authorization::Unavailable => {
            return Err(SchedulerFailure::new(FailureCategory::NativeUnsupported));
        }
        koru_native_notifications::Authorization::Allowed => {}
    }
    if !capability.actions
        || !capability.stable_replacement
        || !capability.withdrawal
        || !capability.activation_while_stopped
    {
        return Err(SchedulerFailure::new(FailureCategory::NativeUnsupported));
    }
    Ok(())
}

fn ensure_delivery_support(
    support: &koru_native_notifications::DeliverySupport,
) -> Result<(), SchedulerFailure> {
    if !support.actions
        || !support.stable_replacement
        || !support.withdrawal
        || !support.activation_while_stopped
    {
        return Err(SchedulerFailure::new(FailureCategory::NativeUnsupported));
    }
    Ok(())
}

enum DesiredMoment<'a> {
    Floating(&'a str),
    Instant(i64),
}

fn desired_moment(effective: &EffectiveState) -> Option<(OccurrenceKey, DesiredMoment<'_>)> {
    match effective {
        EffectiveState::Scheduled {
            occurrence,
            moment: OccurrenceMoment::Floating { wall },
        } => Some((*occurrence, DesiredMoment::Floating(wall.as_str()))),
        EffectiveState::Scheduled {
            occurrence,
            moment: OccurrenceMoment::Zoned { value },
        } => Some((*occurrence, DesiredMoment::Instant(value.instant_ms()))),
        EffectiveState::Snoozed {
            occurrence,
            until_ms,
        } => Some((*occurrence, DesiredMoment::Instant(*until_ms))),
        _ => None,
    }
}

fn actionable_occurrence(effective: &EffectiveState) -> Option<OccurrenceKey> {
    match effective {
        EffectiveState::Scheduled { occurrence, .. }
        | EffectiveState::Snoozed { occurrence, .. } => Some(*occurrence),
        _ => None,
    }
}

fn registration_id(occurrence: OccurrenceKey) -> RegistrationId {
    let mut value = String::with_capacity(64);
    for byte in occurrence.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    RegistrationId::new(value).expect("a lowercase digest is a portable 64-byte id")
}

pub(super) fn summary_registration_id(profile: ProfileId) -> RegistrationId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"koru.reminder.quiet-summary.v1");
    hasher.update(profile.to_storage().as_bytes());
    RegistrationId::new(hasher.finalize().to_hex().to_string())
        .expect("a BLAKE3 digest is a portable stable summary id")
}

pub(super) fn quiet_summary_content(locale: &str, count: usize) -> Content {
    let lang = koru_i18n::Lang::for_system(Some(locale));
    let count = koru_i18n::number::decimal(lang, count as u64);
    Content {
        title: koru_i18n::fill(
            koru_i18n::t(lang, koru_i18n::Key::ReminderMissedSummary),
            &[&count],
        ),
        body: None,
    }
}

fn sound(tone: ToneId) -> Sound {
    let bundled = match tone {
        ToneId::System => return Sound::System,
        ToneId::KoruDefault => BundledTone::KoruDefault,
        ToneId::Soft => BundledTone::Soft,
        ToneId::Chime => BundledTone::Chime,
        ToneId::Pulse => BundledTone::Pulse,
        ToneId::Bright => BundledTone::Bright,
        ToneId::Calm => BundledTone::Calm,
        ToneId::Drop => BundledTone::Drop,
        ToneId::Knock => BundledTone::Knock,
        ToneId::Signal => BundledTone::Signal,
        ToneId::Urgent => BundledTone::Urgent,
    };
    Sound::Bundled(bundled)
}

fn native_failure(failure: koru_native_notifications::Failure) -> SchedulerFailure {
    let category = native_failure_category(failure.code);
    tracing::warn!(
        category = category.as_str(),
        code = ?failure.code,
        detail = %failure.detail,
        "native reminder operation failed"
    );
    SchedulerFailure::new(category)
}

fn native_failure_category(code: FailureCode) -> FailureCategory {
    match code {
        FailureCode::Unsupported => FailureCategory::NativeUnsupported,
        FailureCode::PermissionDenied => FailureCategory::NativePermission,
        FailureCode::AssetMissing => FailureCategory::NativeAsset,
        FailureCode::InvalidRequest => FailureCategory::NativeInvalid,
        FailureCode::Platform => FailureCategory::NativePlatform,
    }
}

fn finalize_report(
    mut report: SchedulerReport,
    state: &LedgerState,
    registration_failures: &HashMap<RegistrationId, RegistrationFailure>,
    now: i64,
) -> SchedulerReport {
    report.delivery_enabled = true;
    report.cursor = state.event_cursor;
    report.next_deadline_ms = next_deadline(state, now);
    report.last_successful_sync_ms = Some(now);
    report.pending_count = state
        .entries
        .iter()
        .filter(|entry| entry.status == RegistrationStatus::Registered)
        .count();
    report.registered_count = state
        .entries
        .iter()
        .filter(|entry| entry.native_handle.is_some())
        .count()
        + state.quiet_summaries.len();
    report.failed_count = registration_failures.len();
    report.attention = state
        .entries
        .iter()
        .filter(|entry| entry.status != RegistrationStatus::Registered)
        .filter_map(|entry| {
            entry
                .fired_at_ms
                .map(|fired_at_ms| (entry.profile, entry.reminder, entry.occurrence, fired_at_ms))
        })
        .collect();
    // A report has one category token for transition logging. Prefer the
    // missing-asset diagnosis when both isolated classes are present because
    // it is more specific than a platform declining custom sound generally.
    report.failure = if registration_failures
        .values()
        .any(|failure| failure.category == FailureCategory::NativeAsset)
    {
        Some(FailureCategory::NativeAsset)
    } else {
        registration_failures
            .values()
            .next()
            .map(|failure| failure.category)
    };
    report
}

fn next_deadline(state: &LedgerState, now: i64) -> Option<i64> {
    state
        .entries
        .iter()
        .filter(|entry| entry.status == RegistrationStatus::Registered)
        .map(|entry| entry.fingerprint.deliver_at_ms)
        .filter(|deadline| *deadline > now)
        .min()
}
