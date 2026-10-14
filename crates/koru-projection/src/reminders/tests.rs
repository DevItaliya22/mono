// SPDX-License-Identifier: MPL-2.0

use std::num::NonZeroU32;

use super::*;
use koru_store_core::ids::{DocumentId, ReminderId, RuleRevisionId, WorkspaceId};

fn entity() -> EntityRef {
    EntityRef::new(
        "node",
        WorkspaceId::new(),
        DocumentId::new(),
        uuid::Uuid::new_v4(),
    )
    .unwrap()
}

fn relative(offset: Option<SemanticOffset>) -> ReminderRule {
    ReminderRule {
        id: ReminderId::new(),
        target: entity(),
        revision: RuleRevisionId::new(),
        status: koru_store_core::reminders::RuleStatus::Enabled,
        schedule: OneTimeSchedule::RelativeToDue {
            relation: if offset.is_some() {
                DueRelation::Before
            } else {
                DueRelation::At
            },
            offset,
        },
        tone_override: None,
    }
}

fn instant(zone: &str, wall: &str) -> i64 {
    let rules = koru_time_zone::IanaZoneRules::parse(zone).unwrap();
    rules.instant_of(NaiveDateTime::parse_from_str(wall, "%Y-%m-%dT%H:%M:%S").unwrap())
}

fn zoned(wall: &str, zone: &str) -> TemporalValue {
    TemporalValue::Zoned {
        value: ZonedDateTime::new(instant(zone, wall), zone.to_owned()).unwrap(),
    }
}

fn moment_ms(source: &OccurrenceSource) -> i64 {
    match source.moment() {
        OccurrenceMoment::Zoned { value } => value.instant_ms(),
        OccurrenceMoment::Floating { .. } => panic!("expected zoned result"),
    }
}

#[test]
fn exact_and_calendar_offsets_diverge_across_the_spring_transition_and_gap() {
    let due = zoned("2026-03-09T02:30:00", "America/New_York");
    let exact = derive_due_source(
        &relative(Some(SemanticOffset::ExactHours(
            NonZeroU32::new(24).unwrap(),
        ))),
        &due,
    )
    .unwrap();
    let calendar = derive_due_source(
        &relative(Some(SemanticOffset::CalendarDays(
            NonZeroU32::new(1).unwrap(),
        ))),
        &due,
    )
    .unwrap();

    assert_eq!(
        moment_ms(&exact),
        instant("America/New_York", "2026-03-08T01:30:00")
    );
    assert_eq!(
        moment_ms(&calendar),
        instant("America/New_York", "2026-03-08T03:30:00"),
        "the shared gap policy moves a nonexistent 02:30 wall time forward"
    );
    assert_eq!(moment_ms(&calendar) - moment_ms(&exact), 3_600_000);
}

#[test]
fn exact_and_calendar_offsets_diverge_across_the_fall_fold() {
    let due = zoned("2026-11-02T01:30:00", "America/New_York");
    let exact = derive_due_source(
        &relative(Some(SemanticOffset::ExactHours(
            NonZeroU32::new(24).unwrap(),
        ))),
        &due,
    )
    .unwrap();
    let calendar = derive_due_source(
        &relative(Some(SemanticOffset::CalendarDays(
            NonZeroU32::new(1).unwrap(),
        ))),
        &due,
    )
    .unwrap();

    assert_eq!(
        moment_ms(&calendar),
        instant("America/New_York", "2026-11-01T01:30:00"),
        "the shared fold policy chooses the earlier instant"
    );
    assert_eq!(moment_ms(&exact) - moment_ms(&calendar), 3_600_000);
}

#[test]
fn calendar_weeks_preserve_wall_time_while_exact_minutes_preserve_elapsed_time() {
    let due = zoned("2026-03-12T09:00:00", "America/New_York");
    let calendar = derive_due_source(
        &relative(Some(SemanticOffset::CalendarWeeks(
            NonZeroU32::new(1).unwrap(),
        ))),
        &due,
    )
    .unwrap();
    let exact = derive_due_source(
        &relative(Some(SemanticOffset::ExactMinutes(
            NonZeroU32::new(7 * 24 * 60).unwrap(),
        ))),
        &due,
    )
    .unwrap();
    assert_eq!(
        moment_ms(&calendar),
        instant("America/New_York", "2026-03-05T09:00:00")
    );
    assert_eq!(moment_ms(&calendar) - moment_ms(&exact), 3_600_000);
}

#[test]
fn date_defaults_to_nine_and_date_and_floating_results_remain_floating() {
    let date = TemporalValue::Date {
        date: CalendarDate::parse("2026-09-02").unwrap(),
    };
    assert_eq!(
        derive_due_source(&relative(None), &date).unwrap().moment(),
        &OccurrenceMoment::Floating {
            wall: FloatingDateTime::parse("2026-09-02T09:00:00").unwrap()
        }
    );
    let floating = TemporalValue::Floating {
        wall: FloatingDateTime::parse("2026-09-02T18:00:00").unwrap(),
    };
    assert_eq!(
        derive_due_source(
            &relative(Some(SemanticOffset::ExactHours(
                NonZeroU32::new(2).unwrap()
            ))),
            &floating,
        )
        .unwrap()
        .moment(),
        &OccurrenceMoment::Floating {
            wall: FloatingDateTime::parse("2026-09-02T16:00:00").unwrap()
        }
    );
}

#[test]
fn at_due_keeps_a_zoned_instant_and_canonical_zone() {
    let due = zoned("2026-09-02T18:00:00", "Europe/Paris");
    let source = derive_due_source(&relative(None), &due).unwrap();
    assert!(matches!(&source, OccurrenceSource::DueRelative { due: stored, .. } if stored == &due));
    assert_eq!(
        source.moment(),
        &OccurrenceMoment::Zoned {
            value: ZonedDateTime::new(
                instant("Europe/Paris", "2026-09-02T18:00:00"),
                "Europe/Paris"
            )
            .unwrap()
        }
    );
}

#[test]
fn unknown_zones_and_checked_arithmetic_overflow_remain_unresolved() {
    for zone in ["Mars/Olympus", "US/Eastern"] {
        let invalid = TemporalValue::Zoned {
            value: ZonedDateTime::new(0, zone).unwrap(),
        };
        for reminder in [
            relative(None),
            relative(Some(SemanticOffset::ExactHours(
                NonZeroU32::new(1).unwrap(),
            ))),
            relative(Some(SemanticOffset::CalendarDays(
                NonZeroU32::new(1).unwrap(),
            ))),
        ] {
            assert!(
                derive_due_source(&reminder, &invalid).is_none(),
                "{zone} must not become actionable for {:?}",
                reminder.schedule
            );
        }
    }

    let overflow = TemporalValue::Zoned {
        value: ZonedDateTime::new(i64::MIN, "UTC").unwrap(),
    };
    assert!(
        derive_due_source(
            &relative(Some(SemanticOffset::ExactMinutes(
                NonZeroU32::new(1).unwrap()
            ))),
            &overflow
        )
        .is_none()
    );
    assert!(
        derive_due_source(
            &relative(Some(SemanticOffset::CalendarDays(
                NonZeroU32::new(1).unwrap()
            ))),
            &overflow
        )
        .is_none()
    );
}

#[test]
fn malformed_or_cleared_due_decodes_as_unresolved_and_restores_cleanly() {
    assert_eq!(decode_due(None, None, None), None);
    assert_eq!(
        decode_due(Some("date".into()), Some("not-a-date".into()), None),
        None
    );
    let first = decode_due(Some("date".into()), Some("2026-09-02".into()), None).unwrap();
    let changed = decode_due(Some("date".into()), Some("2026-09-03".into()), None).unwrap();
    assert_ne!(first, changed);
    assert_eq!(
        decode_due(Some("date".into()), Some("2026-09-02".into()), None),
        Some(first)
    );
}
