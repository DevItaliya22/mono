// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Owned {
    account: u8,
    session: u8,
    version: u64,
}
fn owned(account: u8) -> Owned {
    Owned {
        account,
        session: account,
        version: 1,
    }
}

#[derive(Default)]
struct Effects {
    rows: BTreeMap<u8, (u64, BTreeSet<ProfileId>)>,
    trace: Vec<(&'static str, u8)>,
    blocked: Option<&'static str>,
    entered: bool,
    released: bool,
    candidates: bool,
    fail_rebase: bool,
    notices: usize,
}
type Shared = Arc<(Mutex<Effects>, Condvar)>;
#[derive(Clone)]
struct OwnedPort(Shared);
impl Port for OwnedPort {
    fn fork_effects(&self) -> Result<Self, Failure> {
        Ok(self.clone())
    }
    fn authority_successor(a: &Owned, b: &Owned) -> bool {
        a.account == b.account && b.version > a.version
    }
    fn refresh_continuity(a: &Owned, b: &Owned) -> bool {
        a.account == b.account && a.session == b.session && b.version > a.version
    }
    type Scope = u8;
    type Credential = Owned;
    type Candidate = u8;
    type Attempt = u8;
    type Completion = u8;
    fn scope(value: &Owned) -> u8 {
        value.account
    }
    fn same_authority(a: &Owned, b: &Owned) -> bool {
        a == b
    }
    fn restored_targets(&mut self, credential: &Owned) -> Result<BTreeSet<ProfileId>, Failure> {
        let state = self.0.0.lock().unwrap();
        let (version, targets) = state
            .rows
            .get(&credential.account)
            .ok_or(Failure::Persistence)?;
        if *version != credential.version {
            return Err(Failure::Persistence);
        }
        Ok(targets.clone())
    }
    fn refresh_authority(
        &mut self,
        previous: &Owned,
        next: &Owned,
        expected: &BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        let mut state = self.0.0.lock().unwrap();
        state.trace.push(("rebase", previous.account));
        if std::mem::take(&mut state.fail_rebase) {
            return Err(Failure::Persistence);
        }
        if previous.account != next.account
            || previous.session != next.session
            || previous.version >= next.version
            || state.rows.get(&next.account) != Some(&(next.version, expected.clone()))
        {
            return Err(Failure::Persistence);
        }
        Ok(BTreeSet::new())
    }
    fn replace_targets(
        &mut self,
        credential: &Owned,
        expected: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        let mut state = self.0.0.lock().unwrap();
        if state.rows.get(&credential.account) != Some(&(credential.version, expected.clone())) {
            return Err(Failure::Persistence);
        }
        state
            .rows
            .insert(credential.account, (credential.version, next.clone()));
        state.trace.push(("persist", credential.account));
        self.0.1.notify_all();
        Ok(())
    }
    fn authorize_profile(&mut self, _: ProfileId, _: &Owned) -> Result<bool, Failure> {
        Ok(true)
    }
    fn reconcile(&mut self, _: ProfileId) -> Result<Option<Completion<u8>>, Failure> {
        Ok(None)
    }
    fn discover(
        &mut self,
        profile: ProfileId,
        credential: &Owned,
        _: &AtomicBool,
    ) -> Result<Discovery<u8, u8>, Failure> {
        self.effect("discover", credential.account);
        Ok(Discovery {
            candidates: if self.0.0.lock().unwrap().candidates {
                vec![credential.account]
            } else {
                vec![]
            },
            only_older_peers: false,
            attempt: if profile == profile_with_suffix(1) {
                1
            } else {
                2
            },
        })
    }
    fn enroll(
        &mut self,
        _: ProfileId,
        candidate: &u8,
        attempt: &u8,
        _: &AtomicBool,
    ) -> Result<Completion<u8>, Failure> {
        assert_eq!(
            candidate, attempt,
            "retained ceremony belongs to its original account"
        );
        self.effect("enroll", *candidate);
        Ok(Completion {
            value: *candidate,
            durable_revision: u64::from(*candidate) + 10,
        })
    }
    fn provision(
        &mut self,
        _: ProfileId,
        completion: &u8,
        credential: &Owned,
    ) -> Result<(), Failure> {
        assert_eq!(*completion, credential.account);
        self.effect("provision", credential.account);
        Ok(())
    }
    fn discard(&mut self, _: ProfileId) {
        self.0.0.lock().unwrap().trace.push(("discard", 0));
    }
}
impl OwnedPort {
    fn effect(&self, name: &'static str, account: u8) {
        let mut state = self.0.0.lock().unwrap();
        state.trace.push((name, account));
        if account == 1 && state.blocked == Some(name) {
            state.entered = true;
            self.0.1.notify_all();
            while !state.released {
                state = self.0.1.wait(state).unwrap();
            }
            state.blocked = None;
        }
        self.0.1.notify_all();
    }
}
fn shared() -> Shared {
    Arc::new((
        Mutex::new(Effects {
            rows: BTreeMap::from([
                (1, (1, BTreeSet::from([profile_with_suffix(1)]))),
                (2, (1, BTreeSet::from([profile_with_suffix(2)]))),
            ]),
            ..Default::default()
        }),
        Condvar::new(),
    ))
}
fn until(shared: &Shared, check: impl Fn(&Effects) -> bool) {
    let mut state = shared.0.lock().unwrap();
    while !check(&state) {
        let (next, timeout) = shared
            .1
            .wait_timeout(state, Duration::from_secs(3))
            .unwrap();
        state = next;
        assert!(
            !timeout.timed_out() || check(&state),
            "controlled effect did not complete: {:?}",
            state.trace
        );
    }
}

#[test]
fn parked_target_keeps_report_failure_count_and_original_deadline() {
    let state = shared();
    let mut coordinator = Harness::new(OwnedPort(state.clone()));
    assert!(coordinator.activate(Some(owned(1))));
    assert_eq!(
        coordinator.step(1000, &AtomicBool::new(false)),
        Some(Duration::from_secs(1))
    );
    let report = coordinator.report.clone();
    coordinator.activate(Some(owned(2)));
    coordinator.activate(Some(owned(1)));
    assert_eq!(coordinator.report, report);
    assert_eq!(coordinator.report.retry_at_millis, Some(2000));
    assert_eq!(
        coordinator.step(2000, &AtomicBool::new(false)),
        Some(Duration::from_secs(2))
    );
    assert!(
        !state
            .0
            .lock()
            .unwrap()
            .trace
            .iter()
            .any(|(effect, _)| *effect == "discard")
    );
}

#[test]
fn switching_is_visible_before_held_discovery_or_commit_returns_and_resumes_exact_progress() {
    for blocked in ["discover", "enroll", "provision"] {
        let state = shared();
        {
            let mut value = state.0.lock().unwrap();
            value.blocked = Some(blocked);
            value.candidates = true;
        }
        let notices = state.clone();
        let control = Control::start(OwnedPort(state.clone()), move |_| {
            notices.0.lock().unwrap().notices += 1;
            notices.1.notify_all();
        });
        control.activate(Some(owned(1)));
        until(&state, |state| state.entered);
        assert_eq!(control.snapshot().target, Some(profile_with_suffix(1)));
        control.set_foreground(false);
        control.activate(Some(owned(2)));
        assert_ne!(
            control.snapshot().target,
            Some(profile_with_suffix(1)),
            "A's held result cannot block B's visible selection"
        );
        until(&state, |_| {
            control.snapshot().target == Some(profile_with_suffix(2))
        });
        // B's actual restored projection arrives before A is released.
        {
            let mut value = state.0.lock().unwrap();
            value.released = true;
            state.1.notify_all();
        }
        until(&state, |_| {
            control.snapshot().target == Some(profile_with_suffix(2))
        });
        assert!(
            !state.0.lock().unwrap().trace.contains(&("provision", 1)) || blocked == "provision"
        );
        control.activate(Some(owned(1)));
        control.set_foreground(true);
        until(&state, |state| {
            state.rows[&1].1.is_empty()
                && control.report().state == EnrollmentState::Complete
                && control.report().failure_code.is_none()
        });
        let trace = state.0.lock().unwrap().trace.clone();
        for effect in ["discover", "enroll", "provision"] {
            assert_eq!(
                trace.iter().filter(|entry| **entry == (effect, 1)).count(),
                1,
                "{blocked}: {trace:?}"
            );
        }
        assert_eq!(control.snapshot().target, Some(profile_with_suffix(1)));
        assert_eq!(
            state.0.lock().unwrap().rows[&2].1,
            BTreeSet::from([profile_with_suffix(2)])
        );
    }
}

#[test]
fn acknowledged_refresh_rebases_only_exact_held_predecessor_and_preserves_parked_progress() {
    let state = shared();
    let mut coordinator = Harness::new(OwnedPort(state.clone()));
    coordinator.activate(Some(owned(1)));
    coordinator.step(1000, &AtomicBool::new(false));
    let report = coordinator.report.clone();
    coordinator.activate(Some(owned(2)));
    let next = Owned {
        version: 2,
        ..owned(1)
    };
    state.0.lock().unwrap().rows.get_mut(&1).unwrap().0 = 2;
    assert!(coordinator.refresh_authorization(owned(1), next.clone()));
    assert_eq!(coordinator.credential, Some(owned(2)));
    assert_eq!(coordinator.snapshot_for(&owned(1)).report, report);
    coordinator.activate(Some(next.clone()));
    assert_eq!(coordinator.report.retry_at_millis, Some(2000));
    assert_eq!(coordinator.authority_for(&next), Some(next.clone()));
    // Equal owner/targets after removal and recreation cannot make an older
    // predecessor current, nor can a replacement session impersonate refresh.
    state.0.lock().unwrap().rows.get_mut(&1).unwrap().0 = 4;
    assert!(!coordinator.refresh_authorization(
        owned(1),
        Owned {
            version: 4,
            ..owned(1)
        }
    ));
    assert!(!coordinator.refresh_authorization(
        next.clone(),
        Owned {
            session: 9,
            version: 4,
            ..owned(1)
        }
    ));
    assert_eq!(coordinator.authority_for(&next), Some(next.clone()));
}

#[test]
fn a_failed_refresh_rebase_is_retained_for_the_same_worker_retry() {
    let state = shared();
    let mut coordinator = Harness::new(OwnedPort(state.clone()));
    coordinator.activate(Some(owned(1)));
    let next = Owned {
        version: 2,
        ..owned(1)
    };
    {
        let mut value = state.0.lock().unwrap();
        value.rows.get_mut(&1).unwrap().0 = 2;
        value.fail_rebase = true;
    }
    assert!(!coordinator.refresh_authorization(owned(1), next.clone()));
    assert_eq!(coordinator.authority_for(&next), Some(owned(1)));
    coordinator.step(1000, &AtomicBool::new(false));
    assert_eq!(coordinator.authority_for(&next), Some(next.clone()));
    assert_eq!(
        state
            .0
            .lock()
            .unwrap()
            .trace
            .iter()
            .filter(|entry| **entry == ("rebase", 1))
            .count(),
        2
    );
}

#[test]
fn late_inactive_selection_and_scoped_cancel_never_replace_selected_account() {
    let state = shared();
    let mut coordinator = Harness::new(OwnedPort(state.clone()));
    coordinator.activate(Some(owned(1)));
    coordinator.activate(Some(owned(2)));
    assert!(coordinator.select_parked(profile_with_suffix(3), owned(1)));
    assert_eq!(coordinator.snapshot().target, Some(profile_with_suffix(2)));
    assert_eq!(
        state.0.lock().unwrap().rows[&1].1,
        BTreeSet::from([profile_with_suffix(3)])
    );
    coordinator.cancel_for(&owned(1)).unwrap();
    assert_eq!(coordinator.snapshot().target, Some(profile_with_suffix(2)));
    assert!(state.0.lock().unwrap().rows[&1].1.is_empty());
    assert_eq!(
        state.0.lock().unwrap().rows[&2].1,
        BTreeSet::from([profile_with_suffix(2)])
    );
    let replacement = Owned {
        version: 2,
        session: 9,
        ..owned(1)
    };
    state
        .0
        .lock()
        .unwrap()
        .rows
        .insert(1, (2, BTreeSet::from([profile_with_suffix(1)])));
    coordinator.activate(Some(replacement));
    coordinator.activate(Some(owned(2)));
    assert!(!coordinator.select_parked(profile_with_suffix(3), owned(1)));
    assert_eq!(coordinator.cancel_for(&owned(1)), Err(Failure::Persistence));
    assert_eq!(
        state.0.lock().unwrap().rows[&1].1,
        BTreeSet::from([profile_with_suffix(1)])
    );
    assert_eq!(coordinator.snapshot().target, Some(profile_with_suffix(2)));
}
