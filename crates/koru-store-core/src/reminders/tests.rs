// SPDX-License-Identifier: MPL-2.0

use std::num::NonZeroU32;

use super::*;
use crate::{DocumentId, WorkspaceId};

fn reminder_id() -> ReminderId {
    ReminderId::parse("11111111-1111-4111-8111-111111111111").unwrap()
}

fn revision_id() -> RuleRevisionId {
    RuleRevisionId::parse("22222222-2222-4222-8222-222222222222").unwrap()
}

fn action_id(value: u8) -> ActionId {
    ActionId::parse(&format!("33333333-3333-4333-8333-3333333333{value:02}")).unwrap()
}

fn entity() -> EntityRef {
    EntityRef::new(
        "node",
        WorkspaceId::parse("44444444-4444-4444-8444-444444444444").unwrap(),
        DocumentId::parse("55555555-5555-4555-8555-555555555555").unwrap(),
        uuid::Uuid::parse_str("66666666-6666-4666-8666-666666666666").unwrap(),
    )
    .unwrap()
}

fn floating(value: &str) -> FloatingDateTime {
    FloatingDateTime::parse(value).unwrap()
}

fn enabled(schedule: OneTimeSchedule) -> ReminderRule {
    ReminderRule {
        id: reminder_id(),
        target: entity(),
        revision: revision_id(),
        status: RuleStatus::Enabled,
        schedule,
        tone_override: None,
    }
}

fn relative_rule() -> ReminderRule {
    enabled(OneTimeSchedule::RelativeToDue {
        relation: DueRelation::Before,
        offset: Some(SemanticOffset::ExactHours(NonZeroU32::new(2).unwrap())),
    })
}

fn due_source(date: &str, moment: &str) -> OccurrenceSource {
    OccurrenceSource::due_relative(
        TemporalValue::Date {
            date: crate::CalendarDate::parse(date).unwrap(),
        },
        OccurrenceMoment::Floating {
            wall: floating(moment),
        },
    )
}

#[test]
fn occurrence_key_bytes_are_a_stable_golden() {
    let source = due_source("2026-08-30", "2026-08-30T07:00:00");
    let key = OccurrenceKey::derive(reminder_id(), revision_id(), &source);
    assert_eq!(
        key.to_storage(),
        "occ1-439fc637c3dbb3de534f6dbb30ebdbd4c53cc4bfbe17dcf17b1517180f578843"
    );
    assert_eq!(OccurrenceKey::parse(&key.to_storage()), Some(key));
    assert!(OccurrenceKey::parse(&key.to_storage().to_uppercase()).is_none());
}

#[test]
fn rule_action_and_settings_codecs_skip_unknown_or_malformed_shapes() {
    let rule = enabled(OneTimeSchedule::Floating {
        wall: floating("2026-08-30T09:00:00"),
    });
    assert_eq!(ReminderRule::from_loro(&rule.to_loro()), Some(rule.clone()));

    let action = OccurrenceAction {
        id: action_id(1),
        occurrence: OccurrenceKey::derive(rule.id, rule.revision, &rule.schedule.source().unwrap()),
        kind: ActionKind::Handled,
    };
    assert_eq!(OccurrenceAction::from_loro(&action.to_loro()), Some(action));

    let settings = ReminderSettings {
        end_of_day: LocalTime::parse("17:30:00"),
        default_tone: ToneId::Chime,
        week_start: WeekStartPreference::Sunday,
    };
    assert_eq!(
        ReminderSettings::from_loro(&settings.to_loro()),
        Some(settings)
    );

    for future in [
        serde_json::json!({"type":"recurring","interval":2}),
        serde_json::json!({"id": reminder_id(), "status":"future"}),
        serde_json::json!({"end_of_day":"25:00:00","default_tone":"system"}),
    ] {
        let value = LoroValue::from(future);
        assert!(ReminderRule::from_loro(&value).is_none());
        assert!(OccurrenceAction::from_loro(&value).is_none());
        assert!(ReminderSettings::from_loro(&value).is_none());
    }

    let mut malformed_rule = serde_json::to_value(&rule).unwrap();
    malformed_rule["schedule"]["future_field"] = serde_json::json!(true);
    assert!(ReminderRule::from_loro(&LoroValue::from(malformed_rule)).is_none());

    let malformed_zone = serde_json::json!({
        "id": reminder_id(),
        "target": entity(),
        "revision": revision_id(),
        "status": "enabled",
        "schedule": {
            "type": "zoned",
            "value": {"instant_ms": 42, "zone": "+02:00"}
        },
        "tone_override": null
    });
    assert!(ReminderRule::from_loro(&LoroValue::from(malformed_zone)).is_none());

    let mut malformed_action = serde_json::to_value(&OccurrenceAction {
        id: action_id(4),
        occurrence: OccurrenceKey::derive(rule.id, rule.revision, &rule.schedule.source().unwrap()),
        kind: ActionKind::Handled,
    })
    .unwrap();
    malformed_action["kind"]["future_field"] = serde_json::json!(true);
    assert!(OccurrenceAction::from_loro(&LoroValue::from(malformed_action)).is_none());

    let malformed_settings = serde_json::json!({
        "end_of_day": "17:30:00",
        "default_tone": "system",
        "future_field": true
    });
    assert!(ReminderSettings::from_loro(&LoroValue::from(malformed_settings)).is_none());

    let source_with_extra = serde_json::json!({
        "type": "due_relative",
        "due": {"type": "date", "date": "2026-08-30"},
        "moment": {"type": "floating", "wall": "2026-08-30T07:00:00"},
        "future_field": true
    });
    assert!(serde_json::from_value::<OccurrenceSource>(source_with_extra).is_err());

    let moment_with_extra = serde_json::json!({
        "type": "floating",
        "wall": "2026-08-30T07:00:00",
        "future_field": true
    });
    assert!(serde_json::from_value::<OccurrenceMoment>(moment_with_extra).is_err());
}

#[test]
fn reminder_and_personal_temporal_values_have_identical_refusal_edges() {
    for bad in [
        "2026-02-29T09:00:00",
        "2026-08-30T24:00:00",
        "2026-08-30T09:00:00Z",
    ] {
        assert!(FloatingDateTime::parse(bad).is_none(), "{bad}");
        assert!(
            crate::PersonalAttributeValue::DatetimeFloating { wall: bad.into() }
                .validate()
                .is_err(),
            "{bad}"
        );
    }
    for bad in ["+02:00", "UTC+2", "America/New York"] {
        assert!(ZonedDateTime::new(42, bad).is_err(), "{bad}");
        assert!(
            crate::PersonalAttributeValue::DatetimeZoned {
                ms: 42,
                tz: bad.into()
            }
            .validate()
            .is_err(),
            "{bad}"
        );
    }
}

#[test]
fn waiting_bin_pause_and_delete_win_without_a_clock() {
    let rule = relative_rule();
    assert_eq!(
        reduce(
            &rule,
            false,
            TargetState::Active,
            None,
            ReminderActions::new(rule.id, []),
        )
        .effective,
        EffectiveState::WaitingForDue
    );
    assert_eq!(
        reduce(
            &rule,
            false,
            TargetState::InBin,
            None,
            ReminderActions::new(rule.id, []),
        )
        .effective,
        EffectiveState::Paused(PauseReason::TargetInBin)
    );
    assert_eq!(
        reduce(
            &rule,
            true,
            TargetState::Active,
            Some(&due_source("2026-08-30", "2026-08-30T07:00:00")),
            ReminderActions::new(rule.id, [])
        )
        .effective,
        EffectiveState::Deleted
    );

    let mut deleted_rule = rule.clone();
    deleted_rule.status = RuleStatus::Deleted;
    assert_eq!(
        reduce(
            &deleted_rule,
            false,
            TargetState::Active,
            Some(&due_source("2026-08-30", "2026-08-30T07:00:00")),
            ReminderActions::new(deleted_rule.id, []),
        )
        .effective,
        EffectiveState::Deleted
    );
}

#[test]
fn actions_grouped_under_another_reminder_are_inert() {
    let rule = relative_rule();
    let source = due_source("2026-08-30", "2026-08-30T07:00:00");
    let occurrence = OccurrenceKey::derive(rule.id, rule.revision, &source);
    let handled = OccurrenceAction {
        id: action_id(1),
        occurrence,
        kind: ActionKind::Handled,
    };
    let other = ReminderId::parse("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    let reduced = reduce(
        &rule,
        false,
        TargetState::Active,
        Some(&source),
        ReminderActions::new(other, [&handled]),
    );
    assert!(reduced.handled_history.is_empty());
    assert_eq!(
        reduced.effective,
        EffectiveState::Scheduled {
            occurrence,
            moment: source.moment().clone(),
        }
    );
}

#[test]
fn handled_wins_and_otherwise_the_maximum_snooze_wins_in_every_order() {
    let rule = relative_rule();
    let source = due_source("2026-08-30", "2026-08-30T07:00:00");
    let occurrence = OccurrenceKey::derive(rule.id, rule.revision, &source);
    let actions = [
        OccurrenceAction {
            id: action_id(1),
            occurrence,
            kind: ActionKind::SnoozedUntil { instant_ms: 100 },
        },
        OccurrenceAction {
            id: action_id(2),
            occurrence,
            kind: ActionKind::SnoozedUntil { instant_ms: 300 },
        },
        OccurrenceAction {
            id: action_id(3),
            occurrence,
            kind: ActionKind::Handled,
        },
    ];
    for order in [
        [0, 1, 2],
        [2, 0, 1],
        [1, 2, 0],
        [2, 1, 0],
        [0, 2, 1],
        [1, 0, 2],
    ] {
        let input = order.map(|index| &actions[index]);
        assert_eq!(
            reduce(
                &rule,
                false,
                TargetState::Active,
                Some(&source),
                ReminderActions::new(rule.id, input),
            )
            .effective,
            EffectiveState::Handled { occurrence },
            "{order:?}"
        );
    }
    for order in [[0, 1], [1, 0]] {
        let input = order.map(|index| &actions[index]);
        assert_eq!(
            reduce(
                &rule,
                false,
                TargetState::Active,
                Some(&source),
                ReminderActions::new(rule.id, input),
            )
            .effective,
            EffectiveState::Snoozed {
                occurrence,
                until_ms: 300
            },
            "{order:?}"
        );
    }
}

#[test]
fn two_replica_action_unions_reduce_identically() {
    let rule = relative_rule();
    let source = due_source("2026-08-30", "2026-08-30T07:00:00");
    let occurrence = OccurrenceKey::derive(rule.id, rule.revision, &source);
    let a = OccurrenceAction {
        id: action_id(1),
        occurrence,
        kind: ActionKind::SnoozedUntil { instant_ms: 200 },
    };
    let b = OccurrenceAction {
        id: action_id(2),
        occurrence,
        kind: ActionKind::Handled,
    };
    let from_a = reduce(
        &rule,
        false,
        TargetState::Active,
        Some(&source),
        ReminderActions::new(rule.id, [&a, &b]),
    );
    let from_b = reduce(
        &rule,
        false,
        TargetState::Active,
        Some(&source),
        ReminderActions::new(rule.id, [&b, &a]),
    );
    assert_eq!(from_a, from_b);
    assert_eq!(from_a.effective, EffectiveState::Handled { occurrence });
}

#[test]
fn a_due_change_gets_a_fresh_key_and_old_snooze_is_not_actionable() {
    let rule = relative_rule();
    let old_source = due_source("2026-08-30", "2026-08-30T07:00:00");
    let new_source = due_source("2026-08-31", "2026-08-31T07:00:00");
    let old_key = OccurrenceKey::derive(rule.id, rule.revision, &old_source);
    let new_key = OccurrenceKey::derive(rule.id, rule.revision, &new_source);
    assert_ne!(old_key, new_key);
    let due_only_changed = due_source("2026-08-31", "2026-08-30T07:00:00");
    assert_ne!(
        old_key,
        OccurrenceKey::derive(rule.id, rule.revision, &due_only_changed),
        "the canonical due itself is part of a relative occurrence key"
    );

    let old_snooze = OccurrenceAction {
        id: action_id(1),
        occurrence: old_key,
        kind: ActionKind::SnoozedUntil {
            instant_ms: i64::MAX,
        },
    };
    assert_eq!(
        reduce(
            &rule,
            false,
            TargetState::Active,
            Some(&new_source),
            ReminderActions::new(rule.id, [&old_snooze])
        )
        .effective,
        EffectiveState::Scheduled {
            occurrence: new_key,
            moment: new_source.moment().clone(),
        }
    );
}

#[test]
fn handled_history_survives_while_superseded_snoozes_do_not() {
    let rule = relative_rule();
    let old_source = due_source("2026-08-30", "2026-08-30T07:00:00");
    let current_source = due_source("2026-08-31", "2026-08-31T07:00:00");
    let old = OccurrenceKey::derive(rule.id, rule.revision, &old_source);
    let current = OccurrenceKey::derive(rule.id, rule.revision, &current_source);
    let actions = [
        OccurrenceAction {
            id: action_id(1),
            occurrence: old,
            kind: ActionKind::Handled,
        },
        OccurrenceAction {
            id: action_id(2),
            occurrence: old,
            kind: ActionKind::SnoozedUntil {
                instant_ms: i64::MAX,
            },
        },
    ];
    let reduced = reduce(
        &rule,
        false,
        TargetState::Active,
        Some(&current_source),
        ReminderActions::new(rule.id, &actions),
    );
    assert_eq!(reduced.handled_history, BTreeSet::from([old]));
    assert_eq!(
        reduced.effective,
        EffectiveState::Scheduled {
            occurrence: current,
            moment: current_source.moment().clone()
        }
    );
}
