// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::ids::{DocumentId, RuleRevisionId, WorkspaceId};
use crate::model::Timestamp;
use crate::reminders::{
    ActionKind, DueRelation, EffectiveState, LocalTime, OccurrenceKey, OccurrenceMoment,
    OccurrenceSource, OneTimeSchedule, RuleStatus, TargetState, ToneId, WeekStartPreference,
};

fn profile() -> Profile {
    Profile::new(
        "Test",
        Locale::parse("en").unwrap(),
        Timestamp::from_millis(1),
    )
    .unwrap()
}

fn rule(id: ReminderId, revision: RuleRevisionId, hour: u8) -> ReminderRule {
    ReminderRule {
        id,
        target: EntityRef::new(
            "node",
            WorkspaceId::parse("11111111-1111-4111-8111-111111111111").unwrap(),
            DocumentId::parse("22222222-2222-4222-8222-222222222222").unwrap(),
            uuid::Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        )
        .unwrap(),
        revision,
        status: RuleStatus::Enabled,
        schedule: OneTimeSchedule::Floating {
            wall: crate::FloatingDateTime::parse(&format!("2026-09-02T{hour:02}:00:00")).unwrap(),
        },
        tone_override: None,
    }
}

fn action(rule: &ReminderRule, id: ActionId, kind: ActionKind) -> OccurrenceAction {
    let source = OccurrenceSource::OneTime {
        moment: match &rule.schedule {
            OneTimeSchedule::Floating { wall } => OccurrenceMoment::Floating { wall: wall.clone() },
            _ => unreachable!(),
        },
    };
    OccurrenceAction {
        id,
        occurrence: OccurrenceKey::derive(rule.id, rule.revision, &source),
        kind,
    }
}

fn replicas(
    base: &ProfileManifest,
    a_peer: u64,
    b_peer: u64,
) -> (ProfileManifest, ProfileManifest) {
    let snapshot = base.export_snapshot().unwrap();
    let a = ProfileManifest::from_snapshot(&snapshot).unwrap();
    let b = ProfileManifest::from_snapshot(&snapshot).unwrap();
    a.set_peer_id(a_peer).unwrap();
    b.set_peer_id(b_peer).unwrap();
    (a, b)
}

fn merge_since(
    a: &ProfileManifest,
    a_before: &loro::VersionVector,
    b: &ProfileManifest,
    b_before: &loro::VersionVector,
) {
    let a_frame = a.export_updates_since(a_before).unwrap();
    let b_frame = b.export_updates_since(b_before).unwrap();
    a.import_frames(&[b_frame]).unwrap();
    b.import_frames(&[a_frame]).unwrap();
}

#[test]
fn concurrent_different_reminder_creates_both_survive() {
    let base = ProfileManifest::create(&profile()).unwrap();
    let (a, b) = replicas(&base, 11, 12);
    let av = a.version();
    let bv = b.version();
    let a_rule = rule(ReminderId::new(), RuleRevisionId::new(), 9);
    let b_rule = rule(ReminderId::new(), RuleRevisionId::new(), 10);
    a.put_reminder_rule(&a_rule).unwrap();
    b.put_reminder_rule(&b_rule).unwrap();
    merge_since(&a, &av, &b, &bv);

    assert_eq!(a.reminders(), b.reminders());
    assert_eq!(a.reminders().len(), 2);
    assert!(a.reminder(a_rule.id).is_some());
    assert!(a.reminder(b_rule.id).is_some());
}

#[test]
fn pause_and_resume_change_only_the_addressed_reminder_on_a_shared_target() {
    let manifest = ProfileManifest::create(&profile()).unwrap();
    let first = rule(ReminderId::new(), RuleRevisionId::new(), 9);
    let mut second = rule(ReminderId::new(), RuleRevisionId::new(), 10);
    assert_eq!(first.target, second.target);
    manifest.put_reminder_rule(&first).unwrap();
    manifest.put_reminder_rule(&second).unwrap();

    second.status = RuleStatus::Paused;
    second.revision = RuleRevisionId::new();
    manifest.put_reminder_rule(&second).unwrap();
    assert_eq!(
        manifest.reminder(first.id).unwrap().rule.status,
        RuleStatus::Enabled
    );
    assert_eq!(
        manifest.reminder(second.id).unwrap().rule.status,
        RuleStatus::Paused
    );

    second.status = RuleStatus::Enabled;
    second.revision = RuleRevisionId::new();
    manifest.put_reminder_rule(&second).unwrap();
    assert_eq!(
        manifest.reminder(first.id).unwrap().rule.status,
        RuleStatus::Enabled
    );
    assert_eq!(
        manifest.reminder(second.id).unwrap().rule.status,
        RuleStatus::Enabled
    );
}

#[test]
fn concurrent_whole_rule_edits_choose_one_authored_rule_without_a_hybrid() {
    let id = ReminderId::new();
    let base = ProfileManifest::create(&profile()).unwrap();
    base.put_reminder_rule(&rule(id, RuleRevisionId::new(), 8))
        .unwrap();
    let (a, b) = replicas(&base, 21, 22);
    let av = a.version();
    let bv = b.version();
    let mut from_a = rule(id, RuleRevisionId::new(), 9);
    from_a.status = RuleStatus::Paused;
    from_a.tone_override = Some(ToneId::Chime);
    let mut from_b = rule(id, RuleRevisionId::new(), 17);
    from_b.tone_override = Some(ToneId::System);
    a.put_reminder_rule(&from_a).unwrap();
    b.put_reminder_rule(&from_b).unwrap();
    merge_since(&a, &av, &b, &bv);

    let merged = a.reminder(id).unwrap().rule;
    assert_eq!(merged, b.reminder(id).unwrap().rule);
    assert!(merged == from_a || merged == from_b);
}

#[test]
fn independent_action_appends_union_and_retries_are_idempotent() {
    let id = ReminderId::new();
    let stored_rule = rule(id, RuleRevisionId::new(), 9);
    let base = ProfileManifest::create(&profile()).unwrap();
    base.put_reminder_rule(&stored_rule).unwrap();
    let (a, b) = replicas(&base, 31, 32);
    let av = a.version();
    let bv = b.version();
    let from_a = action(
        &stored_rule,
        ActionId::new(),
        ActionKind::SnoozedUntil { instant_ms: 10 },
    );
    let from_b = action(&stored_rule, ActionId::new(), ActionKind::Handled);
    a.append_reminder_action(id, &from_a).unwrap();
    a.append_reminder_action(id, &from_a).unwrap();
    b.append_reminder_action(id, &from_b).unwrap();
    merge_since(&a, &av, &b, &bv);

    let actions = a.reminder(id).unwrap().actions;
    assert_eq!(actions, b.reminder(id).unwrap().actions);
    assert_eq!(actions.len(), 2);
    assert!(actions.contains(&from_a));
    assert!(actions.contains(&from_b));
}

#[test]
fn independent_settings_keys_converge_without_erasing_each_other() {
    let base = ProfileManifest::create(&profile()).unwrap();
    let (a, b) = replicas(&base, 41, 42);
    let av = a.version();
    let bv = b.version();
    a.update_reminder_settings(Some(Some(LocalTime::new(18, 0, 0).unwrap())), None, None)
        .unwrap();
    b.update_reminder_settings(None, Some(ToneId::Chime), Some(WeekStartPreference::Sunday))
        .unwrap();
    merge_since(&a, &av, &b, &bv);

    assert_eq!(a.reminder_settings(), b.reminder_settings());
    assert_eq!(
        a.reminder_settings(),
        ReminderSettings {
            end_of_day: LocalTime::new(18, 0, 0),
            default_tone: ToneId::Chime,
            week_start: WeekStartPreference::Sunday,
        }
    );
}

#[test]
fn settings_retries_and_clear_absent_are_true_no_ops() {
    let manifest = ProfileManifest::create(&profile()).unwrap();
    let before = manifest.version();
    assert!(
        !manifest
            .update_reminder_settings(Some(None), Some(ToneId::System), None)
            .unwrap()
    );
    assert_eq!(manifest.version(), before);

    assert!(
        manifest
            .update_reminder_settings(Some(Some(LocalTime::new(17, 30, 0).unwrap())), None, None,)
            .unwrap()
    );
    let after_write = manifest.version();
    assert!(
        !manifest
            .update_reminder_settings(
                Some(Some(LocalTime::new(17, 30, 0).unwrap())),
                Some(ToneId::System),
                None,
            )
            .unwrap()
    );
    assert_eq!(manifest.version(), after_write);
}

#[test]
fn delete_wins_over_a_concurrent_whole_rule_edit() {
    let id = ReminderId::new();
    let original = rule(id, RuleRevisionId::new(), 9);
    let base = ProfileManifest::create(&profile()).unwrap();
    base.put_reminder_rule(&original).unwrap();
    let (a, b) = replicas(&base, 51, 52);
    let av = a.version();
    let bv = b.version();
    a.delete_reminder(id).unwrap();
    b.put_reminder_rule(&rule(id, RuleRevisionId::new(), 10))
        .unwrap();
    merge_since(&a, &av, &b, &bv);

    assert!(a.reminder(id).unwrap().deleted);
    assert_eq!(a.reminder(id), b.reminder(id));
}

#[test]
fn authoritative_target_cleanup_tombstones_only_matching_live_rules() {
    let matching = rule(ReminderId::new(), RuleRevisionId::new(), 9);
    let already_deleted = rule(ReminderId::new(), RuleRevisionId::new(), 10);
    let mut other = rule(ReminderId::new(), RuleRevisionId::new(), 11);
    other.target = EntityRef::new(
        "node",
        matching.target.workspace(),
        matching.target.document(),
        uuid::Uuid::new_v4(),
    )
    .unwrap();
    let manifest = ProfileManifest::create(&profile()).unwrap();
    manifest.put_reminder_rule(&matching).unwrap();
    manifest.put_reminder_rule(&already_deleted).unwrap();
    manifest.delete_reminder(already_deleted.id).unwrap();
    manifest.put_reminder_rule(&other).unwrap();

    assert_eq!(
        manifest
            .delete_reminders_for_target(&matching.target)
            .unwrap(),
        vec![matching.id]
    );
    assert!(manifest.reminder(matching.id).unwrap().deleted);
    assert!(manifest.reminder(already_deleted.id).unwrap().deleted);
    assert!(!manifest.reminder(other.id).unwrap().deleted);
    assert!(
        manifest
            .delete_reminders_for_target(&matching.target)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn clear_all_is_monotonic_against_an_offline_retry_and_preserves_snoozes() {
    let id = ReminderId::new();
    let stored_rule = rule(id, RuleRevisionId::new(), 9);
    let handled = action(&stored_rule, ActionId::new(), ActionKind::Handled);
    let snoozed = action(
        &stored_rule,
        ActionId::new(),
        ActionKind::SnoozedUntil { instant_ms: 10 },
    );
    let base = ProfileManifest::create(&profile()).unwrap();
    base.put_reminder_rule(&stored_rule).unwrap();
    base.append_reminder_action(id, &handled).unwrap();
    base.append_reminder_action(id, &snoozed).unwrap();
    let (a, b) = replicas(&base, 61, 62);
    let av = a.version();
    let bv = b.version();
    assert_eq!(a.clear_handled_reminder_history().unwrap(), 1);
    b.append_reminder_action(id, &handled).unwrap();
    merge_since(&a, &av, &b, &bv);

    assert_eq!(a.reminder(id).unwrap().actions, vec![snoozed.clone()]);
    assert_eq!(a.reminder(id), b.reminder(id));
    assert_eq!(a.raw_reminder_action(id, handled.id), Some(handled));
    let input = a
        .reminder_reduction_inputs()
        .into_iter()
        .find(|input| input.stored().rule.id == id)
        .unwrap();
    let occurrence = snoozed.occurrence;
    let reduced = input.reduce(TargetState::Active, None);
    assert_eq!(reduced.effective, EffectiveState::Handled { occurrence });
    assert!(reduced.handled_history.is_empty());
    assert_eq!(a.clear_handled_reminder_history().unwrap(), 0);
}

#[test]
fn clear_history_leaves_active_waiting_and_snoozed_reminders_untouched() {
    let active = rule(ReminderId::new(), RuleRevisionId::new(), 9);
    let mut waiting = rule(ReminderId::new(), RuleRevisionId::new(), 10);
    waiting.schedule = OneTimeSchedule::RelativeToDue {
        relation: DueRelation::At,
        offset: None,
    };
    let snoozed_rule = rule(ReminderId::new(), RuleRevisionId::new(), 11);
    let snoozed = action(
        &snoozed_rule,
        ActionId::new(),
        ActionKind::SnoozedUntil { instant_ms: 50 },
    );
    let handled_rule = rule(ReminderId::new(), RuleRevisionId::new(), 12);
    let handled = action(&handled_rule, ActionId::new(), ActionKind::Handled);
    let manifest = ProfileManifest::create(&profile()).unwrap();
    for value in [&active, &waiting, &snoozed_rule, &handled_rule] {
        manifest.put_reminder_rule(value).unwrap();
    }
    manifest
        .append_reminder_action(snoozed_rule.id, &snoozed)
        .unwrap();
    manifest
        .append_reminder_action(handled_rule.id, &handled)
        .unwrap();
    let before = [active.id, waiting.id, snoozed_rule.id].map(|id| manifest.reminder(id).unwrap());

    assert_eq!(manifest.clear_handled_reminder_history().unwrap(), 1);

    let after = [active.id, waiting.id, snoozed_rule.id].map(|id| manifest.reminder(id).unwrap());
    assert_eq!(after, before);
    assert_eq!(after[2].actions, vec![snoozed]);
    assert!(
        manifest
            .reminder(handled_rule.id)
            .unwrap()
            .actions
            .is_empty()
    );
}

#[test]
fn target_scoped_clear_hides_only_that_objects_handled_history() {
    let first = rule(ReminderId::new(), RuleRevisionId::new(), 9);
    let mut second = rule(ReminderId::new(), RuleRevisionId::new(), 10);
    second.target = EntityRef::new(
        "node",
        second.target.workspace(),
        second.target.document(),
        uuid::Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
    )
    .unwrap();
    let first_action = action(&first, ActionId::new(), ActionKind::Handled);
    let second_action = action(&second, ActionId::new(), ActionKind::Handled);
    let manifest = ProfileManifest::create(&profile()).unwrap();
    manifest.put_reminder_rule(&first).unwrap();
    manifest.put_reminder_rule(&second).unwrap();
    manifest
        .append_reminder_action(first.id, &first_action)
        .unwrap();
    manifest
        .append_reminder_action(second.id, &second_action)
        .unwrap();

    assert_eq!(
        manifest
            .clear_handled_reminder_history_for_target(&first.target)
            .unwrap(),
        1
    );
    assert!(manifest.reminder(first.id).unwrap().actions.is_empty());
    assert_eq!(
        manifest.reminder(second.id).unwrap().actions,
        vec![second_action]
    );
    assert_eq!(
        manifest
            .clear_handled_reminder_history_for_target(&first.target)
            .unwrap(),
        0
    );
}

#[test]
fn a_concurrent_clear_and_offline_handled_append_converge_without_reactivating() {
    let id = ReminderId::new();
    let stored_rule = rule(id, RuleRevisionId::new(), 9);
    let first = action(&stored_rule, ActionId::new(), ActionKind::Handled);
    let concurrent = action(&stored_rule, ActionId::new(), ActionKind::Handled);
    let base = ProfileManifest::create(&profile()).unwrap();
    base.put_reminder_rule(&stored_rule).unwrap();
    base.append_reminder_action(id, &first).unwrap();
    let (a, b) = replicas(&base, 71, 72);
    let av = a.version();
    let bv = b.version();
    assert_eq!(a.clear_handled_reminder_history().unwrap(), 1);
    b.append_reminder_action(id, &concurrent).unwrap();
    merge_since(&a, &av, &b, &bv);

    let a_input = a.reminder_reduction_inputs().pop().unwrap();
    let b_input = b.reminder_reduction_inputs().pop().unwrap();
    assert_eq!(a_input, b_input);
    let occurrence = first.occurrence;
    let reduced = a_input.reduce(TargetState::Active, None);
    assert_eq!(reduced.effective, EffectiveState::Handled { occurrence });
    assert_eq!(reduced.handled_history, BTreeSet::from([occurrence]));
    assert_eq!(a_input.stored().actions, vec![concurrent]);
}

#[test]
fn malformed_entries_are_skipped_beside_valid_siblings() {
    let good_id = ReminderId::new();
    let bad_id = ReminderId::new();
    let good = rule(good_id, RuleRevisionId::new(), 9);
    let manifest = ProfileManifest::create(&profile()).unwrap();
    manifest.put_reminder_rule(&good).unwrap();
    let root = manifest.doc().get_map(C_REMINDERS);
    let rules = root.ensure_mergeable_map(C_REMINDER_RULES).unwrap();
    rules.insert(&bad_id.to_storage(), "not a rule").unwrap();
    let actions = root.ensure_mergeable_map(C_REMINDER_ACTIONS).unwrap();
    actions
        .ensure_mergeable_map(&good_id.to_storage())
        .unwrap()
        .insert(&ActionId::new().to_storage(), "not an action")
        .unwrap();
    manifest.doc().commit();

    assert_eq!(manifest.reminders().len(), 1);
    assert_eq!(manifest.reminder(good_id).unwrap().rule, good);
    assert!(manifest.reminder(good_id).unwrap().actions.is_empty());
}

#[test]
fn an_old_reader_preserves_the_complete_unknown_reminders_root() {
    let id = ReminderId::new();
    let stored_rule = rule(id, RuleRevisionId::new(), 9);
    let handled = action(&stored_rule, ActionId::new(), ActionKind::Handled);
    let current = ProfileManifest::create(&profile()).unwrap();
    current.put_reminder_rule(&stored_rule).unwrap();
    current.append_reminder_action(id, &handled).unwrap();
    current.clear_handled_reminder_history().unwrap();
    current.delete_reminder(id).unwrap();
    current
        .update_reminder_settings(
            Some(Some(LocalTime::new(18, 15, 0).unwrap())),
            Some(ToneId::Chime),
            Some(WeekStartPreference::Monday),
        )
        .unwrap();
    let bytes = current.export_snapshot().unwrap();

    let old = LoroDoc::new();
    old.import(&bytes).unwrap();
    old.get_map(C_META)
        .insert(K_NAME, "Old reader rename")
        .unwrap();
    old.get_map(C_PERSONAL_ATTRIBUTES)
        .ensure_mergeable_map("future-old-reader-key")
        .unwrap()
        .insert("old_reader_value", true)
        .unwrap();
    old.commit();
    let roundtrip = old.export(ExportMode::snapshot()).unwrap();

    let reopened = ProfileManifest::from_snapshot(&roundtrip).unwrap();
    assert_eq!(reopened.profile().unwrap().name, "Old reader rename");
    let reminder = reopened.reminder(id).unwrap();
    assert_eq!(reminder.rule, stored_rule);
    assert!(reminder.deleted);
    assert!(reminder.actions.is_empty());
    assert_eq!(
        reopened.raw_reminder_action(id, handled.id),
        Some(handled.clone())
    );
    assert_eq!(
        reopened.cleared_reminder_actions(),
        BTreeSet::from([(id, handled.id)])
    );
    let input = reopened.reminder_reduction_inputs().pop().unwrap();
    assert!(
        input
            .reduce(TargetState::Active, None)
            .handled_history
            .is_empty()
    );
    assert_eq!(
        reopened.reminder_settings(),
        ReminderSettings {
            end_of_day: LocalTime::new(18, 15, 0),
            default_tone: ToneId::Chime,
            week_start: WeekStartPreference::Monday,
        }
    );
    assert_eq!(crate::version::PROFILE_CRDT_SCHEMA_VERSION, 3);
}
