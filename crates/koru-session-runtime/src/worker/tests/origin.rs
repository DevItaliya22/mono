// SPDX-License-Identifier: MPL-2.0

//! Session origin ownership (protocol v16), at the registry.
//!
//! Each test asks the question the AI worker asks before creating a chat's
//! session — may I, and is there one already? — and proves the answer is
//! decided under the admission lock, from what is live or reserved, never
//! from what the evictable creation history remembers.

use super::*;
use crate::CreationAdmission;
use koru_session_protocol::{ChatOriginId, IdempotencyKey, SessionOrigin};

fn chat_origin() -> SessionOrigin {
    SessionOrigin::Chat {
        chat_id: ChatOriginId::mint(),
    }
}

/// Only a registered `ai_control` connection may create for a chat, and
/// only while its generation is current: a successor's registration makes
/// every earlier worker's create stale wherever it was waiting.
#[test]
fn a_chat_creation_needs_a_current_generation() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let origin = chat_origin();

    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), origin, None),
        CreationAdmission::Forbidden
    ));

    let first = registry.register_ai_control();
    let held = match registry.admit_creation(IdempotencyKey::mint(), origin, Some(first)) {
        CreationAdmission::Reserved(reservation) => reservation,
        _ => panic!("the current generation is admitted"),
    };
    assert_eq!(registry.lookup_origin(origin).creating, 1);

    let second = registry.register_ai_control();
    assert!(second > first);
    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), origin, Some(first)),
        CreationAdmission::StaleAuthority
    ));
    // The successor is current, but the old worker's reservation still
    // holds the chat until it lands or is abandoned.
    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), origin, Some(second)),
        CreationAdmission::OriginOwned
    ));
    drop(held);
    assert!(registry.lookup_origin(origin).is_empty());
    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), origin, Some(second)),
        CreationAdmission::Reserved(_)
    ));
}

/// One conversation per chat: a second key for a chat with a live session
/// is refused under the lock that reserved the first; the same key replays;
/// and the chat is free again only once the session has ended and been
/// reaped.
#[test]
fn one_conversation_per_chat() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let origin = chat_origin();
    let generation = registry.register_ai_control();
    let key = IdempotencyKey::mint();
    let session = harness();
    let reservation = match registry.admit_creation(key, origin, Some(generation)) {
        CreationAdmission::Reserved(reservation) => reservation,
        _ => panic!("a fresh chat is admitted"),
    };
    let handle = registry
        .admit_spawned(session.owner, Some(reservation))
        .unwrap();

    let lookup = registry.lookup_origin(origin);
    assert_eq!(lookup.creating, 0);
    assert_eq!(lookup.live.len(), 1);
    assert_eq!(lookup.live[0].id, handle.id());

    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), origin, Some(generation)),
        CreationAdmission::OriginOwned
    ));
    assert!(matches!(
        registry.admit_creation(key, origin, Some(generation)),
        CreationAdmission::Known(known) if known.id() == handle.id()
    ));
    // Another chat is another conversation; it is not owned by this one.
    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), chat_origin(), Some(generation)),
        CreationAdmission::Reserved(_)
    ));

    session.feeder.close();
    eventually(|| handle.is_ended());
    registry.reap();
    assert!(registry.lookup_origin(origin).is_empty());
    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), origin, Some(generation)),
        CreationAdmission::Reserved(_)
    ));
}

/// Manual and delegated origins carry no ownership: two keys, no authority,
/// both admitted, exactly as every creation before v16.
#[test]
fn manual_origins_are_never_owned() {
    let registry = SessionRegistry::new(SessionRuntimeConfig::default());
    let first = match registry.admit_creation(IdempotencyKey::mint(), SessionOrigin::Manual, None) {
        CreationAdmission::Reserved(reservation) => reservation,
        _ => panic!("manual creations are unowned"),
    };
    assert!(matches!(
        registry.admit_creation(IdempotencyKey::mint(), SessionOrigin::Delegated, None),
        CreationAdmission::Reserved(_)
    ));
    assert!(registry.lookup_origin(SessionOrigin::Manual).is_empty());
    drop(first);
}
