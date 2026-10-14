// SPDX-License-Identifier: MPL-2.0

//! Reminder scheduling owned by the long-lived desktop Agent.
//!
//! The Store remains authoritative and native registrations remain rebuildable
//! device state. This module is deliberately port-driven: deterministic tests
//! exercise the same reconciler as the shipping Store/event/timezone adapters,
//! while Piece 5 supplies the platform notification and disk-ledger adapters.

mod ledger;
mod model;
mod preferences;
mod reconcile;
mod worker;

pub(crate) use ledger::FileLedger;
pub use model::{
    ActivationTarget, FailureCategory, LedgerEntry, LedgerState, QuietSummary,
    QuietSummaryRegistration, RegistrationStatus, ScheduleFingerprint, SchedulerFailure,
    SchedulerReport, Trigger, WaitPlan, ZoneRulesFingerprint,
};
pub(crate) use preferences::{
    DeliveryPreferences, FileDeliveryPreferences, MemoryDeliveryPreferences,
};
pub use reconcile::{ActivationResult, Scheduler};
pub(crate) use worker::ReminderControlIngress;
pub(crate) use worker::ReminderWakeIngress;
pub(crate) use worker::{ActivationIngressError, ReminderActivationIngress};
pub use worker::{
    Command, NotificationControl, NotificationControlQueueFull, NotificationControlReply,
    NotificationControlResponse, ReminderWorker,
};

use koru_store_client::ReminderSnapshot;
use koru_store_core::ids::{ProfileId, ReminderId};
use koru_store_core::personal_attributes::EntityRef;
use koru_store_core::reminders::OccurrenceAction;

pub trait Clock: Send {
    fn now_ms(&mut self) -> i64;
}

pub trait ZoneProvider: Send {
    fn resolve_wall(&mut self, wall: &str) -> Result<LocalResolution, SchedulerFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalResolution {
    pub instant_ms: i64,
    pub rules: ZoneRulesFingerprint,
}

pub trait ReminderStore: Send {
    fn profiles(&mut self) -> Result<Vec<ProfileId>, SchedulerFailure>;
    fn profile_locale(&mut self, profile: ProfileId) -> Result<String, SchedulerFailure>;
    fn snapshot(&mut self, profile: ProfileId) -> Result<ReminderSnapshot, SchedulerFailure>;
    fn handle(
        &mut self,
        profile: ProfileId,
        reminder: ReminderId,
        action: OccurrenceAction,
    ) -> Result<(), SchedulerFailure>;
    fn delete_target(
        &mut self,
        profile: ProfileId,
        target: &EntityRef,
    ) -> Result<(), SchedulerFailure>;
}

pub trait Ledger: Send {
    fn load(&mut self) -> Result<LedgerState, SchedulerFailure>;
    fn replace(&mut self, state: &LedgerState) -> Result<(), SchedulerFailure>;

    /// True after `load` had to use anything other than a valid primary.
    /// Callers must clear their root's native namespace before rebuilding,
    /// because a missing/corrupt cache cannot name obsolete OS registrations.
    fn native_reset_required(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventWake {
    Profile(ProfileId),
    AllProfiles,
    Irrelevant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPage {
    pub next_cursor: u64,
    pub wakes: Vec<EventWake>,
    pub has_more: bool,
}

pub trait EventCursor: Send {
    fn head(&mut self) -> Result<u64, SchedulerFailure>;
    fn read_after(&mut self, cursor: u64) -> Result<EventPage, SchedulerFailure>;
}

/// Wall clock injected in tests; the production worker reads it only while it
/// is already off the winit thread.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&mut self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default()
    }
}

pub struct SystemZoneProvider;

impl ZoneProvider for SystemZoneProvider {
    fn resolve_wall(&mut self, wall: &str) -> Result<LocalResolution, SchedulerFailure> {
        let resolved = koru_time_zone::SystemZone::detect()
            .resolve_canonical_wall(wall)
            .map_err(|_| SchedulerFailure::new(FailureCategory::TimeZone))?;
        Ok(LocalResolution {
            instant_ms: resolved.instant_ms,
            rules: ZoneRulesFingerprint {
                canonical_zone: resolved.rules.canonical_zone,
                database_version: resolved.rules.database_version.to_owned(),
            },
        })
    }
}

/// Shipping typed Store adapter. It never opens a document and never makes a
/// request per reminder while reading; the reconciliation route is the batched
/// projection join from Piece 3.
pub struct StoreClientPort {
    client: koru_api_client::Client,
    locales: std::collections::HashMap<ProfileId, String>,
    units: std::collections::HashMap<(ProfileId, ReminderId), koru_store_core::WorkspaceId>,
}

impl StoreClientPort {
    pub fn new(client: koru_api_client::Client) -> Self {
        Self {
            client,
            locales: std::collections::HashMap::new(),
            units: std::collections::HashMap::new(),
        }
    }
}

impl ReminderStore for StoreClientPort {
    fn profiles(&mut self) -> Result<Vec<ProfileId>, SchedulerFailure> {
        let profiles = self
            .client
            .profiles()
            .map_err(|_| SchedulerFailure::new(FailureCategory::Store))?;
        self.locales = profiles
            .iter()
            .map(|profile| (profile.id, profile.locale.as_str().to_owned()))
            .collect();
        Ok(profiles.into_iter().map(|profile| profile.id).collect())
    }

    fn profile_locale(&mut self, profile: ProfileId) -> Result<String, SchedulerFailure> {
        self.locales
            .get(&profile)
            .cloned()
            .ok_or_else(|| SchedulerFailure::new(FailureCategory::Store))
    }

    fn snapshot(&mut self, profile: ProfileId) -> Result<ReminderSnapshot, SchedulerFailure> {
        let snapshot = self
            .client
            .reminder_reconciliation(profile)
            .map_err(|_| SchedulerFailure::new(FailureCategory::Store))?;
        self.units.retain(|(owner, _), _| *owner != profile);
        for row in &snapshot.reminders {
            self.units.insert(
                (profile, row.stored.rule.id),
                row.stored.rule.target.workspace(),
            );
        }
        Ok(snapshot)
    }

    fn handle(
        &mut self,
        profile: ProfileId,
        reminder: ReminderId,
        action: OccurrenceAction,
    ) -> Result<(), SchedulerFailure> {
        let workspace = self
            .units
            .get(&(profile, reminder))
            .copied()
            .ok_or_else(|| SchedulerFailure::new(FailureCategory::Store))?;
        self.client
            .handle_reminder_in(profile, workspace, reminder, action)
            .map(|_| ())
            .map_err(|_| SchedulerFailure::new(FailureCategory::Store))
    }

    fn delete_target(
        &mut self,
        profile: ProfileId,
        target: &EntityRef,
    ) -> Result<(), SchedulerFailure> {
        // Re-read before the irreversible tombstones. This is the second
        // authoritative projection observation: a cold or currently sweeping
        // workspace can never turn a temporary absence into deletion.
        let snapshot = self.snapshot(profile)?;
        if snapshot.updating_workspaces.contains(&target.workspace())
            || snapshot.reminders.iter().any(|row| {
                row.target.target == *target
                    && (!row.target.projected
                        || row.target.state != koru_store_core::reminders::TargetState::Missing)
            })
        {
            return Err(SchedulerFailure::new(FailureCategory::Store));
        }
        for row in snapshot
            .reminders
            .into_iter()
            .filter(|row| row.target.target == *target && !row.stored.deleted)
        {
            self.client
                .delete_reminder(profile, row.stored.rule.id)
                .map_err(|_| SchedulerFailure::new(FailureCategory::Store))?;
        }
        Ok(())
    }
}

/// The one global journal cursor used by reminders. Filtering is local so
/// workspace lifecycle facts and every reminder family share ordering.
pub struct EventsClientCursor {
    client: koru_api_client::Client,
}

impl EventsClientCursor {
    pub fn new(client: koru_api_client::Client) -> Self {
        Self { client }
    }
}

impl EventCursor for EventsClientCursor {
    fn head(&mut self) -> Result<u64, SchedulerFailure> {
        self.client
            .events()
            .status()
            .map(|status| status.last_seq)
            .map_err(|_| SchedulerFailure::new(FailureCategory::Events))
    }

    fn read_after(&mut self, cursor: u64) -> Result<EventPage, SchedulerFailure> {
        let page = self
            .client
            .events()
            .read(Some(cursor), &koru_api_client::Filter::all(), 1_000)
            .map_err(|_| SchedulerFailure::new(FailureCategory::Events))?;
        let has_more = page.events.len() == 1_000;
        let wakes = page
            .events
            .iter()
            .map(|stored| event_wake(&stored.envelope))
            .collect();
        Ok(EventPage {
            next_cursor: page.next_after,
            wakes,
            has_more,
        })
    }
}

fn event_wake(event: &koru_events::Envelope) -> EventWake {
    match event.name.as_str() {
        "due.changed" | "entity.removed" | "document.created" | "document.trashed"
        | "document.restored" | "document.removed" | "workspace.created" | "workspace.deleted"
        | "workspace.observed" => EventWake::AllProfiles,
        "reminder.rule.changed"
        | "reminder.action.appended"
        | "reminder.history.cleared"
        | "reminder.settings.changed" => event
            .scope
            .profile_id
            .map(|profile| EventWake::Profile(ProfileId::from_uuid(profile.0)))
            .unwrap_or(EventWake::AllProfiles),
        _ => EventWake::Irrelevant,
    }
}

#[cfg(test)]
mod tests;
