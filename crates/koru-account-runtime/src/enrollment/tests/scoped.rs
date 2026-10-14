// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Scoped {
    effects: Fake,
    targets: BTreeMap<u8, BTreeSet<ProfileId>>,
    grants: BTreeMap<u8, BTreeSet<ProfileId>>,
    refuse_write: bool,
    completed: BTreeMap<ProfileId, u64>,
}

impl Port for Scoped {
    fn fork_effects(&self) -> Result<Self, Failure> {
        panic!("scripted Scoped must be wrapped in SharedPort")
    }
    type Scope = u8;
    fn scope(value: &Credential) -> u8 {
        value.0
    }
    fn refresh_authority(
        &mut self,
        previous: &Credential,
        next: &Credential,
        _: &BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        if previous == next {
            Ok(BTreeSet::new())
        } else {
            Err(Failure::Persistence)
        }
    }
    type Credential = Credential;
    type Candidate = u8;
    type Attempt = ();
    type Completion = u8;

    fn restored_targets(
        &mut self,
        credential: &Credential,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        Ok(self.targets.get(&credential.0).cloned().unwrap_or_default())
    }
    fn replace_targets(
        &mut self,
        credential: &Credential,
        expected: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        if self.refuse_write
            || self.targets.get(&credential.0).cloned().unwrap_or_default() != *expected
        {
            return Err(Failure::Persistence);
        }
        self.targets.insert(credential.0, next.clone());
        Ok(())
    }
    fn authorize_profile(
        &mut self,
        profile: ProfileId,
        credential: &Credential,
    ) -> Result<bool, Failure> {
        Ok(self
            .grants
            .get(&credential.0)
            .is_some_and(|profiles| profiles.contains(&profile)))
    }
    fn same_authority(previous: &Credential, next: &Credential) -> bool {
        previous == next
    }
    fn reconcile(&mut self, profile: ProfileId) -> Result<Option<Completion<u8>>, Failure> {
        if let Some(revision) = self.completed.get(&profile) {
            return Ok(Some(Completion {
                value: 1,
                durable_revision: *revision,
            }));
        }
        self.effects.reconcile(profile)
    }
    fn discover(
        &mut self,
        profile: ProfileId,
        credential: &Credential,
        cancel: &AtomicBool,
    ) -> Result<Discovery<u8, ()>, Failure> {
        self.effects.discover(profile, credential, cancel)
    }
    fn enroll(
        &mut self,
        profile: ProfileId,
        candidate: &u8,
        attempt: &(),
        cancel: &AtomicBool,
    ) -> Result<Completion<u8>, Failure> {
        self.effects.enroll(profile, candidate, attempt, cancel)
    }
    fn provision(
        &mut self,
        profile: ProfileId,
        completion: &u8,
        credential: &Credential,
    ) -> Result<(), Failure> {
        self.effects.provision(profile, completion, credential)
    }
    fn discard(&mut self, profile: ProfileId) {
        self.effects.discard(profile);
    }
}

fn id(value: u8) -> ProfileId {
    ProfileId::parse(&format!("30000000-0000-4000-8000-{value:012}")).unwrap()
}
fn seeded() -> Scoped {
    let targets = BTreeMap::from([
        (1, BTreeSet::from([id(1), id(2)])),
        (2, BTreeSet::from([id(3)])),
    ]);
    Scoped {
        grants: targets.clone(),
        targets,
        ..Default::default()
    }
}

#[test]
fn completion_removes_one_member_and_whole_cancel_never_erases_other_accounts() {
    for complete in [false, true] {
        let mut port = seeded();
        port.completed.insert(id(1), 9);
        let mut coordinator = Harness::new(port);
        assert!(coordinator.update_credential(Credential(1)));
        assert_eq!(coordinator.snapshot().profiles.len(), 2);
        if complete {
            coordinator.step(0, &AtomicBool::new(false));
        } else {
            coordinator.cancel().unwrap();
        }
        assert_eq!(
            coordinator.port.lock().unwrap().targets[&1],
            if complete {
                BTreeSet::from([id(2)])
            } else {
                BTreeSet::new()
            }
        );
        assert_eq!(
            coordinator.port.lock().unwrap().targets[&2],
            BTreeSet::from([id(3)])
        );
    }
}

#[test]
fn a_new_authority_parks_the_previous_account_set_and_restores_only_its_own_grants() {
    let mut coordinator = Harness::new(seeded());
    assert!(coordinator.update_credential(Credential(1)));
    assert!(coordinator.update_credential(Credential(2)));
    assert_eq!(coordinator.snapshot().target, Some(id(3)));
    coordinator.cancel().unwrap();
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(1), id(2)])
    );
    assert!(coordinator.port.lock().unwrap().targets[&2].is_empty());
    coordinator
        .port
        .lock()
        .unwrap()
        .grants
        .insert(1, BTreeSet::from([id(2)]));
    // New authenticated restoration revalidates current grants; merely
    // switching to a held account preserves its established authority.
    let saved = coordinator.port.clone();
    drop(coordinator);
    let mut coordinator = Harness::from_shared(saved);
    assert!(coordinator.update_credential(Credential(1)));
    assert_eq!(
        coordinator
            .snapshot()
            .profiles
            .iter()
            .find(|row| row.profile == id(2))
            .unwrap()
            .report
            .state,
        EnrollmentState::Enrolling
    );
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(2)])
    );
    assert_eq!(
        coordinator
            .snapshot()
            .profiles
            .iter()
            .find(|row| row.profile == id(1))
            .unwrap()
            .report
            .failure_code,
        Some(ACCESS_REVOKED)
    );
}

#[test]
fn single_selection_replaces_its_account_set_and_a_failed_cas_retains_newer_intent() {
    let mut coordinator = Harness::new(seeded());
    coordinator
        .port
        .lock()
        .unwrap()
        .grants
        .get_mut(&1)
        .unwrap()
        .insert(id(4));
    assert!(coordinator.update_credential(Credential(1)));
    assert!(coordinator.select(id(4), Credential(1)));
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(4)])
    );
    coordinator
        .port
        .lock()
        .unwrap()
        .targets
        .get_mut(&1)
        .unwrap()
        .insert(id(5));
    assert_eq!(coordinator.cancel(), Err(Failure::Persistence));
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(4), id(5)])
    );
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&2],
        BTreeSet::from([id(3)])
    );
    assert_eq!(coordinator.snapshot().target, Some(id(4)));
}

#[test]
fn selection_before_restoration_replaces_only_the_authenticated_account_set() {
    let mut coordinator = Harness::new(seeded());
    coordinator
        .port
        .lock()
        .unwrap()
        .grants
        .get_mut(&1)
        .unwrap()
        .insert(id(4));
    assert!(coordinator.select(id(4), Credential(1)));
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(4)])
    );
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&2],
        BTreeSet::from([id(3)])
    );
}

#[test]
fn interrupted_attempt_retains_ownership_until_durable_cancellation() {
    let mut coordinator = Harness::new(seeded());
    assert!(coordinator.update_credential(Credential(1)));
    assert_eq!(coordinator.step(0, &AtomicBool::new(true)), None);
    assert_eq!(coordinator.snapshot().profiles.len(), 2);
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(1), id(2)])
    );
    coordinator.cancel().unwrap();
    assert!(coordinator.port.lock().unwrap().targets[&1].is_empty());
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&2],
        BTreeSet::from([id(3)])
    );
}

#[test]
fn unauthorized_selection_and_failed_completion_write_leave_all_intent_discoverable() {
    let mut coordinator = Harness::new(seeded());
    assert!(!coordinator.select(id(3), Credential(1)));
    assert_eq!(coordinator.selection_error, Some(ACCESS_REVOKED));
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(1), id(2)])
    );
    coordinator.update_credential(Credential(1));
    coordinator.port.lock().unwrap().completed.insert(id(1), 9);
    coordinator.port.lock().unwrap().refuse_write = true;
    assert_eq!(
        coordinator.step(0, &AtomicBool::new(false)),
        Some(MIN_BACKOFF)
    );
    let snapshot = coordinator.snapshot();
    let member = snapshot
        .profiles
        .iter()
        .find(|row| row.profile == id(1))
        .unwrap();
    assert_eq!(member.report.state, EnrollmentState::Complete);
    assert_eq!(member.report.failure_code, Some(PERSISTENCE_FAILED));
    assert_eq!(
        coordinator.port.lock().unwrap().targets[&1],
        BTreeSet::from([id(1), id(2)])
    );
}

#[test]
fn background_startup_waits_for_authorization_before_reading_a_target() {
    let control = Control::start_with_clock_and_foreground(
        SharedPort::new(seeded()),
        Arc::new(SystemClock),
        false,
        |_| {},
    );
    assert!(control.update_authorization(Credential(2)));
    wait_until(|| control.snapshot().target == Some(id(3)));
    assert_eq!(control.report().state, EnrollmentState::Enrolling);
    control.cancel_durable().unwrap();
}

#[test]
fn queued_authorization_never_reattributes_an_existing_selection() {
    let mut port = seeded();
    // Both accounts can name this Profile in the fixture, so the assertion
    // proves attribution rather than relying on a denied grant to stop a write.
    port.grants.get_mut(&2).unwrap().insert(id(1));
    let mut worker = None;
    let control = Control::start_with_clock_and_spawn(
        SharedPort::new(port),
        |_| {},
        Arc::new(SystemClock),
        false,
        |pending| {
            worker = Some(pending);
            Ok(())
        },
    );
    assert!(control.select(id(1), Credential(1)));
    assert!(control.update_authorization(Credential(2)));
    let thread = std::thread::spawn(worker.take().unwrap());
    wait_until(|| control.snapshot().target == Some(id(3)));
    assert_eq!(control.report().state, EnrollmentState::Enrolling);
    control.cancel_durable().unwrap();
    // Account one's original selection remains parked under its original owner.
    assert!(control.update_authorization(Credential(1)));
    wait_until(|| control.snapshot().target == Some(id(1)));
    drop(control);
    thread.join().unwrap();
}
