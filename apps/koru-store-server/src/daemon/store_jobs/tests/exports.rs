// SPDX-License-Identifier: MPL-2.0

use super::*;
use loro::VersionVector;

fn request(f: &Fixture, budget: &MemoryBudget) -> ExportRequest {
    ExportRequest::new(f.scope, f.unit, VersionVector::new(), budget.clone()).unwrap()
}

#[test]
fn queued_local_export_retains_capacity_after_unpolled_reply_is_dropped() {
    let mut f = Fixture::new();
    let (client, mut receiver) = queue(1);
    let budget = MemoryBudget::new(transfer::MAX_PAYLOAD);
    let called = Arc::new(AtomicBool::new(false));
    let observed = called.clone();
    let reply = client
        .submit_cloud_export(
            request(&f, &budget),
            Timestamp::from_millis(2),
            move |_, _, _| {
                observed.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(receiver.len(), 1);
    drop(reply);
    assert_eq!(budget.used(), transfer::MAX_PAYLOAD);
    assert!(!called.load(Ordering::SeqCst));
    execute(&mut receiver, &mut f.store);
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(budget.used(), 0);
    assert_eq!(f.store.open_workspaces(), 0);
}

#[test]
fn queued_local_export_holds_its_reservation_in_the_reply_and_upload_guard() {
    let mut f = Fixture::new();
    let (client, mut receiver) = queue(1);
    let budget = MemoryBudget::new(transfer::MAX_PAYLOAD);
    let mut reply = Box::pin(
        client
            .submit_cloud_export(
                request(&f, &budget),
                Timestamp::from_millis(2),
                |_, _, _| Ok(()),
            )
            .unwrap(),
    );
    // Work queued earlier must capture the Store state at execution, not stale
    // bytes from queue admission.
    f.store
        .apply_profile_unit_updates(
            ProfileUnitRef::Identity { profile: f.profile },
            &[f.bytes.clone()],
        )
        .unwrap();
    execute(&mut receiver, &mut f.store);
    assert_eq!(budget.used(), transfer::MAX_PAYLOAD);
    let Poll::Ready(Ok(Ok(Some(export)))) = poll(reply.as_mut()) else {
        panic!("export reply")
    };
    assert_eq!(export.unit(), f.unit);
    let (bytes, guard) = export.into_payload();
    let restored = IdentityManifest::from_snapshot(&bytes).unwrap();
    assert_eq!(restored.profile().unwrap().name, "After");
    assert!(budget.used() >= bytes.capacity());
    drop(bytes);
    assert!(budget.used() > 0);
    drop(guard);
    assert_eq!(budget.used(), 0);
}

#[test]
fn queued_local_export_checks_authority_at_execution_not_submission() {
    let mut f = Fixture::new();
    let (client, mut receiver) = queue(1);
    let budget = MemoryBudget::new(transfer::MAX_PAYLOAD);
    let allowed = Arc::new(AtomicBool::new(true));
    let observed = allowed.clone();
    let mut reply = Box::pin(
        client
            .submit_cloud_export(
                request(&f, &budget),
                Timestamp::from_millis(2),
                move |_, _, _| {
                    if observed.load(Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err(Refusal::CloudDisabled)
                    }
                },
            )
            .unwrap(),
    );
    allowed.store(false, Ordering::SeqCst);
    execute(&mut receiver, &mut f.store);
    assert!(matches!(
        poll(reply.as_mut()),
        Poll::Ready(Ok(Err(Failure::Transfer(transfer::Error::Authority(
            Refusal::CloudDisabled
        )))))
    ));
    assert_eq!(budget.used(), 0);
    assert_eq!(f.store.open_workspaces(), 0);
}

#[test]
fn rejected_or_stopped_export_queue_releases_only_its_own_reservation() {
    let f = Fixture::new();
    let (client, receiver) = queue(1);
    let budget = MemoryBudget::new(2 * transfer::MAX_PAYLOAD);
    let first = client
        .submit_cloud_export(
            request(&f, &budget),
            Timestamp::from_millis(2),
            |_, _, _| panic!("not executed"),
        )
        .unwrap();
    assert!(
        client
            .submit_cloud_export(
                request(&f, &budget),
                Timestamp::from_millis(2),
                |_, _, _| panic!("not admitted"),
            )
            .is_err()
    );
    assert_eq!(budget.used(), transfer::MAX_PAYLOAD);
    drop(first);
    drop(receiver);
    assert_eq!(budget.used(), 0);
    assert!(
        client
            .submit_cloud_export(
                request(&f, &budget),
                Timestamp::from_millis(2),
                |_, _, _| panic!("stopped"),
            )
            .is_err()
    );
    assert_eq!(budget.used(), 0);
}
