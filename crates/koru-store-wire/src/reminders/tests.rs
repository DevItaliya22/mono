// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::{
    ActionId, ActionKind, EffectiveState, FloatingDateTime, OccurrenceKey, OccurrenceMoment,
    PauseReason,
};

fn occurrence() -> OccurrenceKey {
    OccurrenceKey::parse("occ1-439fc637c3dbb3de534f6dbb30ebdbd4c53cc4bfbe17dcf17b1517180f578843")
        .unwrap()
}

fn floating() -> OccurrenceMoment {
    OccurrenceMoment::Floating {
        wall: FloatingDateTime::parse("2026-09-02T18:00:00").unwrap(),
    }
}

#[test]
fn every_effective_state_round_trips_with_an_unambiguous_tagged_shape() {
    let occurrence = occurrence();
    let states = [
        EffectiveState::Deleted,
        EffectiveState::Paused(PauseReason::RulePaused),
        EffectiveState::Paused(PauseReason::TargetInBin),
        EffectiveState::Paused(PauseReason::TargetMissing),
        EffectiveState::WaitingForDue,
        EffectiveState::Scheduled {
            occurrence,
            moment: floating(),
        },
        EffectiveState::Snoozed {
            occurrence,
            until_ms: 123,
        },
        EffectiveState::Handled { occurrence },
    ];
    for state in states {
        let value = serde_json::to_value(&state).unwrap();
        assert!(value["state"].is_string(), "{value}");
        assert_eq!(
            serde_json::from_value::<EffectiveState>(value).unwrap(),
            state
        );
    }

    let schema = serde_json::to_value(schemars::schema_for!(EffectiveState)).unwrap();
    let text = schema.to_string();
    assert!(text.contains("state"), "{text}");
    assert!(text.contains("detail"), "{text}");
}

#[test]
fn every_action_kind_and_settings_patch_shape_round_trips() {
    let occurrence = occurrence();
    for kind in [
        ActionKind::Handled,
        ActionKind::SnoozedUntil { instant_ms: 456 },
    ] {
        let request = ActionRequest {
            action: koru_store_core::OccurrenceAction {
                id: ActionId::new(),
                occurrence,
                kind,
            },
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serde_json::from_value::<ActionRequest>(json).unwrap(),
            request
        );
    }

    for (json, expected) in [
        (serde_json::json!({}), SettingsPatch::default()),
        (
            serde_json::json!({"end_of_day": null}),
            SettingsPatch {
                end_of_day: Some(None),
                default_tone: None,
                week_start: None,
            },
        ),
        (
            serde_json::json!({"end_of_day": "17:30:00", "default_tone": "chime", "week_start": "sunday"}),
            SettingsPatch {
                end_of_day: Some(Some(koru_store_core::LocalTime::new(17, 30, 0).unwrap())),
                default_tone: Some(ToneId::Chime),
                week_start: Some(WeekStartPreference::Sunday),
            },
        ),
    ] {
        let decoded: SettingsPatch = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(
            serde_json::from_value::<SettingsPatch>(serde_json::to_value(&decoded).unwrap())
                .unwrap(),
            decoded
        );
    }
    assert_eq!(
        serde_json::from_value::<SettingsPatch>(serde_json::json!({"future": true})).unwrap(),
        SettingsPatch::default(),
        "public request DTOs ignore additive fields from newer clients"
    );
}

#[test]
fn history_scope_is_additive_and_absent_means_profile_wide() {
    let all: ClearHistoryQuery = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(all, ClearHistoryQuery::default());
    let scoped = ClearHistoryQuery {
        entity: Some("node:11111111-1111-4111-8111-111111111111:22222222-2222-4222-8222-222222222222:33333333-3333-4333-8333-333333333333".into()),
    };
    assert_eq!(
        serde_json::from_value::<ClearHistoryQuery>(serde_json::to_value(&scoped).unwrap())
            .unwrap(),
        scoped
    );
}
