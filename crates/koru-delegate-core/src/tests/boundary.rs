// SPDX-License-Identifier: MPL-2.0

//! The sync/execution split as a type boundary (D3, D14).
//!
//! `RunHeader` and `AttemptState` are distinct types with no `From` between
//! them — the two `compile_fail` doctests on `RunHeader` hold that — and
//! this test holds the other half: the two shapes share the run id and no
//! other field, so nothing can be written to both by name either.

use super::*;

fn keys<T: ::serde::Serialize>(value: &T) -> BTreeSet<String> {
    match serde_json::to_value(value).unwrap() {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        other => panic!("expected an object, got {other}"),
    }
}

#[test]
fn the_header_and_the_attempt_state_share_the_run_id_and_nothing_else() {
    let mut state = AttemptState::first(run_id(1));
    state.started_at_ms = Some(1);
    state.provider_session = Some(ProviderSessionKey {
        idempotency: v4(0x11).hyphenated().to_string().parse().unwrap(),
        session: Some(session_id(0x12)),
    });
    state.process = ProcessFacts {
        host_instance: Some(HostInstanceId::from_uuid(v4(0x13))),
        worker_pid: Some(4_242),
        observed_at_ms: Some(2),
    };
    state.pending_publications.push(PendingPublication {
        event: EventId(v4(0x14)),
        kind: PublicationKind::Usage,
        attempts: 1,
    });
    state.projection_cursor = Some(9);

    let shared: BTreeSet<_> = keys(&header())
        .intersection(&keys(&state))
        .cloned()
        .collect();
    assert_eq!(shared, BTreeSet::from(["run".to_owned()]));
}

#[test]
fn the_header_never_names_an_attempt_and_the_attempt_never_names_a_status() {
    let header = keys(&header());
    for execution_only in [
        "attempt",
        "generation",
        "provider_session",
        "process",
        "pending_publications",
        "projection_cursor",
    ] {
        assert!(!header.contains(execution_only), "{execution_only}");
    }
    let state = keys(&AttemptState::first(run_id(1)));
    for sync_only in [
        "status",
        "objective",
        "context",
        "grants",
        "artifacts",
        "economics",
        "rating",
        "environment",
    ] {
        assert!(!state.contains(sync_only), "{sync_only}");
    }
}

#[test]
fn a_successor_attempt_advances_the_generation_and_carries_no_facts_over() {
    let mut first = AttemptState::first(run_id(1));
    first.provider_session = Some(ProviderSessionKey {
        idempotency: v4(0x11).hyphenated().to_string().parse().unwrap(),
        session: None,
    });
    first.pending_publications.push(PendingPublication {
        event: EventId(v4(0x14)),
        kind: PublicationKind::Lifecycle,
        attempts: 3,
    });
    let second = first.successor();
    assert_eq!(second.run, first.run);
    assert_ne!(second.attempt, first.attempt);
    assert_eq!(second.generation, first.generation.next());
    assert_eq!(second.provider_session, None);
    assert!(second.pending_publications.is_empty());
    assert_eq!(second.process, ProcessFacts::default());
}

#[test]
fn the_generation_fence_refuses_older_and_newer_alike() {
    let current = AttemptGeneration::FIRST.next().next();
    assert_eq!(current.get(), 3);
    assert_eq!(current.admits(current), Ok(()));
    let stale = current.admits(AttemptGeneration::FIRST).unwrap_err();
    assert_eq!(
        stale,
        StaleAttempt {
            current,
            offered: AttemptGeneration::FIRST
        }
    );
    assert_eq!(
        stale.to_string(),
        "stale attempt: generation 1 offered, 3 current"
    );
    assert!(current.admits(current.next()).is_err());
    assert_eq!(
        AttemptGeneration::from_raw(u32::MAX).unwrap().next().get(),
        u32::MAX,
        "saturates rather than wrapping the fence inside out"
    );
    assert_eq!(AttemptGeneration::from_raw(0), None);
    assert_eq!(
        AttemptGeneration::from_raw(1),
        Some(AttemptGeneration::FIRST)
    );
}
