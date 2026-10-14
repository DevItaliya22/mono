// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::ids::{ClientId, WorkspaceId};
use crate::manifest::Manifest;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000 + ms)
}

/// A live document with some real Loro history in it.
fn written() -> LiveDocument {
    let mut manifest = Manifest::create(WorkspaceId::new(), "W", at(0)).unwrap();
    manifest
        .add_document(DocumentId::new(), "四半期計画", "page", None)
        .unwrap();
    LiveDocument::from_snapshot(DocumentId::new(), &manifest.export_snapshot().unwrap()).unwrap()
}

// ------------------------------------------------------------------ checksum

#[test]
fn a_checksum_is_stable_and_content_addressed() {
    assert_eq!(checksum(b"hello"), checksum(b"hello"));
    assert_ne!(checksum(b"hello"), checksum(b"hellp"));
    // 32 bytes of BLAKE3 as hex.
    assert_eq!(checksum(b"hello").len(), 64);
    assert!(checksum(b"").chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn cloud_preflight_preserves_older_device_pending_operations() {
    let mut manifest = Manifest::create(WorkspaceId::new(), "Pending", at(0)).unwrap();
    let mut live =
        LiveDocument::from_snapshot(DocumentId::new(), &manifest.export_snapshot().unwrap())
            .unwrap();
    let source = live.doc().fork();
    source.set_peer_id(710).unwrap();
    let before = source.oplog_vv();
    source.get_map("causal").insert("ancestor", 1).unwrap();
    source.commit();
    let ancestor = source.export(ExportMode::updates(&before)).unwrap();
    let after = source.oplog_vv();
    source.get_map("causal").insert("child", 2).unwrap();
    source.commit();
    let child = source.export(ExportMode::updates(&after)).unwrap();
    live.import(&child).unwrap();
    manifest.import(&child).unwrap();
    assert!(live.doc().get_map("causal").get("child").is_none());
    let independent = LoroDoc::new();
    independent.set_peer_id(711).unwrap();
    independent.get_map("cloud").insert("valid", true).unwrap();
    independent.commit();
    let cloud = independent.export(ExportMode::all_updates()).unwrap();
    live.import_cloud(&cloud).unwrap();
    manifest.import_cloud(&cloud).unwrap();
    live.import(&ancestor).unwrap();
    manifest.import(&ancestor).unwrap();
    assert!(live.doc().get_map("causal").get("child").is_some());
    let recovered = LoroDoc::new();
    recovered
        .import(&manifest.export_snapshot().unwrap())
        .unwrap();
    assert!(recovered.get_map("causal").get("child").is_some());
    assert!(recovered.get_map("cloud").get("valid").is_some());
}

#[test]
fn a_checksum_is_never_empty() {
    // The failure this guards: an injected hash left uninstalled would return
    // "" for everything, and every verifier would accept every checkpoint.
    assert!(!checksum(b"").is_empty());
}

// ------------------------------------------------------------------ live document

#[test]
fn an_empty_document_can_take_its_first_update() {
    let source = written();
    let mut live = LiveDocument::empty(source.id());
    live.import(&source.export_all_updates().unwrap()).unwrap();
    assert!(live.version().includes_vv(&source.version()));
}

#[test]
fn a_snapshot_round_trips_into_a_live_document() {
    let source = written();
    let back =
        LiveDocument::from_snapshot(source.id(), &source.export_snapshot().unwrap()).unwrap();
    assert!(back.version().includes_vv(&source.version()));
    assert!(source.version().includes_vv(&back.version()));
}

#[test]
fn a_bad_snapshot_is_corrupt_state_not_a_panic() {
    let e = LiveDocument::from_snapshot(DocumentId::new(), b"not loro").unwrap_err();
    assert_eq!(e.code(), Code::CorruptState);
}

#[test]
fn a_delta_whose_ancestors_are_missing_applies_nothing_and_still_returns_ok() {
    // **A Loro behaviour worth knowing before it costs a day.** An update
    // exported from a non-empty version names causal ancestors; a replica that
    // does not have them parks it as *pending* — `import` returns `Ok` and the
    // document does not move. There is no error to notice.
    //
    // The consequence for the store: a journal is only replayable if its first
    // entry starts from an empty version vector. The protocol gives that for
    // free — a client syncing a document the store has never seen is asked for
    // everything from an empty VV — but anything constructing a journal by hand
    // has to arrange it deliberately.
    let mut source = Manifest::create(WorkspaceId::new(), "W", at(0)).unwrap();
    let after_create = source.version();
    source
        .add_document(DocumentId::new(), "Orphaned", "page", None)
        .unwrap();
    let delta = source.export_updates_since(&after_create).unwrap();

    let mut empty = LiveDocument::empty(DocumentId::new());
    let before = empty.version();
    empty
        .import(&delta)
        .expect("Loro accepts it without complaint");

    assert!(
        before.includes_vv(&empty.version()) && empty.version().includes_vv(&before),
        "the document moved; this trap may have been fixed upstream"
    );
    assert_eq!(
        Manifest::from_snapshot(&empty.export_snapshot().unwrap())
            .unwrap()
            .entries()
            .len(),
        0,
        "the parked update was applied after all"
    );

    // From an empty version vector, the same history applies.
    let whole = source.export_all_updates().unwrap();
    let mut fed = LiveDocument::empty(DocumentId::new());
    fed.import(&whole).unwrap();
    assert_eq!(
        Manifest::from_snapshot(&fed.export_snapshot().unwrap())
            .unwrap()
            .entries()
            .len(),
        1
    );
}

#[test]
fn an_update_that_does_not_apply_is_refused_before_anything_is_written() {
    // The validation step. The journal must never contain bytes that cannot be
    // replayed, which is why import happens before persist.
    let mut live = written();
    assert_eq!(
        live.import(b"rubbish").unwrap_err().code(),
        Code::CrdtRejected
    );
}

#[test]
fn the_encoded_position_round_trips_through_loro() {
    let live = written();
    let decoded = loro::VersionVector::decode(&live.encoded_version()).expect("decode");
    assert!(decoded.includes_vv(&live.version()));
    assert!(live.version().includes_vv(&decoded));
}

#[test]
fn a_known_frame_is_recognised_from_its_header() {
    let source = written();
    let frame = source.export_all_updates().unwrap();
    let mut live = LiveDocument::empty(source.id());
    assert!(!live.frame_is_known(&frame));
    live.import(&frame).unwrap();
    assert!(live.frame_is_known(&frame));
}

#[test]
fn a_peer_id_can_be_pinned() {
    let live = LiveDocument::empty(DocumentId::new());
    live.set_peer_id(0xBEEF).unwrap();
    assert_eq!(live.peer_id(), 0xBEEF);
}

#[test]
fn a_checkpoint_carries_a_checksum_of_its_own_bytes() {
    let live = written();
    let cp = live.checkpoint(7, at(100)).unwrap();
    assert_eq!(cp.seq, 7);
    assert_eq!(cp.checksum, checksum(&cp.snapshot));
    assert_eq!(cp.version_vector, live.encoded_version());
    assert_eq!(cp.created_at, at(100));
}

// ------------------------------------------------------------------ group commit

#[test]
fn an_empty_window_is_never_due_and_yields_nothing() {
    let live = written();
    let mut w = CommitWindow::new(live.id());
    assert!(w.is_empty());
    assert!(!w.due(at(10_000)), "an empty window must not wake a commit");
    assert_eq!(w.due_at(), None);
    assert!(w.take(vec![], vec![], at(0)).is_none());
}

#[test]
fn a_window_becomes_due_once_its_time_has_passed() {
    let live = written();
    let mut w = CommitWindow::new(live.id());
    w.push(vec![1], at(0), UpdateOrigin::Store);

    assert!(!w.due(at(0)));
    assert!(!w.due(at(CommitWindow::DEFAULT_WINDOW_MS - 1)));
    assert!(w.due(at(CommitWindow::DEFAULT_WINDOW_MS)));
    assert_eq!(w.due_at(), Some(at(CommitWindow::DEFAULT_WINDOW_MS)));
}

#[test]
fn the_window_is_measured_from_the_first_update_not_the_last() {
    // Otherwise steady typing keeps pushing the deadline back and nothing is
    // ever acknowledged — a livelock that only appears under sustained input.
    let live = written();
    let mut w = CommitWindow::new(live.id());
    w.push(vec![1], at(0), UpdateOrigin::Store);
    w.push(vec![2], at(3), UpdateOrigin::Store);
    w.push(vec![3], at(4), UpdateOrigin::Store);
    assert!(w.due(at(5)), "the deadline moved with the last update");
}

#[test]
fn a_flood_commits_on_count_rather_than_growing_without_bound() {
    let live = written();
    let mut w = CommitWindow::new(live.id());
    w.max_updates = 3;
    w.push(vec![1], at(0), UpdateOrigin::Store);
    w.push(vec![2], at(0), UpdateOrigin::Store);
    assert!(!w.due(at(0)));
    w.push(vec![3], at(0), UpdateOrigin::Store);
    assert!(w.due(at(0)), "the count cap did not fire");
}

#[test]
fn taking_a_window_empties_it_and_carries_the_documents_position() {
    let live = written();
    let mut w = CommitWindow::new(live.id());
    w.push(vec![1], at(0), UpdateOrigin::Store);
    w.push(vec![2], at(1), UpdateOrigin::Store);

    let batch = w
        .take(live.encoded_version(), live.encoded_frontiers(), at(5))
        .expect("two updates");
    assert_eq!(batch.updates, vec![vec![1], vec![2]]);
    assert_eq!(batch.document, live.id());
    // The position written is the one the live document actually reached.
    assert_eq!(batch.version_vector, live.encoded_version());
    assert_eq!(batch.received_at, at(5));

    assert!(w.is_empty());
    assert!(!w.due(at(10_000)), "the window did not close");
    assert!(w.take(vec![], vec![], at(6)).is_none());
}

#[test]
fn a_window_preserves_every_origin_even_when_mixed() {
    let live = written();
    let a = ClientId::new();
    let b = ClientId::new();

    let mut single = CommitWindow::new(live.id());
    single.push(vec![1], at(0), UpdateOrigin::LocalClient { client_id: a });
    single.push(vec![2], at(1), UpdateOrigin::LocalClient { client_id: a });
    assert_eq!(
        single
            .take(live.encoded_version(), live.encoded_frontiers(), at(5))
            .unwrap()
            .origins,
        vec![
            UpdateOrigin::LocalClient { client_id: a },
            UpdateOrigin::LocalClient { client_id: a }
        ]
    );

    let mut mixed = CommitWindow::new(live.id());
    mixed.push(vec![1], at(0), UpdateOrigin::LocalClient { client_id: a });
    mixed.push(vec![2], at(1), UpdateOrigin::LocalClient { client_id: b });
    assert_eq!(
        mixed
            .take(live.encoded_version(), live.encoded_frontiers(), at(5))
            .unwrap()
            .origins,
        vec![
            UpdateOrigin::LocalClient { client_id: a },
            UpdateOrigin::LocalClient { client_id: b }
        ]
    );
}

#[test]
fn a_window_reopens_after_being_taken() {
    let live = written();
    let mut w = CommitWindow::new(live.id());
    w.push(vec![1], at(0), UpdateOrigin::Store);
    w.take(live.encoded_version(), live.encoded_frontiers(), at(5))
        .unwrap();

    w.push(vec![2], at(100), UpdateOrigin::Store);
    assert_eq!(w.due_at(), Some(at(100 + CommitWindow::DEFAULT_WINDOW_MS)));
    assert!(!w.due(at(100)));
    assert!(w.due(at(105)));
}

// ------------------------------------------------------------------ checkpoints

#[test]
fn a_retained_commit_resource_survives_failed_write_and_successful_batch_handoff() {
    let live = written();
    let mut window = CommitWindow::new(live.id());
    let owner = std::sync::Arc::new(());
    let bytes = vec![1, 2, 3];
    let original = bytes.as_ptr();
    window.push_retained(
        bytes,
        at(0),
        UpdateOrigin::Store,
        Some(Box::new(owner.clone())),
    );
    assert_eq!(std::sync::Arc::strong_count(&owner), 2);
    let failed = window.commit(
        live.encoded_version(),
        live.encoded_frontiers(),
        at(5),
        |batch| {
            assert_eq!(batch.updates[0].as_ptr(), original);
            assert_eq!(std::sync::Arc::strong_count(&owner), 2);
            Err(Error::bare(Code::StorageFailed))
        },
    );
    assert!(failed.is_err());
    assert_eq!(std::sync::Arc::strong_count(&owner), 2);
    assert_eq!(window.due_at(), Some(at(5)));
    let committed = window
        .commit(
            live.encoded_version(),
            live.encoded_frontiers(),
            at(6),
            |batch| {
                assert_eq!(batch.updates[0].as_ptr(), original);
                Ok(7)
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(committed.head, 7);
    assert_eq!(committed.batch.updates[0].as_ptr(), original);
    assert!(window.is_empty());
    drop(window);
    assert_eq!(std::sync::Arc::strong_count(&owner), 2);
    drop(committed);
    assert_eq!(std::sync::Arc::strong_count(&owner), 1);
}

#[test]
fn a_checkpoint_is_due_on_either_limit() {
    let p = CheckpointPolicy {
        every_updates: 10,
        every_bytes: 1000,
    };
    assert!(!p.due(9, 999));
    // Many small updates.
    assert!(p.due(10, 0));
    // A few enormous ones. Either limit alone would miss one of these cases.
    assert!(p.due(1, 1000));
}

#[test]
fn the_default_policy_does_not_checkpoint_on_every_edit() {
    let p = CheckpointPolicy::default();
    assert!(!p.due(1, 100));
    assert!(p.due(p.every_updates, 0));
    assert!(p.due(0, p.every_bytes));
}
