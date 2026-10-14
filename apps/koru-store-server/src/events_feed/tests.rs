// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_projection::{AttributeChange, AttributeEvent, AttributeEventKind};
use koru_store_core::{
    DictionaryCommitted, IdentityChange, IdentityCommitted, ProfileId, ReminderChange,
    ReminderCommitted, ReminderId,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct FlakySink {
    failures_left: AtomicUsize,
    delivered: Arc<Mutex<Vec<Vec<FeedNote>>>>,
}

impl FeedSink for FlakySink {
    // `fetch_update` is deprecated for `try_update`, which is unstable on the
    // Rust 1.93 the Linux API image builds with; rename when that image moves.
    #[allow(deprecated)]
    fn feed(&self, notes: &[FeedNote]) -> Result<u32, String> {
        if self
            .failures_left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err("injected journal failure".into());
        }
        self.delivered.lock().unwrap().push(notes.to_vec());
        Ok(notes.len() as u32)
    }
}

#[test]
fn this_suite_sends_no_telemetry() {
    assert!(!koru_events_server::forward::sends_by_default());
    assert!(koru_events_server::forward::configured_sink().is_none());
}

#[test]
fn reminder_wakes_map_every_change_without_content() {
    let profile = ProfileId::new();
    let reminder = ReminderId::new();
    let changes = [
        ReminderChange::Rule,
        ReminderChange::Action,
        ReminderChange::History,
        ReminderChange::Settings,
    ]
    .map(|change| ReminderCommitted {
        profile,
        reminder: Some(reminder),
        change,
    });
    let notes = reminder_notes(&changes);
    assert_eq!(notes.len(), 4);
    for (note, expected) in notes.iter().zip([
        ReminderWake::Rule,
        ReminderWake::Action,
        ReminderWake::History,
        ReminderWake::Settings,
    ]) {
        assert!(matches!(
            note,
            FeedNote::ReminderChanged { profile: value, change }
                if value == &profile.to_storage() && *change == expected
        ));
    }
}

#[test]
fn dictionary_wake_names_only_its_profile() {
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let mut feed = EventsFeed {
        sink: Box::new(FlakySink {
            failures_left: AtomicUsize::new(0),
            delivered: Arc::clone(&delivered),
        }),
        projections: Projections::in_memory().unwrap(),
        pending: BTreeMap::new(),
    };
    let profile = ProfileId::new();
    feed.dictionary_changes(&[DictionaryCommitted { profile }]);
    assert!(matches!(
        &delivered.lock().unwrap()[0][0],
        FeedNote::DictionaryChanged { profile: value } if value == &profile.to_storage()
    ));
}

/// The imported-identity wake reaches the journal as its own note, and the two
/// facts stay apart: one key each, so a rename and a recolour in the same pass
/// are two notes rather than one overwriting the other.
#[test]
fn an_imported_identity_wake_carries_which_fact_moved() {
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let mut feed = EventsFeed {
        sink: Box::new(FlakySink {
            failures_left: AtomicUsize::new(0),
            delivered: Arc::clone(&delivered),
        }),
        projections: Projections::in_memory().unwrap(),
        pending: BTreeMap::new(),
    };
    let profile = ProfileId::new();
    feed.identity_changes(&[
        IdentityCommitted {
            profile,
            change: IdentityChange::Name,
        },
        IdentityCommitted {
            profile,
            change: IdentityChange::Appearance,
        },
    ]);
    let delivered = delivered.lock().unwrap();
    let notes = &delivered[0];
    assert_eq!(notes.len(), 2);
    for (note, expected) in notes
        .iter()
        .zip([IdentityWake::Appearance, IdentityWake::Name])
    {
        assert!(matches!(
            note,
            FeedNote::ProfileIdentityChanged { profile: value, change }
                if value == &profile.to_storage() && *change == expected
        ));
    }
}

#[test]
fn due_changes_and_entity_removals_map_only_ids() {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let due = AttributeEvent {
        workspace,
        document,
        node: "11111111-1111-4111-8111-111111111111".into(),
        kind: AttributeEventKind::DueChanged {
            change: AttributeChange::Cleared,
        },
    };
    let notes = due_notes(&[due]);
    assert!(matches!(
        &notes[0],
        FeedNote::DueChanged { workspace: w, document: d, entity }
            if w == &workspace.to_storage()
                && d == &document.to_storage()
                && entity == "11111111-1111-4111-8111-111111111111"
    ));
    let removed = entity_removed_notes(
        workspace,
        document,
        &["22222222-2222-4222-8222-222222222222".into()],
    );
    assert!(matches!(
        &removed[0],
        FeedNote::EntityRemoved { workspace: w, document: d, entity }
            if w == &workspace.to_storage()
                && d == &document.to_storage()
                && entity == "22222222-2222-4222-8222-222222222222"
    ));
}

#[test]
fn a_failed_feed_is_coalesced_and_retried_without_another_commit() {
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let mut feed = EventsFeed {
        sink: Box::new(FlakySink {
            failures_left: AtomicUsize::new(1),
            delivered: Arc::clone(&delivered),
        }),
        projections: Projections::in_memory().unwrap(),
        pending: BTreeMap::new(),
    };
    let profile = ProfileId::new().to_storage();

    feed.deliver(vec![FeedNote::ReminderChanged {
        profile: profile.clone(),
        change: ReminderWake::Action,
    }]);
    assert_eq!(feed.pending.len(), 1);
    assert!(delivered.lock().unwrap().is_empty());

    // The next idle projector tick invokes this method even with no new store
    // commit.
    feed.flush_pending();
    assert!(feed.pending.is_empty());
    let delivered = delivered.lock().unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].len(), 1);
    assert!(matches!(
        &delivered[0][0],
        FeedNote::ReminderChanged { profile: value, change: ReminderWake::Action }
            if value == &profile
    ));
}
