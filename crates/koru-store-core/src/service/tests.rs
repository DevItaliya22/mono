// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::text::Locale;
use crate::{
    ActionKind, DocumentId, EntityRef, FloatingDateTime, LocalTime, OccurrenceKey,
    OccurrenceMoment, OccurrenceSource, OneTimeSchedule, Profile, ReminderRule, RuleRevisionId,
    RuleStatus, Timestamp, ToneId, WeekStartPreference, WorkspaceId,
};

fn rule(id: ReminderId, revision: RuleRevisionId, hour: u8) -> ReminderRule {
    ReminderRule {
        id,
        target: EntityRef::new(
            "node",
            WorkspaceId::new(),
            DocumentId::new(),
            uuid::Uuid::new_v4(),
        )
        .unwrap(),
        revision,
        status: RuleStatus::Enabled,
        schedule: OneTimeSchedule::Floating {
            wall: FloatingDateTime::parse(&format!("2026-09-02T{hour:02}:00:00")).unwrap(),
        },
        tone_override: None,
    }
}

fn action(rule: &ReminderRule, id: ActionId, kind: ActionKind) -> OccurrenceAction {
    let moment = match &rule.schedule {
        OneTimeSchedule::Floating { wall } => OccurrenceMoment::Floating { wall: wall.clone() },
        _ => unreachable!(),
    };
    let source = OccurrenceSource::OneTime { moment };
    OccurrenceAction {
        id,
        occurrence: OccurrenceKey::derive(rule.id, rule.revision, &source),
        kind,
    }
}

#[test]
fn an_imported_clear_is_history_not_an_action_append() {
    let reminder = ReminderId::new();
    let action = ActionId::new();
    let before_cleared = BTreeSet::new();
    let after_cleared = BTreeSet::from([(reminder, action)]);
    let changes = classify_reminder_changes(
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &before_cleared,
        &after_cleared,
        &ReminderSettings::default(),
        &ReminderSettings::default(),
    );

    assert_eq!(changes, vec![(None, ReminderChange::History)]);
}

#[test]
fn create_and_update_retries_are_exact_and_conflicting_id_reuse_is_rejected() {
    let id = ReminderId::new();
    let original = rule(id, RuleRevisionId::new(), 9);
    let stored = StoredReminder {
        rule: original.clone(),
        deleted: false,
        actions: Vec::new(),
    };
    assert!(create_retry(&stored, &original).is_ok());
    assert!(!update_writes(&stored, &original).unwrap());

    let conflict = rule(id, original.revision, 10);
    assert_eq!(
        create_retry(&stored, &conflict).unwrap_err().code(),
        Code::IdempotencyConflict
    );
    assert_eq!(
        update_writes(&stored, &conflict).unwrap_err().code(),
        Code::IdempotencyConflict
    );
    let new_revision = rule(id, RuleRevisionId::new(), 10);
    assert!(update_writes(&stored, &new_revision).unwrap());
}

#[test]
fn delayed_action_retries_stay_exact_even_when_clear_all_hides_the_action() {
    let stored_rule = rule(ReminderId::new(), RuleRevisionId::new(), 9);
    let id = ActionId::new();
    let handled = action(&stored_rule, id, ActionKind::Handled);
    assert!(action_retry(&handled, &handled).is_ok());
    let conflict = action(
        &stored_rule,
        id,
        ActionKind::SnoozedUntil { instant_ms: 10 },
    );
    assert_eq!(
        action_retry(&handled, &conflict).unwrap_err().code(),
        Code::IdempotencyConflict
    );
}

fn reject_profile_commit(manifest: &mut ProfileManifest, before: &[u8]) -> Error {
    commit_profile_mutation(manifest, 77, before, |_profile, _snapshot| {
        Err(Error::new(Code::StorageFailed, "injected profile failure"))
    })
    .unwrap_err()
}

fn accept_profile_commit(manifest: &mut ProfileManifest, before: &[u8]) -> Vec<u8> {
    let mut durable = None;
    commit_profile_mutation(manifest, 77, before, |_profile, snapshot| {
        durable = Some(snapshot.to_vec());
        Ok(())
    })
    .unwrap();
    durable.unwrap()
}

#[test]
fn failed_profile_commits_hide_nothing_and_exact_retries_make_every_reminder_fact_durable() {
    let profile = Profile::new("Test", Locale::default(), Timestamp::from_millis(1)).unwrap();
    let mut manifest = ProfileManifest::create(&profile).unwrap();
    manifest.set_peer_id(77).unwrap();
    let id = ReminderId::new();
    let stored_rule = rule(id, RuleRevisionId::new(), 9);

    let before = manifest.export_snapshot().unwrap();
    manifest.put_reminder_rule(&stored_rule).unwrap();
    assert_eq!(
        reject_profile_commit(&mut manifest, &before).code(),
        Code::StorageFailed
    );
    assert!(
        manifest.reminder(id).is_none(),
        "failed create stayed visible"
    );
    manifest.put_reminder_rule(&stored_rule).unwrap();
    let mut prior = manifest.export_snapshot().unwrap();
    let mut durable = accept_profile_commit(&mut manifest, &before);
    assert_eq!(
        ProfileManifest::from_snapshot(&durable)
            .unwrap()
            .reminder(id)
            .unwrap()
            .rule,
        stored_rule
    );

    let snooze = action(
        &stored_rule,
        ActionId::new(),
        ActionKind::SnoozedUntil { instant_ms: 10 },
    );
    manifest.append_reminder_action(id, &snooze).unwrap();
    assert_eq!(
        reject_profile_commit(&mut manifest, &prior).code(),
        Code::StorageFailed
    );
    assert!(manifest.reminder(id).unwrap().actions.is_empty());
    manifest.append_reminder_action(id, &snooze).unwrap();
    prior = manifest.export_snapshot().unwrap();
    durable = accept_profile_commit(&mut manifest, &durable);
    assert_eq!(
        ProfileManifest::from_snapshot(&durable)
            .unwrap()
            .raw_reminder_action(id, snooze.id),
        Some(snooze.clone())
    );

    manifest
        .update_reminder_settings(
            Some(Some(LocalTime::new(17, 0, 0).unwrap())),
            Some(ToneId::Chime),
            Some(WeekStartPreference::Sunday),
        )
        .unwrap();
    assert_eq!(
        reject_profile_commit(&mut manifest, &prior).code(),
        Code::StorageFailed
    );
    assert_eq!(manifest.reminder_settings(), ReminderSettings::default());
    manifest
        .update_reminder_settings(
            Some(Some(LocalTime::new(17, 0, 0).unwrap())),
            Some(ToneId::Chime),
            Some(WeekStartPreference::Sunday),
        )
        .unwrap();
    prior = manifest.export_snapshot().unwrap();
    durable = accept_profile_commit(&mut manifest, &durable);
    assert_eq!(
        ProfileManifest::from_snapshot(&durable)
            .unwrap()
            .reminder_settings()
            .end_of_day,
        LocalTime::new(17, 0, 0)
    );

    let handled = action(&stored_rule, ActionId::new(), ActionKind::Handled);
    manifest.append_reminder_action(id, &handled).unwrap();
    durable = accept_profile_commit(&mut manifest, &prior);
    let before_clear = durable.clone();
    assert_eq!(manifest.clear_handled_reminder_history().unwrap(), 1);
    assert_eq!(
        reject_profile_commit(&mut manifest, &before_clear).code(),
        Code::StorageFailed
    );
    assert!(manifest.reminder(id).unwrap().actions.contains(&handled));
    assert_eq!(manifest.clear_handled_reminder_history().unwrap(), 1);
    durable = accept_profile_commit(&mut manifest, &before_clear);
    let reopened = ProfileManifest::from_snapshot(&durable).unwrap();
    assert_eq!(reopened.reminder(id).unwrap().actions, vec![snooze]);
    assert_eq!(reopened.raw_reminder_action(id, handled.id), Some(handled));

    let before_delete = durable.clone();
    manifest.delete_reminder(id).unwrap();
    assert_eq!(
        reject_profile_commit(&mut manifest, &before_delete).code(),
        Code::StorageFailed
    );
    assert!(!manifest.reminder(id).unwrap().deleted);
    manifest.delete_reminder(id).unwrap();
    durable = accept_profile_commit(&mut manifest, &before_delete);
    assert!(
        ProfileManifest::from_snapshot(&durable)
            .unwrap()
            .reminder(id)
            .unwrap()
            .deleted
    );
}

#[test]
fn a_failed_remote_profile_import_is_rolled_back_and_the_same_frame_can_commit() {
    let profile = Profile::new("Test", Locale::default(), Timestamp::from_millis(1)).unwrap();
    let mut local = ProfileManifest::create(&profile).unwrap();
    local.set_peer_id(77).unwrap();
    let durable_before = local.export_snapshot().unwrap();
    let remote = ProfileManifest::from_snapshot(&durable_before).unwrap();
    remote.set_peer_id(88).unwrap();
    let remote_before = remote.version();
    let remote_rule = rule(ReminderId::new(), RuleRevisionId::new(), 11);
    remote.put_reminder_rule(&remote_rule).unwrap();
    let frame = remote.export_updates_since(&remote_before).unwrap();

    local.import_frames(std::slice::from_ref(&frame)).unwrap();
    assert_eq!(
        reject_profile_commit(&mut local, &durable_before).code(),
        Code::StorageFailed
    );
    assert!(local.reminder(remote_rule.id).is_none());

    local.import_frames(&[frame]).unwrap();
    let durable = accept_profile_commit(&mut local, &durable_before);
    assert_eq!(
        ProfileManifest::from_snapshot(&durable)
            .unwrap()
            .reminder(remote_rule.id)
            .unwrap()
            .rule,
        remote_rule
    );
}

/// **Exactly one device ever creates a given manifest. Every other device
/// receives it.**
///
/// The behavioural half of this invariant is in
/// `koru-store-sqlite/src/service_tests/manifest_origin.rs`, which walks the
/// three real paths with a real catalog and a real restart. This is the part
/// that cannot be written behaviourally: a *fourth* mint site added later would
/// pass every one of those tests by not being reached by them.
///
/// So the source is read. Each function that calls `IdentityManifest::create`
/// must also name the origin marker — by asking `may_mint_identity`, or by
/// recording `ORIGIN_CREATED_HERE` because it is the path on which a person
/// actually creates a Profile here.
///
/// It proves a mention, not a correct guard. What it buys is that the next
/// author cannot add a mint site *silently*: a new one has to be argued for in
/// a diff rather than appear in one.
#[test]
fn every_identity_mint_site_names_the_origin_marker() {
    const SOURCE: &str = include_str!("../service.rs");
    const SENTINEL: &str = "\u{0}fn\u{0}";
    let chunked = SOURCE
        .replace("\n    pub fn ", SENTINEL)
        .replace("\n    fn ", SENTINEL);
    let minting: Vec<&str> = chunked
        .split(SENTINEL)
        .filter(|body| body.contains("IdentityManifest::create("))
        .collect();

    assert_eq!(
        minting.len(),
        3,
        "the mint sites are `open`, `create_profile_with_appearance` and \
         `restore_profile_local`. A fourth needs the origin marker and a test \
         in manifest_origin.rs before this count moves",
    );
    for body in minting {
        let name = body.lines().next().unwrap_or_default();
        assert!(
            body.contains("may_mint_identity(") || body.contains("ORIGIN_CREATED_HERE"),
            "`{name}` authors an identity lineage without consulting the origin \
             marker. A device that was only told a Profile exists must receive \
             its manifest, never mint one",
        );
    }
}
