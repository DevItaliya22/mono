// SPDX-License-Identifier: MPL-2.0

//! Public reminder request and response bodies.

use koru_store_core::ids::{RuleRevisionId, WorkspaceId};
use koru_store_core::personal_attributes::EntityRef;
use koru_store_core::reminders::{
    OccurrenceAction, OccurrenceSource, ReducedReminder, ReminderRule, ReminderSettings,
    StoredReminder, TargetState, ToneId, WeekStartPreference,
};
use koru_store_core::temporal::TemporalValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuleRequest {
    pub rule: ReminderRule,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RevisionRequest {
    pub revision: RuleRevisionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionRequest {
    pub action: OccurrenceAction,
}

/// Patch semantics preserve independent CRDT settings fields. An absent field
/// is unchanged; `end_of_day: null` explicitly clears that setting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_end_of_day_patch"
    )]
    pub end_of_day: Option<Option<koru_store_core::reminders::LocalTime>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_tone: Option<ToneId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_start: Option<WeekStartPreference>,
}

fn deserialize_end_of_day_patch<'de, D>(
    deserializer: D,
) -> Result<Option<Option<koru_store_core::reminders::LocalTime>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<koru_store_core::reminders::LocalTime>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReminderView {
    Active,
    Waiting,
    Missed,
    Upcoming,
    HandledHistory,
}

/// Device context is mandatory for missed/upcoming queries. It is explicit so
/// a remote caller is never classified through the gateway machine's zone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReminderQuery {
    pub view: ReminderView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_zone: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReminderQueryResponse {
    pub reminders: Vec<ReminderSnapshotRow>,
    pub updating_workspaces: Vec<WorkspaceId>,
}

/// The projection half of one reminder row. `projected = false` means missing
/// is not authoritative yet; consumers request/retry after the workspace sweep.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReminderTarget {
    pub target: EntityRef,
    pub title: String,
    /// Root-first workspace/document location of the source object. This is
    /// Store-to-desktop presentation data and never crosses the Agent wire.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_location: String,
    pub state: TargetState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<TemporalValue>,
    pub projected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReminderSnapshotRow {
    pub stored: StoredReminder,
    pub target: ReminderTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence_source: Option<OccurrenceSource>,
    pub reduced: ReducedReminder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReminderSnapshot {
    pub reminders: Vec<ReminderSnapshotRow>,
    pub settings: ReminderSettings,
    pub updating_workspaces: Vec<WorkspaceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClearedHistory {
    pub cleared: usize,
}

/// Optional scope on the additive reminder-history DELETE route.
///
/// Absence preserves the original profile-wide operation for older clients.
/// A present canonical entity reference limits clear facts to that object.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClearHistoryQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
}

#[cfg(test)]
mod tests;
