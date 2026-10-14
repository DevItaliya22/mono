// SPDX-License-Identifier: MPL-2.0

//! The ports are traits, so most of what matters about them is proved by the
//! implementations. What can be tested here is [`Opened`] — the shape that
//! makes a recovery impossible to drop silently.

use super::*;

fn recovery() -> Recovery {
    Recovery {
        problem: "checkpoint 7 failed its checksum".into(),
        from_checkpoint: Some(6),
        quarantined: vec!["quarantine/1770000000000/checkpoint-7".into()],
        skipped_updates: 2,
    }
}

#[test]
fn a_normal_open_reports_no_recovery() {
    let o = Opened::Normal(42);
    assert_eq!(o.value(), &42);
    assert!(o.recovery().is_none());
}

#[test]
fn a_fresh_open_is_distinguishable_from_a_normal_one() {
    // "Nothing was there" and "there was nothing in it" are different answers,
    // and a caller that cannot tell them apart cannot decide whether to seed.
    let fresh: Opened<Vec<u8>> = Opened::Fresh(Vec::new());
    let normal: Opened<Vec<u8>> = Opened::Normal(Vec::new());
    assert_ne!(fresh, normal);
    assert!(fresh.recovery().is_none() && normal.recovery().is_none());
}

#[test]
fn a_recovery_travels_with_the_value() {
    let o = Opened::Recovered(42, recovery());
    assert_eq!(o.value(), &42);
    let r = o.recovery().expect("a recovery must be reachable");
    assert_eq!(r.from_checkpoint, Some(6));
    assert_eq!(r.skipped_updates, 2);
    assert_eq!(r.quarantined.len(), 1);
}

#[test]
fn unwrapping_keeps_the_value_whichever_way_it_opened() {
    assert_eq!(Opened::Fresh(1).into_value(), 1);
    assert_eq!(Opened::Normal(1).into_value(), 1);
    assert_eq!(Opened::Recovered(1, recovery()).into_value(), 1);
}

#[test]
fn a_document_with_no_checkpoint_is_representable() {
    // The journal alone is a whole document until the first checkpoint. If
    // this were not expressible, the first few edits after a create would have
    // nowhere valid to live.
    let s = DocumentState {
        id: DocumentId::new(),
        kind: DocumentKind::Manifest,
        latest_checkpoint_seq: None,
        version_vector: Vec::new(),
        frontiers: Vec::new(),
        updated_at: Timestamp::from_millis(0),
    };
    assert_eq!(s.latest_checkpoint_seq, None);
}

#[test]
fn a_batch_carries_the_position_it_establishes() {
    // The updates and the version vector they produce travel together, because
    // they have to be written in one transaction: a journal ahead of its
    // position replays applied work, and a position ahead of its journal claims
    // durability for bytes that are gone.
    let batch = UpdateBatch {
        document: DocumentId::new(),
        updates: vec![vec![1, 2, 3], vec![4, 5]],
        version_vector: vec![9],
        frontiers: vec![8],
        received_at: Timestamp::from_millis(1),
        origins: vec![UpdateOrigin::Store, UpdateOrigin::Store],
    };
    assert_eq!(batch.updates.len(), 2);
    assert!(!batch.version_vector.is_empty());
}
