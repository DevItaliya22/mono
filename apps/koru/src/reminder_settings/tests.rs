// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::agent::AgentLink;

fn profile() -> ProfileId {
    ProfileId::new()
}

fn ready(tone: ToneId) -> (ReminderPreferences, ProfileId, u64) {
    let profile = profile();
    let mut state = ReminderPreferences::default();
    let effects = state.select_profile(profile);
    let generation = match effects[0] {
        Effect::LoadSettings { generation, .. } => generation,
        _ => unreachable!(),
    };
    assert!(state.settings_loaded(
        profile,
        generation,
        Ok(ReminderSettings {
            end_of_day: None,
            default_tone: tone,
            week_start: WeekStartPreference::System,
        })
    ));
    (state, profile, generation)
}

fn notification(effect: &Effect) -> (u64, &NotificationOperation) {
    match effect {
        Effect::Notification { id, operation } => (*id, operation),
        _ => panic!("expected notification effect"),
    }
}

fn startup_request(effects: Vec<Effect>) -> ReminderStartupRequest {
    match effects.as_slice() {
        [Effect::SaveStartupConsent(request)] => *request,
        _ => panic!("expected one startup consent effect"),
    }
}

#[test]
fn unset_end_of_day_stays_unset_until_the_person_sets_it() {
    let (mut state, profile, generation) = ready(ToneId::System);
    assert!(matches!(
        state.settings,
        SettingsState::Ready(ReminderSettings {
            end_of_day: None,
            ..
        })
    ));

    let at = LocalTime::new(17, 30, 0).unwrap();
    assert_eq!(
        state.set_end_of_day(Some(at)),
        vec![Effect::PatchSettings {
            profile,
            generation,
            patch: SettingsPatch {
                end_of_day: Some(Some(at)),
                default_tone: None,
                week_start: None,
            },
        }]
    );
}

#[test]
fn week_start_is_an_independent_optimistic_profile_setting() {
    let (mut state, profile, generation) = ready(ToneId::System);
    assert_eq!(
        state.set_week_start(WeekStartPreference::Sunday),
        vec![Effect::PatchSettings {
            profile,
            generation,
            patch: SettingsPatch {
                end_of_day: None,
                default_tone: None,
                week_start: Some(WeekStartPreference::Sunday),
            },
        }]
    );
    assert!(matches!(
        state.settings,
        SettingsState::Saving(ReminderSettings {
            week_start: WeekStartPreference::Sunday,
            ..
        })
    ));
}

#[test]
fn rapid_profile_setting_changes_coalesce_to_the_latest_desired_value() {
    let (mut state, profile, generation) = ready(ToneId::System);
    let end_of_day = LocalTime::new(17, 31, 0).unwrap();
    assert_eq!(state.set_end_of_day(Some(end_of_day)).len(), 1);
    assert!(state.set_default_tone(ToneId::Calm).is_empty());
    assert!(state.set_default_tone(ToneId::Bright).is_empty());
    assert_eq!(
        state.settings,
        SettingsState::Saving(ReminderSettings {
            end_of_day: Some(end_of_day),
            default_tone: ToneId::Bright,
            week_start: WeekStartPreference::System,
        })
    );

    let (applied, follow_up) = state.settings_saved_with_follow_up(
        profile,
        generation,
        Ok(ReminderSettings {
            end_of_day: Some(end_of_day),
            default_tone: ToneId::System,
            week_start: WeekStartPreference::System,
        }),
    );
    assert!(applied);
    assert_eq!(
        follow_up,
        vec![Effect::PatchSettings {
            profile,
            generation,
            patch: SettingsPatch {
                end_of_day: None,
                default_tone: Some(ToneId::Bright),
                week_start: None,
            },
        }]
    );
    assert!(state.settings_saved(
        profile,
        generation,
        Ok(ReminderSettings {
            end_of_day: Some(end_of_day),
            default_tone: ToneId::Bright,
            week_start: WeekStartPreference::System,
        })
    ));
    assert!(matches!(
        state.settings,
        SettingsState::Ready(ReminderSettings {
            end_of_day: Some(value),
            default_tone: ToneId::Bright,
            week_start: WeekStartPreference::System,
        }) if value == end_of_day
    ));
}

#[test]
fn remote_refresh_preserves_local_fields_still_being_saved() {
    let (mut state, profile, generation) = ready(ToneId::System);
    let local_end_of_day = LocalTime::new(18, 15, 0).unwrap();
    assert_eq!(state.set_end_of_day(Some(local_end_of_day)).len(), 1);

    let (applied, follow_up) = state.settings_loaded_with_follow_up(
        profile,
        generation,
        Ok(ReminderSettings {
            end_of_day: None,
            default_tone: ToneId::System,
            week_start: WeekStartPreference::Monday,
        }),
    );

    assert!(applied);
    assert!(follow_up.is_empty());
    assert_eq!(
        state.settings,
        SettingsState::Saving(ReminderSettings {
            end_of_day: Some(local_end_of_day),
            default_tone: ToneId::System,
            week_start: WeekStartPreference::Monday,
        })
    );
}

#[test]
fn failed_setting_load_and_save_can_reload_and_reapply_the_desired_value() {
    let profile = profile();
    let mut loading = ReminderPreferences::default();
    let generation = match loading.select_profile(profile)[0] {
        Effect::LoadSettings { generation, .. } => generation,
        _ => unreachable!(),
    };
    assert!(loading.settings_loaded(profile, generation, Err(())));
    assert_eq!(loading.retry_settings().len(), 1);
    assert!(loading.settings_loaded(
        profile,
        generation,
        Ok(ReminderSettings {
            end_of_day: None,
            default_tone: ToneId::System,
            week_start: WeekStartPreference::System,
        })
    ));
    assert!(matches!(loading.settings, SettingsState::Ready(_)));

    let (mut saving, profile, generation) = ready(ToneId::System);
    saving.set_default_tone(ToneId::Calm);
    assert!(saving.settings_saved(profile, generation, Err(())));
    assert_eq!(saving.settings, SettingsState::Failed);
    assert!(matches!(
        saving.retry_settings().as_slice(),
        [Effect::LoadSettings { .. }]
    ));
    let (applied, follow_up) = saving.settings_loaded_with_follow_up(
        profile,
        generation,
        Ok(ReminderSettings {
            end_of_day: LocalTime::new(9, 5, 0),
            default_tone: ToneId::System,
            week_start: WeekStartPreference::System,
        }),
    );
    assert!(applied);
    assert_eq!(
        follow_up,
        vec![Effect::PatchSettings {
            profile,
            generation,
            patch: SettingsPatch {
                end_of_day: None,
                default_tone: Some(ToneId::Calm),
                week_start: None,
            },
        }]
    );
    assert!(matches!(
        saving.settings,
        SettingsState::Saving(ReminderSettings {
            end_of_day: Some(value),
            default_tone: ToneId::Calm,
            week_start: WeekStartPreference::System,
        }) if value == LocalTime::new(9, 5, 0).unwrap()
    ));
}

#[test]
fn declining_startup_is_durable_warning_and_never_mutates_reminder_state() {
    let (mut state, _, _) = ready(ToneId::System);
    state.observe_reminders(
        true,
        StartupFacts {
            start_on_login: false,
            reminder_explained: false,
        },
    );
    assert_eq!(state.startup, StartupState::NeedsExplanation);
    let request = startup_request(state.answer_startup(false));
    assert!(!request.accepted);
    assert!(state.startup_saved(
        request.id,
        Ok(StartupFacts {
            start_on_login: false,
            reminder_explained: true,
        })
    ));
    assert_eq!(state.startup, StartupState::Declined);
    assert!(
        state
            .warnings()
            .any(|warning| warning == DeliveryWarning::StartOnLoginDeclined)
    );
}

#[test]
fn accepting_startup_waits_for_shell_success() {
    let (mut state, _, _) = ready(ToneId::System);
    state.observe_reminders(true, StartupFacts::default());
    let request = startup_request(state.answer_startup(true));
    assert!(request.accepted);
    assert_eq!(
        state.startup,
        StartupState::Saving {
            request_id: request.id,
            accepted: true
        }
    );
    assert!(state.startup_saved(
        request.id,
        Ok(StartupFacts {
            start_on_login: true,
            reminder_explained: true,
        })
    ));
    assert_eq!(state.startup, StartupState::NotNeeded);
    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::StartOnLoginDeclined)
    );
}

#[test]
fn failed_startup_persistence_keeps_the_shells_actual_state() {
    let (mut state, _, _) = ready(ToneId::System);
    state.observe_reminders(true, StartupFacts::default());
    let request = startup_request(state.answer_startup(true));
    let actual = StartupFacts {
        start_on_login: true,
        reminder_explained: false,
    };
    assert!(state.startup_saved(request.id, Err(actual)));
    assert_eq!(state.startup, StartupState::Failed { accepted: true });
    assert_eq!(state.startup_facts, actual);
    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::StartOnLoginDeclined)
    );
}

#[test]
fn either_startup_answer_can_replace_a_failed_answer() {
    for first in [false, true] {
        let (mut state, _, _) = ready(ToneId::System);
        state.observe_reminders(true, StartupFacts::default());
        let failed = startup_request(state.answer_startup(first));
        assert!(state.startup_saved(failed.id, Err(StartupFacts::default())));

        let replacement = startup_request(state.answer_startup(!first));
        assert_eq!(replacement.accepted, !first);
        assert_eq!(
            state.startup,
            StartupState::Saving {
                request_id: replacement.id,
                accepted: !first,
            }
        );
    }
}

#[test]
fn reconciliation_during_startup_save_does_not_erase_the_correlation() {
    let (mut state, _, _) = ready(ToneId::System);
    state.observe_reminders(true, StartupFacts::default());
    let request = startup_request(state.answer_startup(true));
    state.observe_reminders(
        true,
        StartupFacts {
            start_on_login: false,
            reminder_explained: true,
        },
    );
    assert_eq!(
        state.startup,
        StartupState::Saving {
            request_id: request.id,
            accepted: true
        }
    );
}

#[test]
fn opposite_answers_apply_only_to_the_window_that_made_each_request() {
    let mut declining = Controller::default();
    let mut accepting = Controller::default();
    assert!(declining.observe_reminders(true));
    assert!(accepting.observe_reminders(true));
    declining.answer_startup(false);
    accepting.answer_startup(true);
    declining.maintain(&AgentLink::absent());
    accepting.maintain(&AgentLink::absent());
    let decline = declining.take_startup_request().unwrap();
    let accept = accepting.take_startup_request().unwrap();

    let declined = ReminderStartupResult {
        id: decline.id,
        actual: koru_window::ReminderStartupFacts {
            start_on_login: false,
            explained: true,
        },
        error: None,
    };
    assert!(declining.finish_startup(declined.clone()));
    assert!(!accepting.finish_startup(declined));
    assert_eq!(declining.model.startup, StartupState::Declined);
    assert_eq!(accepting.startup_facts, StartupFacts::default());
    assert_eq!(
        accepting.model.startup,
        StartupState::Saving {
            request_id: accept.id,
            accepted: true,
        }
    );

    let accepted = ReminderStartupResult {
        id: accept.id,
        actual: koru_window::ReminderStartupFacts {
            start_on_login: true,
            explained: true,
        },
        error: None,
    };
    assert!(!declining.finish_startup(accepted.clone()));
    assert!(accepting.finish_startup(accepted));
    assert_eq!(accepting.model.startup, StartupState::NotNeeded);
}

#[test]
fn superseded_opposite_answer_converges_to_actual_facts_without_failure() {
    let mut first_decline = Controller::default();
    let mut superseded_accept = Controller::default();
    let mut latest_decline = Controller::default();
    for controller in [
        &mut first_decline,
        &mut superseded_accept,
        &mut latest_decline,
    ] {
        assert!(controller.observe_reminders(true));
    }
    first_decline.answer_startup(false);
    superseded_accept.answer_startup(true);
    latest_decline.answer_startup(false);
    for controller in [
        &mut first_decline,
        &mut superseded_accept,
        &mut latest_decline,
    ] {
        controller.maintain(&AgentLink::absent());
    }
    let first = first_decline.take_startup_request().unwrap();
    let superseded = superseded_accept.take_startup_request().unwrap();
    let latest = latest_decline.take_startup_request().unwrap();

    // A later decline matched the already-running decline in another window,
    // so this accept needed no separate OS write. Its tagged completion still
    // carries the winner's authoritative facts and is a successful outcome.
    let actual = koru_window::ReminderStartupFacts {
        start_on_login: false,
        explained: true,
    };
    assert!(first_decline.finish_startup(ReminderStartupResult {
        id: first.id,
        actual,
        error: None,
    }));
    assert!(superseded_accept.finish_startup(ReminderStartupResult {
        id: superseded.id,
        actual,
        error: None,
    }));
    assert!(latest_decline.finish_startup(ReminderStartupResult {
        id: latest.id,
        actual,
        error: None,
    }));
    assert_eq!(first_decline.model.startup, StartupState::Declined);
    assert_eq!(superseded_accept.model.startup, StartupState::Declined);
    assert_eq!(latest_decline.model.startup, StartupState::Declined);
}

#[test]
fn durable_create_and_remote_presence_each_require_the_first_explanation() {
    for _source in ["durable create reconciliation", "remote reconciliation"] {
        let mut controller = Controller::default();
        assert!(controller.observe_reminders(true));
        assert_eq!(controller.model.startup, StartupState::NeedsExplanation);
    }
}

#[test]
fn tone_warning_tracks_loading_and_each_patch_direction() {
    let (mut state, profile, generation) = ready(ToneId::System);
    state.custom_sound = false;
    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::CustomToneUnavailable)
    );
    state.set_default_tone(ToneId::Calm);
    assert!(
        state
            .warnings()
            .any(|warning| warning == DeliveryWarning::CustomToneUnavailable)
    );
    state.settings_saved(
        profile,
        generation,
        Ok(ReminderSettings {
            end_of_day: None,
            default_tone: ToneId::Calm,
            week_start: WeekStartPreference::System,
        }),
    );
    state.set_default_tone(ToneId::System);
    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::CustomToneUnavailable)
    );
}

#[test]
fn not_determined_permission_is_not_reported_as_denied() {
    let (mut state, _, _) = ready(ToneId::System);
    state.set_agent_connected(true);
    let inspect = state.refresh_notification_status();
    let (id, _) = notification(&inspect[0]);
    state.notification_result(
        id,
        NotificationControlResult::Capability {
            authorization: NotificationAuthorization::NotDetermined,
            actions: true,
            stable_replacement: true,
            withdrawal: true,
            custom_sound: true,
            activation_while_stopped: true,
        },
        koru_i18n::Lang::En,
    );
    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::NotificationsNotAllowed)
    );
    assert_eq!(
        authorization_label(koru_i18n::Lang::En, state.authorization()),
        koru_i18n::t(
            koru_i18n::Lang::En,
            koru_i18n::Key::ReminderAuthorizationNotDetermined
        )
    );
}

#[test]
fn permission_changes_replace_one_warning_instead_of_repeating_it() {
    let (mut state, _, _) = ready(ToneId::System);
    state.set_agent_connected(true);
    let inspect = state.refresh_notification_status();
    let (inspect_id, _) = notification(&inspect[0]);
    state.notification_result(
        inspect_id,
        NotificationControlResult::Capability {
            authorization: NotificationAuthorization::Denied,
            actions: true,
            stable_replacement: true,
            withdrawal: true,
            custom_sound: true,
            activation_while_stopped: true,
        },
        koru_i18n::Lang::En,
    );
    let inspect = state.refresh_notification_status();
    let (inspect_id, _) = notification(&inspect[0]);
    state.notification_result(
        inspect_id,
        NotificationControlResult::Capability {
            authorization: NotificationAuthorization::Denied,
            actions: true,
            stable_replacement: true,
            withdrawal: true,
            custom_sound: true,
            activation_while_stopped: true,
        },
        koru_i18n::Lang::En,
    );
    assert_eq!(
        state
            .warnings()
            .filter(|warning| *warning == DeliveryWarning::NotificationsNotAllowed)
            .count(),
        1
    );
    let inspect = state.refresh_notification_status();
    let (inspect_id, _) = notification(&inspect[0]);
    state.notification_result(
        inspect_id,
        NotificationControlResult::Capability {
            authorization: NotificationAuthorization::Allowed,
            actions: true,
            stable_replacement: true,
            withdrawal: true,
            custom_sound: true,
            activation_while_stopped: true,
        },
        koru_i18n::Lang::En,
    );
    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::NotificationsNotAllowed)
    );
}

#[test]
fn disconnect_after_success_restores_the_agent_warning() {
    let (mut state, _, _) = ready(ToneId::System);
    let inspect = state.refresh_notification_status();
    let (inspect_id, _) = notification(&inspect[0]);
    let success = NotificationControlResult::Capability {
        authorization: NotificationAuthorization::Allowed,
        actions: true,
        stable_replacement: true,
        withdrawal: true,
        custom_sound: true,
        activation_while_stopped: true,
    };
    state.set_agent_connected(notification_agent_connected(&success));
    state.notification_result(inspect_id, success, koru_i18n::Lang::En);
    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::AgentUnavailable)
    );

    let inspect = state.refresh_notification_status();
    let (inspect_id, _) = notification(&inspect[0]);
    let disconnected = NotificationControlResult::Failed {
        code: NotificationFailure::Unavailable,
    };
    state.set_agent_connected(notification_agent_connected(&disconnected));
    state.notification_result(inspect_id, disconnected, koru_i18n::Lang::En);
    assert!(
        state
            .warnings()
            .any(|warning| warning == DeliveryWarning::AgentUnavailable)
    );
}

#[test]
fn notification_worker_timeout_keeps_the_agent_connected() {
    let (mut state, _, _) = ready(ToneId::System);
    let inspect = state.refresh_notification_status();
    let (inspect_id, _) = notification(&inspect[0]);
    let timeout = NotificationControlResult::Failed {
        code: NotificationFailure::Timeout,
    };
    state.set_agent_connected(notification_agent_connected(&timeout));
    state.notification_result(inspect_id, timeout, koru_i18n::Lang::En);

    assert!(
        !state
            .warnings()
            .any(|warning| warning == DeliveryWarning::AgentUnavailable)
    );
    assert!(
        state
            .warnings()
            .any(|warning| warning == DeliveryWarning::NativeDeliveryUnavailable)
    );
}

#[test]
fn test_notification_uses_agent_only_and_never_writes_a_reminder() {
    let (mut state, _, _) = ready(ToneId::System);
    let effects = state.send_test(koru_i18n::Lang::En);
    assert_eq!(effects.len(), 1);
    assert!(matches!(
        &effects[0],
        Effect::Notification {
            operation: NotificationOperation::SendTest {
                tone: NotificationTone::System,
                ..
            },
            ..
        }
    ));
    assert!(!effects.iter().any(|effect| matches!(
        effect,
        Effect::LoadSettings { .. } | Effect::PatchSettings { .. }
    )));
}

#[test]
fn missing_custom_asset_falls_back_once_without_changing_the_saved_tone() {
    let (mut state, _, _) = ready(ToneId::Calm);
    state.set_agent_connected(true);
    let inspect = state.refresh_notification_status();
    let (inspect_id, _) = notification(&inspect[0]);
    state.notification_result(
        inspect_id,
        NotificationControlResult::Capability {
            authorization: NotificationAuthorization::Allowed,
            actions: true,
            stable_replacement: true,
            withdrawal: true,
            custom_sound: true,
            activation_while_stopped: true,
        },
        koru_i18n::Lang::En,
    );
    let first = state.send_test(koru_i18n::Lang::En);
    assert!(matches!(
        &first[0],
        Effect::Notification {
            operation: NotificationOperation::SendTest {
                tone: NotificationTone::Calm,
                ..
            },
            ..
        }
    ));
    let (test_id, _) = notification(&first[0]);
    let fallback = state.notification_result(
        test_id,
        NotificationControlResult::Failed {
            code: NotificationFailure::AssetMissing,
        },
        koru_i18n::Lang::En,
    );
    assert!(matches!(
        &fallback[0],
        Effect::Notification {
            operation: NotificationOperation::SendTest {
                tone: NotificationTone::System,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        state.settings,
        SettingsState::Ready(ReminderSettings {
            default_tone: ToneId::Calm,
            ..
        })
    ));
    assert!(
        state
            .warnings()
            .any(|warning| warning == DeliveryWarning::CustomToneUnavailable)
    );

    let (fallback_id, _) = notification(&fallback[0]);
    assert!(matches!(
        state
            .notification_result(
                fallback_id,
                NotificationControlResult::Failed {
                    code: NotificationFailure::AssetMissing,
                },
                koru_i18n::Lang::En,
            )
            .as_slice(),
        [Effect::ReminderStatus { .. }]
    ));
}

#[test]
fn only_the_explicit_allow_intent_requests_authorization() {
    let (mut state, _, _) = ready(ToneId::System);
    let allow = state.allow_notifications();
    assert!(matches!(
        notification(&allow[0]).1,
        NotificationOperation::RequestAuthorization
    ));
    for effects in [
        state.refresh_notification_status(),
        state.send_test(koru_i18n::Lang::En),
    ] {
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::Notification {
                operation: NotificationOperation::RequestAuthorization,
                ..
            }
        )));
    }
}

#[test]
fn delivery_toggle_and_repair_are_agent_only_current_device_effects() {
    let (mut state, _, _) = ready(ToneId::System);
    let disabled = state.set_delivery_enabled(false);
    assert!(matches!(
        disabled.as_slice(),
        [Effect::Notification {
            operation: NotificationOperation::SetDeliveryEnabled { enabled: false },
            ..
        }]
    ));
    assert!(!disabled.iter().any(|effect| matches!(
        effect,
        Effect::LoadSettings { .. } | Effect::PatchSettings { .. }
    )));
    assert!(matches!(
        state.repair_delivery().as_slice(),
        [Effect::Notification {
            operation: NotificationOperation::Repair,
            ..
        }]
    ));
}

#[test]
fn rapid_delivery_reversal_serializes_latest_intent_and_ignores_stale_health() {
    let (mut state, _, _) = ready(ToneId::System);
    assert!(state.health_result(
        1,
        Ok(ReminderDiagnostics {
            enabled: false,
            ..ReminderDiagnostics::default()
        })
    ));

    let first = state.set_delivery_enabled(true);
    let (first_id, _) = notification(&first[0]);
    assert!(state.set_delivery_enabled(false).is_empty());
    assert_eq!(state.desired_delivery_enabled(), Some(false));

    let second = state.notification_result(
        first_id,
        NotificationControlResult::Done,
        koru_i18n::Lang::En,
    );
    let (second_id, second_operation) = notification(&second[0]);
    assert_eq!(
        second_operation,
        &NotificationOperation::SetDeliveryEnabled { enabled: false }
    );
    assert!(
        state
            .notification_result(
                first_id,
                NotificationControlResult::Done,
                koru_i18n::Lang::En,
            )
            .is_empty()
    );
    assert_eq!(state.desired_delivery_enabled(), Some(false));
    let health = state.notification_result(
        second_id,
        NotificationControlResult::Done,
        koru_i18n::Lang::En,
    );
    let health_id = match health.as_slice() {
        [Effect::ReminderStatus { id }] => *id,
        other => panic!("expected final correlated health request, got {other:?}"),
    };
    assert!(!state.health_result(
        health_id.wrapping_sub(1),
        Ok(ReminderDiagnostics {
            enabled: true,
            ..ReminderDiagnostics::default()
        })
    ));
    assert_eq!(state.desired_delivery_enabled(), Some(false));
    assert!(state.health_result(
        health_id,
        Ok(ReminderDiagnostics {
            enabled: false,
            ..ReminderDiagnostics::default()
        })
    ));
    assert_eq!(state.desired_delivery_enabled(), None);
}

#[test]
fn health_is_plain_content_free_agent_data_and_permission_does_not_change_choice() {
    let (mut state, _, _) = ready(ToneId::System);
    let health = ReminderDiagnostics {
        enabled: false,
        pending: 0,
        registered: 0,
        failed: 1,
        cursor: Some(17),
        next_deadline_ms: None,
        last_successful_sync_ms: Some(1_725_000_000_000),
        attention: Vec::new(),
        failure_category: Some("native_permission".into()),
    };
    assert!(state.health_result(1, Ok(health.clone())));
    assert_eq!(state.health, HealthState::Ready(health.clone()));

    let allow = state.allow_notifications();
    let (id, _) = notification(&allow[0]);
    state.notification_result(
        id,
        NotificationControlResult::Authorization {
            authorization: NotificationAuthorization::Allowed,
        },
        koru_i18n::Lang::En,
    );
    assert_eq!(state.health, HealthState::Ready(health));
}

#[test]
fn controller_applies_only_its_correlated_health_answer() {
    let mut controller = Controller::default();
    controller.model.latest_health = 1;
    controller.health_requests.insert(70, 1);
    let agent = AgentLink::absent();
    let expected = ReminderDiagnostics {
        enabled: true,
        pending: 2,
        registered: 2,
        failed: 0,
        cursor: Some(4),
        next_deadline_ms: Some(20),
        last_successful_sync_ms: Some(10),
        attention: Vec::new(),
        failure_category: None,
    };
    agent.answer_reminder_status_for_test(71, Ok(ReminderDiagnostics::default()));
    agent.answer_reminder_status_for_test(70, Ok(expected.clone()));
    assert!(controller.maintain(&agent));
    assert_eq!(controller.model.health, HealthState::Ready(expected));
    assert!(agent.take_reminder_status_for(71).is_some());
}

#[test]
fn pushed_health_supersedes_an_older_answer_queued_before_maintenance() {
    let mut controller = Controller::default();
    controller.model.latest_health = 1;
    controller.model.next_health = 2;
    controller.health_requests.insert(70, 1);
    let agent = AgentLink::absent();
    let stale = ReminderDiagnostics {
        enabled: false,
        ..ReminderDiagnostics::default()
    };
    let pushed = ReminderDiagnostics {
        enabled: true,
        pending: 3,
        registered: 2,
        failed: 1,
        ..ReminderDiagnostics::default()
    };

    agent.answer_reminder_status_for_test(70, Ok(stale));
    assert!(controller.set_pushed_diagnostics(pushed.clone()));
    controller.maintain(&agent);

    assert_eq!(controller.model.health, HealthState::Ready(pushed));
    assert!(controller.health_requests.is_empty());
}

#[test]
fn health_presentation_covers_denied_disabled_failure_and_recovery() {
    let mut controller = Controller::default();
    controller.model.agent_connected = true;
    controller.model.delivery_supported = true;
    controller.model.authorization = NotificationAuthorization::Denied;
    controller.model.health = HealthState::Ready(ReminderDiagnostics {
        enabled: true,
        ..ReminderDiagnostics::default()
    });
    controller.model.refresh_warnings();
    let denied = controller.health_presentation(koru_i18n::Lang::En);
    assert!(denied.banner.is_some());
    assert_eq!(denied.action, Some(HealthAction::OpenNotificationSettings));

    controller.model.authorization = NotificationAuthorization::Allowed;
    controller.model.health = HealthState::Ready(ReminderDiagnostics {
        enabled: false,
        ..ReminderDiagnostics::default()
    });
    controller.model.refresh_warnings();
    let disabled = controller.health_presentation(koru_i18n::Lang::En);
    assert_eq!(disabled.action, Some(HealthAction::EnableDelivery));

    controller.model.health = HealthState::Ready(ReminderDiagnostics {
        enabled: true,
        failure_category: Some("ledger".into()),
        ..ReminderDiagnostics::default()
    });
    let failed = controller.health_presentation(koru_i18n::Lang::En);
    assert_eq!(failed.action, Some(HealthAction::Repair));
    assert_eq!(
        failed.banner.as_deref(),
        Some(koru_i18n::t(
            koru_i18n::Lang::En,
            koru_i18n::Key::ReminderWarningDeliveryUnavailable
        ))
    );

    controller.model.health = HealthState::Ready(ReminderDiagnostics {
        enabled: true,
        ..ReminderDiagnostics::default()
    });
    controller.model.refresh_warnings();
    let recovered = controller.health_presentation(koru_i18n::Lang::En);
    assert!(recovered.banner.is_none());
    assert!(recovered.action.is_none());
    assert_eq!(
        recovered.footer,
        koru_i18n::t(koru_i18n::Lang::En, koru_i18n::Key::ReminderUpToDate)
    );

    controller.model.set_agent_connected(false);
    assert_eq!(
        controller.health_presentation(koru_i18n::Lang::En).action,
        Some(HealthAction::Refresh)
    );
}

#[test]
fn stale_health_answer_cannot_overwrite_a_newer_snapshot() {
    let (mut state, _, _) = ready(ToneId::System);
    let Effect::ReminderStatus { id: older } = state.health_effect() else {
        unreachable!();
    };
    let Effect::ReminderStatus { id: newer } = state.health_effect() else {
        unreachable!();
    };
    let current = ReminderDiagnostics {
        enabled: false,
        ..ReminderDiagnostics::default()
    };
    assert!(state.health_result(newer, Ok(current.clone())));
    assert!(!state.health_result(older, Ok(ReminderDiagnostics::default())));
    assert_eq!(state.health, HealthState::Ready(current));
}

#[test]
fn out_of_order_agent_results_keep_their_own_purpose() {
    let (mut state, _, _) = ready(ToneId::Calm);
    state.set_agent_connected(true);
    state.custom_sound = true;
    let inspect = state.refresh_notification_status();
    let test = state.send_test(koru_i18n::Lang::En);
    let (inspect_id, _) = notification(&inspect[0]);
    let (test_id, _) = notification(&test[0]);

    let fallback = state.notification_result(
        test_id,
        NotificationControlResult::Failed {
            code: NotificationFailure::AssetMissing,
        },
        koru_i18n::Lang::En,
    );
    assert_eq!(fallback.len(), 1);

    // The older Inspect answer must update capability rather than being
    // mistaken for the test that has already completed.
    assert!(
        state
            .notification_result(
                inspect_id,
                NotificationControlResult::Capability {
                    authorization: NotificationAuthorization::Allowed,
                    actions: true,
                    stable_replacement: true,
                    withdrawal: true,
                    custom_sound: false,
                    activation_while_stopped: true,
                },
                koru_i18n::Lang::En,
            )
            .is_empty()
    );
    assert_eq!(state.authorization(), NotificationAuthorization::Allowed);
}

#[test]
fn profile_switch_ignores_the_old_inspect_without_reusing_its_purpose() {
    let mut state = ReminderPreferences::default();
    let a = state.select_profile(profile());
    let (_, old_inspect) = (&a[0], &a[1]);
    let (old_id, _) = notification(old_inspect);
    state.select_profile(profile());

    assert!(
        state
            .notification_result(
                old_id,
                NotificationControlResult::Capability {
                    authorization: NotificationAuthorization::Denied,
                    actions: true,
                    stable_replacement: true,
                    withdrawal: true,
                    custom_sound: true,
                    activation_while_stopped: true,
                },
                koru_i18n::Lang::En,
            )
            .is_empty()
    );
    assert_eq!(
        state.authorization(),
        NotificationAuthorization::Unavailable,
        "the stale answer was consumed but did not overwrite the new profile state"
    );
}

#[test]
fn profile_switch_drains_an_in_flight_agent_answer_without_stale_overwrite() {
    let mut controller = Controller::default();
    let first = profile();
    controller.select_profile(first);
    let old_local_id = match controller.effects.get(1) {
        Some(Effect::Notification { id, .. }) => *id,
        _ => panic!("profile selection should inspect notification capability"),
    };
    controller.agent_requests.insert(71, old_local_id);

    controller.select_profile(profile());
    assert_eq!(controller.agent_requests.get(&71), Some(&old_local_id));

    let agent = crate::agent::AgentLink::absent();
    agent.answer_notification_control_for_test(
        71,
        NotificationControlResult::Capability {
            authorization: NotificationAuthorization::Denied,
            actions: true,
            stable_replacement: true,
            withdrawal: true,
            custom_sound: true,
            activation_while_stopped: true,
        },
    );
    controller.maintain(&agent);

    assert!(controller.agent_requests.is_empty());
    assert!(agent.take_notification_control_for(71).is_none());
    assert_eq!(
        controller.model.authorization(),
        NotificationAuthorization::Unavailable,
        "the old profile's answer was drained but did not overwrite the new model"
    );
}

#[test]
fn late_profile_load_cannot_trigger_first_reminder_state_in_the_new_profile() {
    let mut controller = Controller::default();
    let first = profile();
    controller.select_profile(first);
    let first_generation = match controller.effects.front() {
        Some(Effect::LoadSettings { generation, .. }) => *generation,
        _ => unreachable!(),
    };
    controller.select_profile(profile());
    controller
        .store_tx
        .send(StoreAnswer::Loaded {
            profile: first,
            generation: first_generation,
            result: Ok((ReminderSettings::default(), true)),
        })
        .unwrap();

    controller.maintain(&crate::agent::AgentLink::absent());
    assert_eq!(controller.model.startup, StartupState::Unknown);
}

#[test]
fn every_tone_has_a_nonempty_label_in_every_supported_language() {
    for lang in koru_i18n::Lang::ALL {
        for tone in TONES {
            assert!(
                !tone_label(lang, tone).is_empty(),
                "{} {tone:?}",
                lang.tag()
            );
        }
    }
}
