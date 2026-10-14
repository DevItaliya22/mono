// SPDX-License-Identifier: MPL-2.0

//! Persisted reminder vocabulary and its pure convergence reducer.
//!
//! The Store owns synchronized intent, not clock or OS behavior. A projection
//! adapter supplies the canonical occurrence source for due-relative rules;
//! the Agent later resolves floating moments for its own device.

use std::collections::BTreeSet;
use std::fmt;

use loro::LoroValue;
use serde::{Deserialize, Serialize};

use crate::errors::{Code, Error, Result};
use crate::ids::{ActionId, ReminderId, RuleRevisionId};
use crate::personal_attributes::EntityRef;
use crate::temporal::{FloatingDateTime, TemporalValue, ZonedDateTime};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReminderRule {
    pub id: ReminderId,
    pub target: EntityRef,
    pub revision: RuleRevisionId,
    pub status: RuleStatus,
    pub schedule: OneTimeSchedule,
    pub tone_override: Option<ToneId>,
}

impl ReminderRule {
    /// Validate database-free storage invariants.
    ///
    /// This crate deliberately cannot decide whether a syntactically valid
    /// zone names a current canonical IANA entry. Any model-aware boundary
    /// accepting an authored zoned rule must validate that name with the
    /// timezone mechanism before calling the Store service.
    pub fn validate(&self) -> Result<()> {
        if let OneTimeSchedule::RelativeToDue { relation, offset } = &self.schedule {
            match relation {
                DueRelation::At if offset.is_some() => {
                    return Err(Error::new(
                        Code::InvalidRequest,
                        "an at-due reminder has no offset",
                    ));
                }
                DueRelation::Before if offset.is_none() => {
                    return Err(Error::new(
                        Code::InvalidRequest,
                        "a before-due reminder requires an offset",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Atomic Loro register value used by the Piece 3 profile root.
    pub fn to_loro(&self) -> LoroValue {
        LoroValue::from(serde_json::to_value(self).expect("reminder rule is serializable"))
    }

    /// Unknown or malformed future shapes are absent, never partially read.
    pub fn from_loro(value: &LoroValue) -> Option<Self> {
        let value = serde_json::Value::from(value.clone());
        let rule: Self = serde_json::from_value(value).ok()?;
        rule.validate().ok()?;
        Some(rule)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    Enabled,
    Paused,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OneTimeSchedule {
    Floating {
        wall: FloatingDateTime,
    },
    Zoned {
        value: ZonedDateTime,
    },
    RelativeToDue {
        relation: DueRelation,
        offset: Option<SemanticOffset>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DueRelation {
    At,
    Before,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "unit",
    content = "amount",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SemanticOffset {
    ExactMinutes(std::num::NonZeroU32),
    ExactHours(std::num::NonZeroU32),
    CalendarDays(std::num::NonZeroU32),
    CalendarWeeks(std::num::NonZeroU32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ToneId {
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

/// Canonical `HH:MM:SS` wall time. It carries no date and no zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct LocalTime {
    hour: u8,
    minute: u8,
    second: u8,
}

impl LocalTime {
    pub fn new(hour: u8, minute: u8, second: u8) -> Option<Self> {
        (hour <= 23 && minute <= 59 && second <= 59).then_some(Self {
            hour,
            minute,
            second,
        })
    }

    pub fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 8 || bytes[2] != b':' || bytes[5] != b':' {
            return None;
        }
        let two = |at: usize| {
            let a = bytes[at];
            let b = bytes[at + 1];
            (a.is_ascii_digit() && b.is_ascii_digit()).then(|| (a - b'0') * 10 + (b - b'0'))
        };
        Self::new(two(0)?, two(3)?, two(6)?)
    }

    pub fn to_storage(self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }
}

impl TryFrom<String> for LocalTime {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Self::parse(&value).ok_or("a local time is HH:MM:SS")
    }
}

impl From<LocalTime> for String {
    fn from(value: LocalTime) -> Self {
        value.to_storage()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReminderSettings {
    pub end_of_day: Option<LocalTime>,
    pub default_tone: ToneId,
    #[serde(default)]
    pub week_start: WeekStartPreference,
}

/// The synchronized override for phrases whose meaning depends on the first
/// day of the week. System delegates to each device's regional setting;
/// authored overrides travel with the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WeekStartPreference {
    #[default]
    System,
    Monday,
    Sunday,
}

/// One reminder as read from the synchronized profile root.
///
/// Actions are already scoped by the reminder's child map. The id is still
/// checked while decoding, so a malformed value cannot escape under a key
/// naming a different reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredReminder {
    pub rule: ReminderRule,
    pub deleted: bool,
    pub actions: Vec<OccurrenceAction>,
}

/// The complete synchronized input to effective-state reduction.
///
/// [`StoredReminder`] is the public, visible-history shape. Clear history must
/// therefore remove handled actions from that value, but forgetting the same
/// action for reduction would make a one-time occurrence actionable again.
/// This in-process type carries only the cleared occurrence identities beside
/// the public record. It deliberately has no Serde or schema implementation,
/// so Clear history does not add a private fact to the HTTP contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderReductionInput {
    stored: StoredReminder,
    cleared_handled_occurrences: BTreeSet<OccurrenceKey>,
}

impl ReminderReductionInput {
    pub fn new(
        stored: StoredReminder,
        cleared_handled_occurrences: BTreeSet<OccurrenceKey>,
    ) -> Self {
        Self {
            stored,
            cleared_handled_occurrences,
        }
    }

    pub fn stored(&self) -> &StoredReminder {
        &self.stored
    }

    pub fn into_stored(self) -> StoredReminder {
        self.stored
    }

    /// Reduce all synchronized facts while exposing only uncleared handled
    /// actions as history.
    pub fn reduce(
        &self,
        target: TargetState,
        injected_source: Option<&OccurrenceSource>,
    ) -> ReducedReminder {
        reduce_with_cleared_handled(
            &self.stored.rule,
            self.stored.deleted,
            target,
            injected_source,
            ReminderActions::new(self.stored.rule.id, self.stored.actions.iter()),
            &self.cleared_handled_occurrences,
        )
    }
}

/// Content-free trigger emitted only after the profile snapshot is durable.
/// Consumers always re-read the authoritative reminder snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReminderChange {
    Rule,
    Action,
    History,
    Settings,
}

impl ReminderSettings {
    pub fn to_loro(&self) -> LoroValue {
        LoroValue::from(serde_json::to_value(self).expect("reminder settings are serializable"))
    }

    pub fn from_loro(value: &LoroValue) -> Option<Self> {
        serde_json::from_value(serde_json::Value::from(value.clone())).ok()
    }
}

/// The canonical occurrence moment. A floating moment remains a wall clock;
/// a zoned moment remains one instant plus one semantic IANA zone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OccurrenceMoment {
    Floating { wall: FloatingDateTime },
    Zoned { value: ZonedDateTime },
}

impl OneTimeSchedule {
    fn source(&self) -> Option<OccurrenceSource> {
        match self {
            Self::Floating { wall } => Some(OccurrenceSource::OneTime {
                moment: OccurrenceMoment::Floating { wall: wall.clone() },
            }),
            Self::Zoned { value } => Some(OccurrenceSource::OneTime {
                moment: OccurrenceMoment::Zoned {
                    value: value.clone(),
                },
            }),
            Self::RelativeToDue { .. } => None,
        }
    }
}

/// Canonical source injected by the model-aware projection edge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OccurrenceSource {
    OneTime {
        moment: OccurrenceMoment,
    },
    DueRelative {
        due: TemporalValue,
        moment: OccurrenceMoment,
    },
}

impl OccurrenceSource {
    pub fn due_relative(due: TemporalValue, moment: OccurrenceMoment) -> Self {
        Self::DueRelative { due, moment }
    }

    pub fn moment(&self) -> &OccurrenceMoment {
        match self {
            Self::OneTime { moment } | Self::DueRelative { moment, .. } => moment,
        }
    }

    fn append_canonical(&self, bytes: &mut Vec<u8>) {
        match self {
            Self::OneTime { moment } => {
                bytes.push(1);
                append_moment(moment, bytes);
            }
            Self::DueRelative { due, moment } => {
                bytes.push(2);
                append_temporal(due, bytes);
                append_moment(moment, bytes);
            }
        }
    }
}

/// Deterministic, versioned hash of reminder + revision + canonical source.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OccurrenceKey([u8; 32]);

impl OccurrenceKey {
    pub fn derive(
        reminder: ReminderId,
        revision: RuleRevisionId,
        source: &OccurrenceSource,
    ) -> Self {
        let mut canonical = b"koru.reminder.occurrence\0v1".to_vec();
        canonical.extend_from_slice(reminder.as_uuid().as_bytes());
        canonical.extend_from_slice(revision.as_uuid().as_bytes());
        source.append_canonical(&mut canonical);
        Self(*blake3::hash(&canonical).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_storage(self) -> String {
        format!(
            "occ1-{}",
            self.0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    }

    pub fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix("occ1-")?;
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Debug for OccurrenceKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_storage()[..17])
    }
}

impl Serialize for OccurrenceKey {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_storage())
    }
}

impl<'de> Deserialize<'de> for OccurrenceKey {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| serde::de::Error::custom("not an occurrence key"))
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for OccurrenceKey {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("OccurrenceKey")
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"string", "pattern":"^occ1-[0-9a-f]{64}$"})
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct OccurrenceAction {
    pub id: ActionId,
    pub occurrence: OccurrenceKey,
    pub kind: ActionKind,
}

impl OccurrenceAction {
    pub fn to_loro(&self) -> LoroValue {
        LoroValue::from(serde_json::to_value(self).expect("occurrence action is serializable"))
    }

    pub fn from_loro(value: &LoroValue) -> Option<Self> {
        serde_json::from_value(serde_json::Value::from(value.clone())).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionKind {
    SnoozedUntil { instant_ms: i64 },
    Handled,
}

impl<'de> Deserialize<'de> for ActionKind {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let serde_json::Value::Object(mut fields) = serde_json::Value::deserialize(deserializer)?
        else {
            return Err(serde::de::Error::custom("an action kind is an object"));
        };
        let kind = fields
            .remove("type")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| serde::de::Error::custom("an action kind has a type"))?;
        match kind.as_str() {
            "handled" if fields.is_empty() => Ok(Self::Handled),
            "snoozed_until" if fields.len() == 1 => fields
                .remove("instant_ms")
                .and_then(|value| value.as_i64())
                .map(|instant_ms| Self::SnoozedUntil { instant_ms })
                .ok_or_else(|| serde::de::Error::custom("a snooze has one integer instant_ms")),
            _ => Err(serde::de::Error::custom(
                "unknown or malformed occurrence action",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Active,
    InBin,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    RulePaused,
    TargetInBin,
    TargetMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "state",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EffectiveState {
    Deleted,
    Paused(PauseReason),
    WaitingForDue,
    Scheduled {
        occurrence: OccurrenceKey,
        moment: OccurrenceMoment,
    },
    Snoozed {
        occurrence: OccurrenceKey,
        until_ms: i64,
    },
    Handled {
        occurrence: OccurrenceKey,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReducedReminder {
    pub effective: EffectiveState,
    pub handled_history: BTreeSet<OccurrenceKey>,
}

/// Actions read from one reminder's child map in the profile manifest.
///
/// [`OccurrenceAction`] omits `ReminderId` intentionally: Piece 3 persists it
/// under the reminder's map key instead of duplicating identity in every
/// action. Carrying that key into the reducer prevents another reminder's
/// handled actions from leaking into this reminder's history.
pub struct ReminderActions<'a> {
    reminder: ReminderId,
    values: Vec<&'a OccurrenceAction>,
}

impl<'a> ReminderActions<'a> {
    pub fn new(
        reminder: ReminderId,
        values: impl IntoIterator<Item = &'a OccurrenceAction>,
    ) -> Self {
        Self {
            reminder,
            values: values.into_iter().collect(),
        }
    }
}

/// Reduce synchronized facts without reading a clock, model, Store, or OS.
///
/// `deleted` is the independent grow-only tombstone Piece 3 persists beside
/// the atomic whole-rule register. It is OR-merged across replicas; an edit can
/// replace the rule value but can never clear this fact.
pub fn reduce(
    rule: &ReminderRule,
    deleted: bool,
    target: TargetState,
    injected_source: Option<&OccurrenceSource>,
    actions: ReminderActions<'_>,
) -> ReducedReminder {
    reduce_with_cleared_handled(
        rule,
        deleted,
        target,
        injected_source,
        actions,
        &BTreeSet::new(),
    )
}

fn reduce_with_cleared_handled(
    rule: &ReminderRule,
    deleted: bool,
    target: TargetState,
    injected_source: Option<&OccurrenceSource>,
    actions: ReminderActions<'_>,
    cleared_handled_occurrences: &BTreeSet<OccurrenceKey>,
) -> ReducedReminder {
    // A malformed adapter grouping is inert, never cross-reminder history.
    let actions = if actions.reminder == rule.id {
        actions.values
    } else {
        Vec::new()
    };
    let handled_history: BTreeSet<OccurrenceKey> = actions
        .iter()
        .filter_map(|action| {
            matches!(action.kind, ActionKind::Handled).then_some(action.occurrence)
        })
        .collect();

    let terminal = |effective| ReducedReminder {
        effective,
        handled_history: handled_history.clone(),
    };
    if deleted || rule.status == RuleStatus::Deleted {
        return terminal(EffectiveState::Deleted);
    }
    if rule.status == RuleStatus::Paused {
        return terminal(EffectiveState::Paused(PauseReason::RulePaused));
    }
    match target {
        TargetState::InBin => return terminal(EffectiveState::Paused(PauseReason::TargetInBin)),
        TargetState::Missing => {
            return terminal(EffectiveState::Paused(PauseReason::TargetMissing));
        }
        TargetState::Active => {}
    }

    let owned_source = rule.schedule.source();
    let source = match (&rule.schedule, owned_source.as_ref(), injected_source) {
        (
            OneTimeSchedule::RelativeToDue { .. },
            _,
            Some(source @ OccurrenceSource::DueRelative { .. }),
        ) => source,
        (OneTimeSchedule::RelativeToDue { .. }, _, _) => {
            return terminal(EffectiveState::WaitingForDue);
        }
        (_, Some(source), _) => source,
        _ => return terminal(EffectiveState::WaitingForDue),
    };
    let occurrence = OccurrenceKey::derive(rule.id, rule.revision, source);
    let current = actions
        .iter()
        .copied()
        .filter(|action| action.occurrence == occurrence)
        .collect::<Vec<_>>();
    if cleared_handled_occurrences.contains(&occurrence)
        || current
            .iter()
            .any(|action| action.kind == ActionKind::Handled)
    {
        return terminal(EffectiveState::Handled { occurrence });
    }
    if let Some(until_ms) = current
        .iter()
        .filter_map(|action| match action.kind {
            ActionKind::SnoozedUntil { instant_ms } => Some(instant_ms),
            ActionKind::Handled => None,
        })
        .max()
    {
        return terminal(EffectiveState::Snoozed {
            occurrence,
            until_ms,
        });
    }
    terminal(EffectiveState::Scheduled {
        occurrence,
        moment: source.moment().clone(),
    })
}

fn append_string(value: &str, bytes: &mut Vec<u8>) {
    let len = u32::try_from(value.len()).expect("canonical temporal strings are bounded");
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn append_moment(moment: &OccurrenceMoment, bytes: &mut Vec<u8>) {
    match moment {
        OccurrenceMoment::Floating { wall } => {
            bytes.push(1);
            append_string(wall.as_str(), bytes);
        }
        OccurrenceMoment::Zoned { value } => {
            bytes.push(2);
            bytes.extend_from_slice(&value.instant_ms().to_be_bytes());
            append_string(value.zone(), bytes);
        }
    }
}

fn append_temporal(value: &TemporalValue, bytes: &mut Vec<u8>) {
    match value {
        TemporalValue::Date { date } => {
            bytes.push(1);
            append_string(date.as_str(), bytes);
        }
        TemporalValue::Floating { wall } => {
            bytes.push(2);
            append_string(wall.as_str(), bytes);
        }
        TemporalValue::Zoned { value } => {
            bytes.push(3);
            bytes.extend_from_slice(&value.instant_ms().to_be_bytes());
            append_string(value.zone(), bytes);
        }
    }
}

#[cfg(test)]
mod tests;
