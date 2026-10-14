// SPDX-License-Identifier: MPL-2.0

use super::*;
use chrono::{NaiveDate, NaiveTime};
use koru_core::time::{DateOnly, Temporal, TimeZoneName, ZonedDateTime};
use koru_when::{DateOrder, Language};

fn context() -> ResolutionContext {
    ResolutionContext {
        now_ms: NaiveDate::from_ymd_opt(2026, 9, 2)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis(),
        language: Language::English,
        date_order: DateOrder::MonthFirst,
        current_zone: TimeZoneName::utc(),
        selection: None,
        end_of_day: None,
        due_anchor: None,
        system_week_start: crate::utils::when::WeekStart::Monday,
        week_start_override: None,
    }
}

fn request(phrase: &str) -> Request {
    Request {
        identity: RequestIdentity {
            token: RequestToken::new(),
            node: NodeId::new(),
        },
        phrase: phrase.into(),
        due_anchor: None,
    }
}

#[test]
fn parser_answers_always_become_an_uncommitted_surface_state() {
    let mut creation = Creation::loading(request("tomorrow at 9am"));
    creation.resolve(&context(), Tone::System);
    assert!(matches!(creation.state, CreationState::Confirmation(_)));
    assert_eq!(creation.request.phrase, "tomorrow at 9am");
    assert_eq!(creation.default_tone, Some(Tone::System));

    let mut empty = Creation::loading(request(""));
    empty.resolve(&context(), Tone::System);
    assert_eq!(empty.state, CreationState::Empty);
}

#[test]
fn passed_ambiguity_and_unset_end_of_day_are_distinct_states() {
    let mut passed = Creation::loading(request("today at 9am"));
    passed.resolve(&context(), Tone::System);
    assert!(matches!(passed.state, CreationState::Passed { .. }));

    let mut ambiguous = Creation::loading(request("tomorrow 9am EST"));
    ambiguous.resolve(&context(), Tone::System);
    assert_eq!(ambiguous.state, CreationState::Ambiguity);

    let mut eod = Creation::loading(request("end of day"));
    eod.resolve(&context(), Tone::System);
    assert_eq!(eod.state, CreationState::EndOfDay);

    let mut set = context();
    set.end_of_day = NaiveTime::from_hms_opt(17, 30, 0);
    eod.resolve(&set, Tone::System);
    assert!(matches!(eod.state, CreationState::Confirmation(_)));
}

#[test]
fn tab_focus_wraps_and_only_confirm_or_cancel_emit_intents() {
    let mut creation = Creation::loading(request("tomorrow at 9am"));
    creation.resolve(&context(), Tone::System);
    assert!(
        creation
            .key(&Key::Named(NamedKey::Enter), ModifiersState::empty())
            .is_none()
    );
    creation.key(&Key::Named(NamedKey::Tab), ModifiersState::empty());
    assert_eq!(creation.focus, CreationFocus::Confirm);
    assert!(matches!(
        creation.key(&Key::Named(NamedKey::Enter), ModifiersState::empty()),
        Some(CreationIntent::Confirm(_))
    ));
    assert!(matches!(creation.state, CreationState::Saving(_)));

    let mut cancelling = Creation::loading(request("tomorrow at 9am"));
    cancelling.resolve(&context(), Tone::System);
    cancelling.focus = CreationFocus::Cancel;
    assert_eq!(
        cancelling.key(&Key::Named(NamedKey::Enter), ModifiersState::empty()),
        Some(CreationIntent::Cancel)
    );
    let mut escaping = Creation::loading(request("tomorrow at 9am"));
    escaping.resolve(&context(), Tone::System);
    assert_eq!(
        escaping.key(&Key::Named(NamedKey::Escape), ModifiersState::empty()),
        Some(CreationIntent::Cancel)
    );
}

#[test]
fn saving_suppresses_duplicate_confirm_and_failed_state_can_retry() {
    let mut creation = Creation::loading(request("tomorrow 9am"));
    creation.resolve(&context(), Tone::System);
    creation.focus = CreationFocus::Confirm;
    let first = creation.key(&Key::Named(NamedKey::Enter), ModifiersState::empty());
    assert!(matches!(first, Some(CreationIntent::Confirm(_))));
    assert_eq!(
        creation.key(&Key::Named(NamedKey::Enter), ModifiersState::empty()),
        None
    );
    assert_eq!(
        creation.key(&Key::Named(NamedKey::Escape), ModifiersState::empty()),
        None
    );

    creation.save_failed(Failure::Unavailable);
    assert!(matches!(creation.state, CreationState::Failed { .. }));
    assert!(matches!(
        creation.key(&Key::Named(NamedKey::Enter), ModifiersState::empty()),
        Some(CreationIntent::Confirm(_))
    ));
}

#[test]
fn saving_draft_is_immutable_until_the_worker_answers() {
    let mut creation = Creation::loading(request("tomorrow 9am"));
    creation.resolve(&context(), Tone::System);
    creation.focus = CreationFocus::Confirm;
    creation.key(&Key::Named(NamedKey::Enter), ModifiersState::empty());
    let CreationState::Saving(before) = creation.state.clone() else {
        panic!("confirmation enters Saving")
    };
    assert_eq!(creation.editable_draft(), None);
    creation.replace_draft(ReminderDraft::Zoned(ZonedDateTime::new(
        123,
        TimeZoneName::utc(),
    )));
    assert_eq!(creation.state, CreationState::Saving(before));
}

#[test]
fn new_drafts_have_no_override_and_edits_preserve_one() {
    let wall = koru_core::time::FloatingDateTime::new(
        NaiveDate::from_ymd_opt(2026, 9, 3)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
    );
    assert_eq!(
        Draft::new(ReminderDraft::Floating(wall)).tone_override,
        None
    );

    let zoned = ZonedDateTime::new(0, TimeZoneName::utc());
    let edited = Draft::preserving(ReminderDraft::Zoned(zoned), Some(Tone::Soft));
    assert_eq!(edited.tone_override, Some(Tone::Soft));
}

#[test]
fn natural_language_edit_preserves_an_existing_tone_override() {
    let mut creation = Creation::loading(request("tomorrow at 9am"));
    creation.resolve_preserving(&context(), Tone::System, Some(Tone::Soft));
    assert_eq!(
        creation
            .editable_draft()
            .and_then(|draft| draft.tone_override),
        Some(Tone::Soft)
    );

    creation.request.phrase = "today at 9am".to_owned();
    creation.resolve_preserving(&context(), Tone::System, Some(Tone::Soft));
    let CreationState::Passed {
        requested,
        proposal,
    } = &creation.state
    else {
        panic!("past edit should offer a proposal");
    };
    assert_eq!(requested.tone_override, Some(Tone::Soft));
    assert_eq!(proposal.tone_override, Some(Tone::Soft));
}

#[test]
fn typed_review_preserves_relative_semantics_anchor_and_tone() {
    let old_anchor = Temporal::Date(DateOnly::new(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()));
    let new_anchor = Temporal::Date(DateOnly::new(NaiveDate::from_ymd_opt(2026, 9, 5).unwrap()));
    let mut current = context();
    current.due_anchor = Some(new_anchor.clone());
    let source = Draft::preserving(
        ReminderDraft::Relative {
            relation: koru_when::DueRelation::At,
            offset: None,
            anchor: Some(old_anchor),
            preview: None,
        },
        Some(Tone::Soft),
    );

    let creation = Creation::reviewing(request(""), source, &current, Tone::System);
    let draft = creation.editable_draft().expect("typed review is editable");
    assert_eq!(draft.tone_override, Some(Tone::Soft));
    assert!(matches!(
        &draft.schedule,
        ReminderDraft::Relative {
            relation: koru_when::DueRelation::At,
            offset: None,
            anchor: Some(anchor),
            preview: Some(_),
        } if anchor == &new_anchor
    ));
}

#[test]
fn typed_past_review_preserves_tone_and_waits_for_confirmation() {
    let source = Draft::preserving(
        ReminderDraft::Floating(koru_core::time::FloatingDateTime::new(
            NaiveDate::from_ymd_opt(2026, 9, 1)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        )),
        Some(Tone::Soft),
    );
    let creation = Creation::reviewing(request(""), source, &context(), Tone::System);
    let CreationState::Passed {
        requested,
        proposal,
    } = creation.state
    else {
        panic!("past typed draft must require the ordinary proposal review")
    };
    assert_eq!(requested.tone_override, Some(Tone::Soft));
    assert_eq!(proposal.tone_override, Some(Tone::Soft));
}

#[test]
fn tone_tokens_are_closed_and_round_trip() {
    assert_eq!(Tone::ALL.len(), 11);
    for tone in Tone::ALL {
        assert_eq!(Tone::parse(tone.token()), Some(tone));
    }
    assert_eq!(Tone::parse("future_tone"), None);
}

#[test]
fn management_counts_live_items_and_keeps_history_collapsed() {
    let draft = Draft::new(ReminderDraft::Floating(
        koru_core::time::FloatingDateTime::new(
            NaiveDate::from_ymd_opt(2026, 9, 3)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        ),
    ));
    let mut management = Management::loading("block:target".into());
    management.replace(
        vec![
            Item {
                id: "a".into(),
                revision: "ra".into(),
                occurrence: None,
                draft: draft.clone(),
                state: ItemState::WaitingForDue,
            },
            Item {
                id: "b".into(),
                revision: "rb".into(),
                occurrence: Some("occ".into()),
                draft: draft.clone(),
                state: ItemState::Snoozed { until_ms: 50 },
            },
        ],
        vec![HistoryItem {
            reminder_id: "old".into(),
            occurrence: "occ".into(),
            draft,
        }],
    );
    assert_eq!(
        management.summary(),
        ChipSummary {
            active_count: 2,
            waiting: true,
            snoozed: true,
        }
    );
    assert!(!management.history_expanded);
    management.toggle_history();
    assert!(management.history_expanded);

    management.apply_local(&ManagementIntent::AskClearHistory);
    assert!(management.clear_history_confirmation);
    management.apply_local(&ManagementIntent::CancelClearHistory);
    assert!(!management.clear_history_confirmation);
    management.apply_local(&ManagementIntent::ConfirmClearHistory);
    assert!(management.loading);
}

#[test]
fn snooze_uses_only_the_six_increments_and_never_goes_negative() {
    assert_eq!(
        SnoozeIncrement::ALL.map(SnoozeIncrement::minutes),
        [1, 5, 10, 15, 30, 60]
    );
    let mut snooze = Snooze::new();
    assert!(!snooze.can_confirm(1_000), "zero is not a snooze");
    snooze.subtract(SnoozeIncrement::OneHour);
    assert_eq!(snooze.total_minutes(), Some(0));
    snooze.add(SnoozeIncrement::OneHour);
    snooze.add(SnoozeIncrement::FifteenMinutes);
    assert_eq!(snooze.total_minutes(), Some(75));
    assert_eq!(snooze.preview_ms(1_000), Some(4_501_000));
    assert!(snooze.can_confirm(1_000));
    assert_eq!(snooze.confirm(2_000), Some(4_502_000));
    snooze.subtract(SnoozeIncrement::OneHour);
    snooze.subtract(SnoozeIncrement::OneHour);
    assert_eq!(snooze.total_minutes(), Some(15));
}

#[test]
fn overflowing_final_instant_disables_confirmation() {
    let mut snooze = Snooze::new();
    snooze.add(SnoozeIncrement::OneMinute);
    assert_eq!(snooze.preview_ms(i64::MAX - 1_000), None);
    assert!(!snooze.can_confirm(i64::MAX - 1_000));
}

#[test]
fn phase_one_global_filters_are_complete_and_all_excludes_history() {
    assert_eq!(GlobalFilter::ALL.len(), 6);
    assert!(GlobalFilter::All.includes(GlobalItemState::NeedsAttention));
    assert!(GlobalFilter::All.includes(GlobalItemState::ComingUp));
    assert!(GlobalFilter::All.includes(GlobalItemState::Snoozed));
    assert!(GlobalFilter::All.includes(GlobalItemState::Paused));
    assert!(!GlobalFilter::All.includes(GlobalItemState::History));
    for (filter, state) in [
        (
            GlobalFilter::NeedsAttention,
            GlobalItemState::NeedsAttention,
        ),
        (GlobalFilter::ComingUp, GlobalItemState::ComingUp),
        (GlobalFilter::Snoozed, GlobalItemState::Snoozed),
        (GlobalFilter::Paused, GlobalItemState::Paused),
        (GlobalFilter::History, GlobalItemState::History),
    ] {
        assert!(filter.includes(state));
    }
}

#[test]
fn global_view_state_filters_real_rows_and_applies_device_attention() {
    let row = |id: &str, occurrence: Option<&str>, state| GlobalItem {
        target: format!("block:{id}"),
        title: id.into(),
        source_location: "Acme".into(),
        reminder_id: id.into(),
        revision: format!("revision-{id}"),
        occurrence: occurrence.map(str::to_owned),
        sort_at_ms: None,
        draft: Draft::new(ReminderDraft::Zoned(ZonedDateTime::new(
            0,
            TimeZoneName::utc(),
        ))),
        snoozed: state == GlobalItemState::Snoozed,
        rule_paused: state == GlobalItemState::Paused,
        state,
    };
    let mut global = GlobalManagement::new(vec![
        row(
            "attention",
            Some("occ-attention"),
            GlobalItemState::ComingUp,
        ),
        row("coming", Some("occ-coming"), GlobalItemState::ComingUp),
        row("snoozed", Some("occ-snoozed"), GlobalItemState::Snoozed),
        row("paused", None, GlobalItemState::Paused),
        row("history", Some("occ-history"), GlobalItemState::History),
    ]);
    global.begin_loading();
    assert!(global.loading);
    global.replace_rows(global.rows.clone(), true);
    assert!(!global.loading);
    assert!(global.updating);
    assert!(global.set_needs_attention([GlobalAttention {
        reminder_id: "attention".into(),
        occurrence: "occ-attention".into(),
        fired_at_ms: 1_700_000_000_000,
    }]));

    assert_eq!(global.visible().count(), 4, "All excludes only History");
    for (filter, expected) in [
        (GlobalFilter::NeedsAttention, vec!["attention"]),
        (GlobalFilter::ComingUp, vec!["coming"]),
        (GlobalFilter::Snoozed, vec!["snoozed"]),
        (GlobalFilter::Paused, vec!["paused"]),
        (GlobalFilter::History, vec!["history"]),
    ] {
        global.set_filter(filter);
        assert_eq!(
            global
                .visible()
                .map(|row| row.reminder_id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }

    assert!(global.set_needs_attention(Vec::new()));
    global.set_filter(GlobalFilter::ComingUp);
    assert_eq!(
        global.visible().count(),
        2,
        "attention removal restores ComingUp"
    );
    global.load_failed(Failure::Unavailable);
    assert_eq!(global.error, Some(Failure::Unavailable));
}

#[test]
fn elapsed_snooze_with_device_delivery_is_needs_attention() {
    let mut global = GlobalManagement::new(vec![GlobalItem {
        target: "block:snoozed".into(),
        title: "snoozed".into(),
        source_location: "Acme".into(),
        reminder_id: "snoozed".into(),
        revision: "revision-snoozed".into(),
        occurrence: Some("occ-snoozed".into()),
        sort_at_ms: Some(1_700_000_000_000),
        draft: Draft::new(ReminderDraft::Zoned(ZonedDateTime::new(
            0,
            TimeZoneName::utc(),
        ))),
        state: GlobalItemState::Snoozed,
        snoozed: true,
        rule_paused: false,
    }]);

    assert!(global.set_needs_attention([GlobalAttention {
        reminder_id: "snoozed".into(),
        occurrence: "occ-snoozed".into(),
        fired_at_ms: 1_700_000_000_001,
    }]));
    global.set_filter(GlobalFilter::NeedsAttention);
    assert_eq!(
        global
            .visible()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["snoozed"]
    );
}

#[test]
fn choosing_snooze_updates_the_cached_row_and_clears_attention_immediately() {
    let mut global = GlobalManagement::new(vec![GlobalItem {
        target: "block:attention".into(),
        title: "attention".into(),
        source_location: "Acme".into(),
        reminder_id: "attention".into(),
        revision: "revision-attention".into(),
        occurrence: Some("occ-attention".into()),
        sort_at_ms: Some(100),
        draft: Draft::new(ReminderDraft::Zoned(ZonedDateTime::new(
            0,
            TimeZoneName::utc(),
        ))),
        state: GlobalItemState::ComingUp,
        snoozed: false,
        rule_paused: false,
    }]);
    global.set_needs_attention([GlobalAttention {
        reminder_id: "attention".into(),
        occurrence: "occ-attention".into(),
        fired_at_ms: 100,
    }]);
    assert_eq!(global.rows[0].state, GlobalItemState::NeedsAttention);

    assert!(global.snooze_local("attention", "occ-attention", 200));
    assert_eq!(global.rows[0].state, GlobalItemState::Snoozed);
    assert_eq!(global.rows[0].sort_at_ms, Some(200));
    global.replace_rows(global.rows.clone(), false);
    assert_eq!(
        global.rows[0].state,
        GlobalItemState::Snoozed,
        "the removed local attention identity cannot undo the optimistic state"
    );
    // The next maintenance pass can still carry the Agent's previous delivery
    // report while its snooze registration is being reconciled.
    global.set_needs_attention([GlobalAttention {
        reminder_id: "attention".into(),
        occurrence: "occ-attention".into(),
        fired_at_ms: 100,
    }]);
    assert_eq!(
        global.rows[0].state,
        GlobalItemState::Snoozed,
        "the pre-snooze delivery cannot restore attention or the original pill"
    );
    global.replace_rows(global.rows.clone(), false);
    assert_eq!(global.rows[0].state, GlobalItemState::Snoozed);
    assert!(global.rows[0].snoozed);
    assert_eq!(global.rows[0].sort_at_ms, Some(200));

    // Delivery at the new deadline earns attention without losing the pill's
    // effective snooze schedule, even if the device later retracts attention.
    global.set_needs_attention([GlobalAttention {
        reminder_id: "attention".into(),
        occurrence: "occ-attention".into(),
        fired_at_ms: 200,
    }]);
    assert_eq!(global.rows[0].state, GlobalItemState::NeedsAttention);
    assert!(global.rows[0].snoozed);
    assert_eq!(global.rows[0].sort_at_ms, Some(200));
    global.set_needs_attention([]);
    assert_eq!(global.rows[0].state, GlobalItemState::Snoozed);

    assert!(global.snooze_local("attention", "occ-attention", 300));
    global.set_needs_attention([GlobalAttention {
        reminder_id: "attention".into(),
        occurrence: "occ-attention".into(),
        fired_at_ms: 200,
    }]);
    assert_eq!(global.rows[0].state, GlobalItemState::Snoozed);
    assert_eq!(global.rows[0].sort_at_ms, Some(300));
}
