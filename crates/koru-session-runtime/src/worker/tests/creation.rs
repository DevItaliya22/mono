// SPDX-License-Identifier: MPL-2.0

//! The four recovery scenarios of §7.6, at the registry.
//!
//! Each asks the question the orchestrator will ask after a restart — did
//! my creation happen, and may I start another? — and proves the answer
//! comes from the ledger, never from spawning. The host-level half, over
//! real IPC with a real host, is `apps/koru-agent/tests/sessions_delegation.rs`.

use super::*;
use crate::CreationAdmission;
use koru_session_protocol::{CreationLookup, IdempotencyKey};

fn reserve(registry: &SessionRegistry, key: IdempotencyKey) -> crate::CreationReservation<'_> {
    match registry.admit_creation(key, Default::default(), None) {
        CreationAdmission::Reserved(reservation) => reservation,
        _ => panic!("a fresh key is admitted"),
    }
}

/// **Scenario 1.** The creation reply was lost and the replay entry has
/// been evicted; the recovering worker asks again. Zero new spawns: while
/// the session lives the key is `Known`, and after it has been reaped the
/// key is `KnownEnded`, and admission under it never yields a reservation.
#[test]
fn a_lost_reply_and_an_evicted_replay_entry_never_spawn_twice() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let key = IdempotencyKey::mint();
    let first = harness();
    let capture = Arc::clone(&first.capture);
    let handle = registry
        .admit_spawned(first.owner, Some(reserve(&registry, key)))
        .unwrap();

    // Live: the lookup and the admission both say the same session.
    assert!(matches!(
        registry.lookup_creation(key),
        CreationLookup::Known { session } if session.id == handle.id()
    ));
    assert!(matches!(
        registry.admit_creation(key, Default::default(), None),
        CreationAdmission::Known(known) if known.id() == handle.id()
    ));
    assert_eq!(registry.list().len(), 1, "nothing was spawned");

    // Ended and reaped: still not nothing.
    first.feeder.close();
    eventually(|| handle.is_ended());
    registry.reap();
    assert!(registry.list().is_empty());
    assert!(matches!(
        registry.lookup_creation(key),
        CreationLookup::KnownEnded { session } if session.id == handle.id()
    ));
    assert!(matches!(
        registry.admit_creation(key, Default::default(), None),
        CreationAdmission::Ended(session) if session.id == handle.id()
    ));
    assert!(
        registry.list().is_empty(),
        "nothing was spawned after the reap"
    );
    assert!(capture.0.lock().unwrap().terminated || handle.is_ended());
}

/// **Scenario 2.** Creation is paused after the process started and before
/// the registry insert. A lookup in that window reads `Creating`, a
/// replacement under the key is refused as in flight, and once the insert
/// lands the same key is `Known` — the process that existed all along.
#[test]
fn a_creation_paused_before_insertion_is_creating_and_admits_no_replacement() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let key = IdempotencyKey::mint();
    let reservation = reserve(&registry, key);
    // The process exists now; the map does not know it yet.
    let spawned = harness();
    assert!(registry.list().is_empty());

    assert!(matches!(
        registry.lookup_creation(key),
        CreationLookup::Creating
    ));
    assert!(matches!(
        registry.admit_creation(key, Default::default(), None),
        CreationAdmission::InFlight
    ));
    // Retiring in the window fences the future without touching the
    // creation in flight: it still completes, and is then known.
    assert!(matches!(
        registry.retire_creation(key),
        CreationLookup::Creating
    ));

    let handle = registry
        .admit_spawned(spawned.owner, Some(reservation))
        .unwrap();
    assert!(matches!(
        registry.lookup_creation(key),
        CreationLookup::Known { session } if session.id == handle.id()
    ));
    assert!(matches!(
        registry.admit_creation(key, Default::default(), None),
        CreationAdmission::Retired
    ));
    assert_eq!(registry.list().len(), 1);
}

/// **Scenario 3.** The original create is delayed until after the
/// reconciliation. The reconciliation finds nothing and retires the key;
/// when the late create arrives it is refused, so no second live provider
/// can exist. And the other order — the late create lands first — is the
/// case the retirement answer covers: it reports `Known`, and the caller
/// reattaches rather than spawning.
#[test]
fn a_create_delayed_past_reconciliation_cannot_start_a_second_provider() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let key = IdempotencyKey::mint();
    assert!(matches!(
        registry.lookup_creation(key),
        CreationLookup::NoEvidence
    ));
    assert!(matches!(
        registry.retire_creation(key),
        CreationLookup::NoEvidence
    ));
    // The late create.
    assert!(matches!(
        registry.admit_creation(key, Default::default(), None),
        CreationAdmission::Retired
    ));
    assert!(registry.list().is_empty());

    // The other order.
    let raced = IdempotencyKey::mint();
    let spawned = harness();
    let handle = registry
        .admit_spawned(spawned.owner, Some(reserve(&registry, raced)))
        .unwrap();
    assert!(matches!(
        registry.retire_creation(raced),
        CreationLookup::Known { session } if session.id == handle.id()
    ));
    assert!(matches!(
        registry.admit_creation(raced, Default::default(), None),
        CreationAdmission::Retired
    ));
    assert_eq!(
        registry.list().len(),
        1,
        "the one that ran is the one there is"
    );
}

/// A reservation whose spawn fails is abandoned by its drop, and leaves no
/// evidence: the next create under the key is admitted afresh.
#[test]
fn a_failed_spawn_abandons_its_reservation() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let key = IdempotencyKey::mint();
    {
        let _reservation = reserve(&registry, key);
        assert!(matches!(
            registry.lookup_creation(key),
            CreationLookup::Creating
        ));
    }
    assert!(matches!(
        registry.lookup_creation(key),
        CreationLookup::NoEvidence
    ));
    assert!(matches!(
        registry.admit_creation(key, Default::default(), None),
        CreationAdmission::Reserved(_)
    ));
}

/// The lookup never creates: however often it is asked, on a hit, a miss
/// or a reservation in flight, the catalogue is what it was.
#[test]
fn the_lookup_never_creates() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let miss = IdempotencyKey::mint();
    for _ in 0..3 {
        assert!(matches!(
            registry.lookup_creation(miss),
            CreationLookup::NoEvidence
        ));
    }
    assert!(registry.list().is_empty());

    let pending = IdempotencyKey::mint();
    let reservation = reserve(&registry, pending);
    for _ in 0..3 {
        assert!(matches!(
            registry.lookup_creation(pending),
            CreationLookup::Creating
        ));
    }
    assert!(registry.list().is_empty());
    drop(reservation);

    let hit = IdempotencyKey::mint();
    let spawned = harness();
    registry
        .admit_spawned(spawned.owner, Some(reserve(&registry, hit)))
        .unwrap();
    for _ in 0..3 {
        assert!(matches!(
            registry.lookup_creation(hit),
            CreationLookup::Known { .. }
        ));
    }
    assert_eq!(registry.list().len(), 1);
}
