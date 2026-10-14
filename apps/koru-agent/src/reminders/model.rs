// SPDX-License-Identifier: MPL-2.0

use koru_native_notifications::{NativeHandle, RegistrationId};
use koru_store_core::ids::{ProfileId, ReminderId, RuleRevisionId};
use koru_store_core::personal_attributes::EntityRef;
use koru_store_core::reminders::OccurrenceKey;
use serde::{Deserialize, Serialize};

/// The device-local, content-free input to one native registration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleFingerprint {
    pub deliver_at_ms: i64,
    pub tone: koru_store_core::reminders::ToneId,
    /// Present only for a floating wall. Zoned and snoozed instants must not
    /// move when the device zone changes.
    pub floating_rules: Option<ZoneRulesFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneRulesFingerprint {
    pub canonical_zone: String,
    pub database_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationStatus {
    Registered,
    /// An already-late occurrence was handed to the native adapter once.
    Delivered,
    /// An occurrence over the missed threshold was included in a quiet batch.
    QuietSummarized,
}

/// One rebuildable device fact. No authored text or entity target belongs in
/// this type; activation resolves those from the Store after validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub profile: ProfileId,
    pub reminder: ReminderId,
    pub revision: RuleRevisionId,
    pub occurrence: OccurrenceKey,
    pub registration: RegistrationId,
    pub native_handle: Option<NativeHandle>,
    pub fingerprint: ScheduleFingerprint,
    pub status: RegistrationStatus,
    /// When this device actually handed an overdue occurrence to the native
    /// adapter, observed its deadline, or included it in a quiet summary.
    /// Registered future occurrences have no fired instant yet.
    #[serde(default)]
    pub fired_at_ms: Option<i64>,
}

/// One content-free native summary registration for a profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietSummaryRegistration {
    pub profile: ProfileId,
    pub registration: RegistrationId,
    pub native_handle: NativeHandle,
    pub count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerState {
    /// One cursor for the device journal, not one cursor per profile or event
    /// family. It advances only in the same successful replacement as entries.
    pub event_cursor: Option<u64>,
    pub entries: Vec<LedgerEntry>,
    #[serde(default)]
    pub quiet_summaries: Vec<QuietSummaryRegistration>,
}

impl LedgerState {
    pub fn entry(&self, registration: &RegistrationId) -> Option<&LedgerEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.registration == registration)
    }

    pub fn remove(&mut self, registration: &RegistrationId) -> Option<LedgerEntry> {
        let at = self
            .entries
            .iter()
            .position(|entry| &entry.registration == registration)?;
        Some(self.entries.remove(at))
    }

    pub fn replace(&mut self, entry: LedgerEntry) {
        self.remove(&entry.registration);
        self.entries.push(entry);
    }

    pub fn quiet_summary(&self, profile: ProfileId) -> Option<&QuietSummaryRegistration> {
        self.quiet_summaries
            .iter()
            .find(|summary| summary.profile == profile)
    }

    pub fn remove_quiet_summary(&mut self, profile: ProfileId) -> Option<QuietSummaryRegistration> {
        let at = self
            .quiet_summaries
            .iter()
            .position(|summary| summary.profile == profile)?;
        Some(self.quiet_summaries.remove(at))
    }

    pub fn replace_quiet_summary(&mut self, summary: QuietSummaryRegistration) {
        self.remove_quiet_summary(summary.profile);
        self.quiet_summaries.push(summary);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuietSummary {
    pub profile: ProfileId,
    pub count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureCategory {
    Store,
    Events,
    NativeUnsupported,
    NativePermission,
    NativeAsset,
    NativeInvalid,
    NativePlatform,
    TimeZone,
    Ledger,
    Preferences,
    /// At least one profile row is not yet an authoritative projection. Its
    /// native state is preserved and the journal boundary stays frozen until
    /// a bounded retry observes a current sweep.
    ProjectionUpdating,
    StaleActivation,
    UnsupportedActivation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulerFailure {
    pub category: FailureCategory,
}

impl SchedulerFailure {
    pub(crate) fn new(category: FailureCategory) -> Self {
        Self { category }
    }
}

impl FailureCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::Events => "events",
            Self::NativeUnsupported => "native_unsupported",
            Self::NativePermission => "native_permission",
            Self::NativeAsset => "native_asset",
            Self::NativeInvalid => "native_invalid",
            Self::NativePlatform => "native_platform",
            Self::TimeZone => "time_zone",
            Self::Ledger => "ledger",
            Self::Preferences => "preferences",
            Self::ProjectionUpdating => "projection_updating",
            Self::StaleActivation => "stale_activation",
            Self::UnsupportedActivation => "unsupported_activation",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerReport {
    pub delivery_enabled: bool,
    pub registered: usize,
    pub withdrawn: usize,
    pub unchanged: usize,
    pub quiet_summaries: Vec<QuietSummary>,
    pub cursor: Option<u64>,
    pub next_deadline_ms: Option<i64>,
    pub last_successful_sync_ms: Option<i64>,
    pub failure: Option<FailureCategory>,
    pub pending_count: usize,
    pub registered_count: usize,
    pub failed_count: usize,
    /// Content-free delivered or quiet-summarized occurrences which remain
    /// live in Store authority and therefore still need attention here.
    pub attention: Vec<(ProfileId, ReminderId, OccurrenceKey, i64)>,
}

impl Default for SchedulerReport {
    fn default() -> Self {
        Self {
            delivery_enabled: true,
            registered: 0,
            withdrawn: 0,
            unchanged: 0,
            quiet_summaries: Vec::new(),
            cursor: None,
            next_deadline_ms: None,
            last_successful_sync_ms: None,
            failure: None,
            pending_count: 0,
            registered_count: 0,
            failed_count: 0,
            attention: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Startup,
    Reconnect,
    NativeCapabilityChanged,
    Events,
    DeviceZoneChanged,
    Resume,
    Deadline,
}

impl Trigger {
    pub(crate) fn is_full_repair(self) -> bool {
        matches!(
            self,
            Self::Startup
                | Self::Reconnect
                | Self::NativeCapabilityChanged
                | Self::DeviceZoneChanged
                | Self::Resume
        )
    }

    pub(crate) fn may_replace_future(self) -> bool {
        matches!(self, Self::Startup | Self::Reconnect)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitPlan {
    Indefinite,
    UntilUnixMs(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationTarget {
    pub profile: ProfileId,
    pub reminder: ReminderId,
    pub occurrence: OccurrenceKey,
    pub entity: EntityRef,
}
