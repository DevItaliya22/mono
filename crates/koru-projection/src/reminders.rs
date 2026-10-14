// SPDX-License-Identifier: MPL-2.0

//! Model-aware reminder projection adapter.
//!
//! Store-core persists semantic rules without a document model or timezone
//! database. This edge joins canonical due values and applies their semantics;
//! floating results remain floating for each device to resolve later.

use chrono::{Duration, NaiveDateTime, NaiveTime};
use koru_core::time::ZoneRules;
use koru_store_core::personal_attributes::EntityRef;
use koru_store_core::reminders::{
    DueRelation, OccurrenceMoment, OccurrenceSource, OneTimeSchedule, ReminderRule, SemanticOffset,
    TargetState,
};
use koru_store_core::temporal::{CalendarDate, FloatingDateTime, TemporalValue, ZonedDateTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderTargetRow {
    pub target: EntityRef,
    pub title: String,
    /// Root-first workspace/document location of the source object. For a
    /// document target this excludes the document itself because `title`
    /// already names it; nested targets include their containing document.
    pub source_location: String,
    pub state: TargetState,
    pub due: Option<TemporalValue>,
    /// Whether the document is listed in this exact workspace. Absence is
    /// authoritative only after the caller completes its workspace sweep.
    pub listed: bool,
    /// False while the target document is absent/stale in the projection.
    pub projected: bool,
}

/// Derive the canonical due-relative source. Unknown zones and arithmetic
/// overflow stay unresolved; callers expose WaitingForDue and retry after the
/// authoritative source changes rather than inventing a timestamp.
pub fn derive_due_source(rule: &ReminderRule, due: &TemporalValue) -> Option<OccurrenceSource> {
    let OneTimeSchedule::RelativeToDue { relation, offset } = &rule.schedule else {
        return None;
    };
    let offset = match relation {
        DueRelation::At => None,
        DueRelation::Before => Some((*offset)?),
    };
    let moment = match due {
        TemporalValue::Date { date } => {
            let date = chrono::NaiveDate::parse_from_str(date.as_str(), "%Y-%m-%d").ok()?;
            let wall = date.and_time(NaiveTime::from_hms_opt(9, 0, 0)?);
            OccurrenceMoment::Floating {
                wall: store_floating(subtract_wall(wall, offset)?),
            }
        }
        TemporalValue::Floating { wall } => {
            let wall = NaiveDateTime::parse_from_str(wall.as_str(), "%Y-%m-%dT%H:%M:%S").ok()?;
            OccurrenceMoment::Floating {
                wall: store_floating(subtract_wall(wall, offset)?),
            }
        }
        TemporalValue::Zoned { value } => {
            let rules = koru_time_zone::IanaZoneRules::parse(value.zone()).ok()?;
            if rules.canonical_name().as_str() != value.zone() {
                return None;
            }
            let instant_ms = match offset {
                None => value.instant_ms(),
                Some(SemanticOffset::ExactMinutes(amount)) => value
                    .instant_ms()
                    .checked_sub(i64::from(amount.get()).checked_mul(60_000)?)?,
                Some(SemanticOffset::ExactHours(amount)) => value
                    .instant_ms()
                    .checked_sub(i64::from(amount.get()).checked_mul(3_600_000)?)?,
                Some(SemanticOffset::CalendarDays(_)) | Some(SemanticOffset::CalendarWeeks(_)) => {
                    chrono::DateTime::from_timestamp_millis(value.instant_ms())?;
                    let days = match offset? {
                        SemanticOffset::CalendarDays(value) => i64::from(value.get()),
                        SemanticOffset::CalendarWeeks(value) => {
                            i64::from(value.get()).checked_mul(7)?
                        }
                        _ => unreachable!(),
                    };
                    let wall = rules.wall_of(value.instant_ms());
                    let moved = wall.checked_sub_signed(Duration::days(days))?;
                    rules.instant_of(moved)
                }
            };
            OccurrenceMoment::Zoned {
                value: ZonedDateTime::new(instant_ms, value.zone().to_owned()).ok()?,
            }
        }
    };
    Some(OccurrenceSource::due_relative(due.clone(), moment))
}

fn subtract_wall(wall: NaiveDateTime, offset: Option<SemanticOffset>) -> Option<NaiveDateTime> {
    let duration = match offset {
        None => Duration::zero(),
        Some(SemanticOffset::ExactMinutes(value)) => Duration::minutes(i64::from(value.get())),
        Some(SemanticOffset::ExactHours(value)) => Duration::hours(i64::from(value.get())),
        Some(SemanticOffset::CalendarDays(value)) => Duration::days(i64::from(value.get())),
        Some(SemanticOffset::CalendarWeeks(value)) => {
            Duration::days(i64::from(value.get()).checked_mul(7)?)
        }
    };
    wall.checked_sub_signed(duration)
}

fn store_floating(wall: NaiveDateTime) -> FloatingDateTime {
    FloatingDateTime::parse(&wall.format("%Y-%m-%dT%H:%M:%S").to_string())
        .expect("chrono produced canonical floating time")
}

pub(crate) fn decode_due(
    kind: Option<String>,
    value: Option<String>,
    zone: Option<String>,
) -> Option<TemporalValue> {
    match (kind.as_deref(), value, zone) {
        (Some("date"), Some(value), None) => {
            CalendarDate::parse(&value).map(|date| TemporalValue::Date { date })
        }
        (Some("datetime_floating"), Some(value), None) => {
            FloatingDateTime::parse(&value).map(|wall| TemporalValue::Floating { wall })
        }
        (Some("datetime_zoned"), Some(value), Some(zone)) => value
            .parse::<i64>()
            .ok()
            .and_then(|instant_ms| ZonedDateTime::new(instant_ms, zone).ok())
            .map(|value| TemporalValue::Zoned { value }),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
