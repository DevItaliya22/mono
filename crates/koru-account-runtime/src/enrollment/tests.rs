// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::*;

mod harness;
use harness::{Harness, SharedPort};
mod parking;
mod scoped;

struct PauseClock {
    origin: Instant,
    elapsed: std::sync::atomic::AtomicU64,
}
impl Clock for PauseClock {
    fn now_millis(&self) -> Option<i64> {
        Some(1000 + self.elapsed.load(Ordering::Acquire) as i64)
    }
    fn monotonic(&self) -> Instant {
        self.origin + Duration::from_millis(self.elapsed.load(Ordering::Acquire))
    }
}

#[test]
fn background_keeps_target_and_backoff_and_allows_durable_cancel() {
    let state = Arc::new((Mutex::new(PressureState::default()), Condvar::new()));
    let clock = Arc::new(PauseClock {
        origin: Instant::now(),
        elapsed: std::sync::atomic::AtomicU64::new(0),
    });
    let control = Control::start_with_clock_and_foreground(
        QueuePressure {
            state: state.clone(),
        },
        clock.clone(),
        false,
        |_| {},
    );
    control.select(profile(), Credential(1));
    wait_until(|| state.0.lock().unwrap().persisted_targets == vec![Some(profile())]);
    wait_until(|| control.snapshot().target == Some(profile()));
    assert!(state.0.lock().unwrap().seen.is_empty());
    control.cancel_durable().unwrap();
    assert_eq!(control.snapshot().target, None);
    assert_eq!(
        state.0.lock().unwrap().persisted_targets,
        vec![Some(profile()), None]
    );
    control.select(profile(), Credential(1));
    control.set_foreground(true);
    wait_for_first_discovery(&state);
    control.set_foreground(false);
    release_discovery(&state);
    wait_until(|| control.report().retry_at_millis == Some(2000));
    control.set_foreground(true);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(state.0.lock().unwrap().seen, vec![profile()]);
    assert_eq!(control.report().retry_at_millis, Some(2000));
    control.set_foreground(false);
    clock.elapsed.store(1000, Ordering::Release);
    control.set_foreground(true);
    wait_until(|| control.report().retry_at_millis == Some(4000));
    assert_eq!(state.0.lock().unwrap().seen, vec![profile(), profile()]);
    assert_eq!(
        state.0.lock().unwrap().persisted_targets,
        vec![Some(profile()), None, Some(profile())]
    );
}

#[test]
fn replacing_a_paused_target_does_not_inherit_the_previous_targets_deadline() {
    let state = Arc::new((Mutex::new(PressureState::default()), Condvar::new()));
    let clock = Arc::new(PauseClock {
        origin: Instant::now(),
        elapsed: std::sync::atomic::AtomicU64::new(0),
    });
    let control = Control::start_with_clock_and_foreground(
        QueuePressure {
            state: state.clone(),
        },
        clock,
        true,
        |_| {},
    );
    let a = profile_with_suffix(1);
    let b = profile_with_suffix(2);
    control.select(a, Credential(1));
    wait_for_first_discovery(&state);
    release_discovery(&state);
    wait_until(|| control.report().retry_at_millis == Some(2000));
    control.set_foreground(false);
    control.select(b, Credential(2));
    wait_until(|| state.0.lock().unwrap().persisted_targets == vec![Some(a), Some(b)]);
    wait_until(|| control.snapshot().target == Some(b));
    control.set_foreground(true);
    wait_until(|| state.0.lock().unwrap().seen == vec![a, b]);
    control.set_foreground(false);
    control.cancel_durable().unwrap();
    control.set_foreground(true);
    assert_eq!(control.report().state, EnrollmentState::Idle);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Credential(u8);

#[derive(Default)]
struct Fake {
    cloud_primary: bool,
    reconcile: VecDeque<Result<Option<Completion<u8>>, Failure>>,
    discoveries: VecDeque<Result<Discovery<u8, ()>, Failure>>,
    enrollments: VecDeque<Result<Completion<u8>, Failure>>,
    provisions: VecDeque<Result<(), Failure>>,
    enrolled: Vec<u8>,
    refreshed_discoveries: usize,
    proof_expired: bool,
    discarded: Vec<ProfileId>,
    restored_target: Option<ProfileId>,
    persisted_targets: Vec<Option<ProfileId>>,
    target_persistence: VecDeque<Result<(), Failure>>,
}

impl Port for Fake {
    fn cloud_primary(&mut self, _: ProfileId, _: &Credential) -> Result<bool, Failure> {
        Ok(self.cloud_primary)
    }
    fn fork_effects(&self) -> Result<Self, Failure> {
        panic!("scripted Fake must be wrapped in SharedPort")
    }
    type Scope = u8;
    fn scope(_: &Credential) -> u8 {
        0
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
    fn authorize_profile(&mut self, _: ProfileId, _: &Credential) -> Result<bool, Failure> {
        Ok(true)
    }
    fn same_authority(previous: &Credential, next: &Credential) -> bool {
        previous == next
    }
    fn authority_successor(previous: &Credential, next: &Credential) -> bool {
        next.0 > previous.0
    }

    fn restored_targets(&mut self, _: &Credential) -> Result<BTreeSet<ProfileId>, Failure> {
        Ok(self.restored_target.into_iter().collect())
    }

    fn replace_targets(
        &mut self,
        _: &Credential,
        _: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        assert!(next.len() <= 1);
        let profile = next.first().copied();
        let result = self.target_persistence.pop_front().unwrap_or(Ok(()));
        if result.is_ok() {
            self.persisted_targets.push(profile);
            self.restored_target = profile;
        }
        result
    }
    fn reconcile(
        &mut self,
        _profile: ProfileId,
    ) -> Result<Option<Completion<Self::Completion>>, Failure> {
        self.reconcile.pop_front().unwrap_or(Ok(None))
    }

    fn discover(
        &mut self,
        _profile: ProfileId,
        _credential: &Credential,
        _cancel: &AtomicBool,
    ) -> Result<Discovery<u8, ()>, Failure> {
        self.discoveries.pop_front().unwrap_or(Ok(Discovery {
            candidates: Vec::new(),
            only_older_peers: false,
            attempt: (),
        }))
    }

    fn enroll(
        &mut self,
        _profile: ProfileId,
        candidate: &u8,
        _attempt: &(),
        _cancel: &AtomicBool,
    ) -> Result<Completion<u8>, Failure> {
        self.enrolled.push(*candidate);
        self.enrollments
            .pop_front()
            .unwrap_or(Err(Failure::Offline))
    }

    fn refresh_discovery(
        &mut self,
        profile: ProfileId,
        credential: &Credential,
        _: &(),
        cancel: &AtomicBool,
    ) -> Result<Discovery<u8, ()>, Failure> {
        self.refreshed_discoveries += 1;
        self.discover(profile, credential, cancel)
    }

    fn attempt_expired(&self, _: &()) -> bool {
        self.proof_expired
    }

    fn provision(
        &mut self,
        _profile: ProfileId,
        _completion: &u8,
        _credential: &Credential,
    ) -> Result<(), Failure> {
        self.provisions.pop_front().unwrap_or(Ok(()))
    }

    fn discard(&mut self, profile: ProfileId) {
        self.discarded.push(profile);
    }
}

fn profile() -> ProfileId {
    ProfileId::parse("30000000-0000-4000-8000-000000000003").unwrap()
}

fn selected(fake: Fake) -> Harness<Fake> {
    let mut coordinator = Harness::new(fake);
    coordinator.select(profile(), Credential(1));
    coordinator
}

#[test]
fn cloud_primary_pauses_restored_enrollment_and_disabling_resumes_it() {
    let mut harness = Harness::new(Fake {
        cloud_primary: true,
        restored_target: Some(profile()),
        discoveries: VecDeque::from([candidates(&[7], false)]),
        ..Default::default()
    });
    harness.activate(Some(Credential(1)));
    harness.step(0, &AtomicBool::new(false));
    assert_eq!(harness.report.state, EnrollmentState::Idle);
    assert_eq!(
        harness.snapshot().profiles[0].report.state,
        EnrollmentState::Idle
    );
    {
        let port = harness.port.lock().unwrap();
        assert_eq!(port.restored_target, Some(profile()));
        assert_eq!(port.discoveries.len(), 1, "no discovery or attestation");
        assert!(port.enrolled.is_empty());
        assert!(port.discarded.is_empty(), "existing trust is retained");
    }
    harness.port.lock().unwrap().cloud_primary = false;
    harness.step(60_000, &AtomicBool::new(false));
    assert_eq!(harness.port.lock().unwrap().enrolled, vec![7]);
}

#[test]
fn cloud_primary_finishes_durable_or_uncertain_peer_enrollment() {
    let mut committed = selected(Fake {
        cloud_primary: true,
        reconcile: VecDeque::from([Ok(Some(Completion {
            value: 5,
            durable_revision: 9,
        }))]),
        ..Default::default()
    });
    committed.step(0, &AtomicBool::new(false));
    assert_eq!(committed.report.state, EnrollmentState::Complete);
    assert_eq!(committed.port.lock().unwrap().restored_target, None);

    let mut uncertain = selected(Fake {
        discoveries: VecDeque::from([candidates(&[7], false)]),
        enrollments: VecDeque::from([
            Err(Failure::Uncertain),
            Ok(Completion {
                value: 5,
                durable_revision: 9,
            }),
        ]),
        ..Default::default()
    });
    uncertain.step(0, &AtomicBool::new(false));
    uncertain.port.lock().unwrap().cloud_primary = true;
    uncertain.step(1_000, &AtomicBool::new(false));
    assert_eq!(uncertain.port.lock().unwrap().enrolled, vec![7, 7]);
    assert_eq!(uncertain.report.state, EnrollmentState::Complete);
}

fn candidates(values: &[u8], older: bool) -> Result<Discovery<u8, ()>, Failure> {
    Ok(Discovery {
        candidates: values.to_vec(),
        only_older_peers: older,
        attempt: (),
    })
}

fn completion(value: u8, revision: u64) -> Result<Completion<u8>, Failure> {
    Ok(Completion {
        value,
        durable_revision: revision,
    })
}

#[test]
fn offline_waiting_is_a_state_not_repeated_failure() {
    let mut coordinator = selected(Fake::default());
    assert_eq!(
        coordinator.step(1_000, &AtomicBool::new(false)),
        Some(Duration::from_secs(1))
    );
    assert_eq!(coordinator.report().state, EnrollmentState::Waiting);
    assert_eq!(coordinator.report().failure_code, Some(NO_PEER_ONLINE));
    assert_eq!(coordinator.report().retry_at_millis, Some(2_000));
}

#[test]
fn roster_wake_resumes_and_first_success_wins() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(candidates(&[1, 2, 3], false));
    fake.enrollments.push_back(Err(Failure::Offline));
    fake.enrollments.push_back(completion(9, 41));
    let mut coordinator = selected(fake);
    assert_eq!(coordinator.step(0, &AtomicBool::new(false)), None);
    assert_eq!(coordinator.report().state, EnrollmentState::Complete);
    assert_eq!(coordinator.report().durable_revision, Some(41));
    assert_eq!(coordinator.port.lock().unwrap().enrolled, vec![1, 2]);
}

#[test]
fn offline_roster_retries_keep_proof_and_complete_when_a_peer_appears() {
    for initially_empty in [false, true] {
        let mut fake = Fake::default();
        fake.discoveries
            .push_back(candidates(if initially_empty { &[] } else { &[1] }, false));
        fake.discoveries.push_back(candidates(&[2], false));
        if !initially_empty {
            fake.enrollments.push_back(Err(Failure::Offline));
        }
        fake.enrollments.push_back(completion(9, 41));
        let mut coordinator = selected(fake);
        coordinator.step(0, &AtomicBool::new(false));
        assert_eq!(coordinator.report().failure_code, Some(NO_PEER_ONLINE));
        assert_eq!(
            coordinator.port.lock().unwrap().restored_target,
            Some(profile())
        );
        coordinator.step(1_000, &AtomicBool::new(false));
        assert_eq!(coordinator.report().state, EnrollmentState::Complete);
        let port = coordinator.port.lock().unwrap();
        assert_eq!(port.refreshed_discoveries, 1);
        assert_eq!(port.enrolled.last(), Some(&2));
        assert!(port.discarded.is_empty());
    }
}

#[test]
fn temporary_attestation_conflict_keeps_the_target_and_retries() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(Err(Failure::Unavailable));
    fake.discoveries.push_back(candidates(&[1], false));
    fake.enrollments.push_back(completion(9, 41));
    let mut coordinator = selected(fake);
    coordinator.step(0, &AtomicBool::new(false));
    assert_eq!(
        coordinator.port.lock().unwrap().restored_target,
        Some(profile())
    );
    coordinator.step(1_000, &AtomicBool::new(false));
    assert_eq!(coordinator.report().state, EnrollmentState::Complete);
    assert!(coordinator.port.lock().unwrap().discarded.is_empty());
}

#[test]
fn expired_proof_renews_before_a_new_candidate_but_retains_an_uncertain_join() {
    for uncertain in [false, true] {
        let mut fake = Fake::default();
        fake.discoveries.push_back(candidates(&[1, 2], false));
        fake.enrollments.push_back(Err(if uncertain {
            Failure::Uncertain
        } else {
            Failure::Offline
        }));
        let cancel = AtomicBool::new(false);
        let mut effects = member::Effects::default();
        // Reconcile, discover, then one unsuccessful candidate.
        for _ in 0..3 {
            effects = effects
                .step(&mut fake, profile(), &Credential(1), &cancel)
                .effects;
        }
        assert_eq!(fake.enrolled, vec![1]);
        fake.proof_expired = true;
        let result = effects.step(&mut fake, profile(), &Credential(1), &cancel);
        if uncertain {
            assert_eq!(
                fake.enrolled,
                vec![1, 1],
                "the possibly committed Join stays with its peer"
            );
        } else {
            assert_eq!(
                fake.enrolled,
                vec![1],
                "expired evidence never starts a new ceremony"
            );
            assert_eq!(result.failure, None);
            let _ = result
                .effects
                .step(&mut fake, profile(), &Credential(1), &cancel);
            assert_eq!(
                fake.refreshed_discoveries, 0,
                "expired evidence was discarded before fresh discovery"
            );
        }
    }
}

#[test]
fn only_older_peers_has_a_distinct_automatic_wait_code() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(candidates(&[], true));
    let mut coordinator = selected(fake);
    assert!(coordinator.step(0, &AtomicBool::new(false)).is_some());
    assert_eq!(coordinator.report().failure_code, Some(OLDER_PEER));
}

#[test]
fn duplicate_or_process_restart_reconciles_the_durable_result_first() {
    let mut fake = Fake::default();
    fake.reconcile.push_back(Ok(Some(Completion {
        value: 7,
        durable_revision: 88,
    })));
    let mut restarted = selected(fake);
    assert_eq!(restarted.step(0, &AtomicBool::new(false)), None);
    assert_eq!(restarted.report().state, EnrollmentState::Complete);
    assert!(restarted.port.lock().unwrap().enrolled.is_empty());
}

#[test]
fn session_expiry_preserves_target_until_fresh_authorization() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(Err(Failure::SessionExpired));
    fake.discoveries.push_back(candidates(&[1], false));
    fake.enrollments.push_back(completion(1, 2));
    let mut coordinator = selected(fake);
    assert_eq!(coordinator.step(0, &AtomicBool::new(false)), None);
    assert_eq!(coordinator.report().failure_code, Some(SESSION_EXPIRED));
    assert!(coordinator.update_credential(Credential(2)));
    coordinator.step(1, &AtomicBool::new(false));
    assert_eq!(coordinator.report().state, EnrollmentState::Complete);
}

#[test]
fn revoked_access_is_terminal_and_discards_only_pending_material() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(Err(Failure::AccessRevoked));
    let mut coordinator = selected(fake);
    coordinator.step(0, &AtomicBool::new(false));
    assert_eq!(coordinator.report().failure_code, Some(ACCESS_REVOKED));
    assert_eq!(coordinator.port.lock().unwrap().discarded, vec![profile()]);
    assert_eq!(coordinator.port.lock().unwrap().restored_target, None);
    assert!(!coordinator.retry());
}

#[test]
fn bad_attestation_requires_explicit_fresh_retry() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(Err(Failure::BadAttestation));
    let mut coordinator = selected(fake);
    coordinator.step(0, &AtomicBool::new(false));
    assert_eq!(coordinator.report().failure_code, Some(BAD_ATTESTATION));
    assert_eq!(coordinator.report().retry_at_millis, None);
    assert!(coordinator.retry());
}

#[test]
fn durable_commit_failure_never_reports_connected() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(candidates(&[1], false));
    fake.enrollments.push_back(Err(Failure::Persistence));
    let mut coordinator = selected(fake);
    coordinator.step(0, &AtomicBool::new(false));
    assert_eq!(coordinator.report().state, EnrollmentState::Failed);
    assert_eq!(coordinator.report().failure_code, Some(PERSISTENCE_FAILED));
}

#[test]
fn provisioning_retries_after_trust_is_already_durable() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(candidates(&[1], false));
    fake.enrollments.push_back(completion(1, 5));
    fake.provisions.push_back(Err(Failure::Offline));
    let mut coordinator = selected(fake);
    assert!(coordinator.step(10, &AtomicBool::new(false)).is_some());
    assert_eq!(coordinator.report().state, EnrollmentState::Complete);
    assert_eq!(
        coordinator.report().failure_code,
        Some(PROVISIONING_PENDING)
    );
    assert_eq!(coordinator.report().durable_revision, Some(5));
}

#[test]
fn cancellation_and_logout_do_not_remove_established_trust() {
    let mut coordinator = selected(Fake::default());
    coordinator.cancel().unwrap();
    assert_eq!(coordinator.report(), &Report::default());
    assert_eq!(coordinator.port.lock().unwrap().discarded, vec![profile()]);
    assert_eq!(coordinator.port.lock().unwrap().restored_target, None);
}

#[derive(Clone)]
struct Slow {
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl Port for Slow {
    fn fork_effects(&self) -> Result<Self, Failure> {
        Ok(self.clone())
    }
    type Scope = u8;
    fn scope(_: &Credential) -> u8 {
        0
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
    fn authorize_profile(&mut self, _: ProfileId, _: &Credential) -> Result<bool, Failure> {
        Ok(true)
    }
    fn same_authority(_: &Credential, _: &Credential) -> bool {
        true
    }

    fn restored_targets(&mut self, _: &Credential) -> Result<BTreeSet<ProfileId>, Failure> {
        Ok(BTreeSet::new())
    }
    fn replace_targets(
        &mut self,
        _: &Credential,
        _: &BTreeSet<ProfileId>,
        _: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        Ok(())
    }

    fn reconcile(&mut self, _profile: ProfileId) -> Result<Option<Completion<u8>>, Failure> {
        Ok(None)
    }

    fn discover(
        &mut self,
        _profile: ProfileId,
        _credential: &Credential,
        cancel: &AtomicBool,
    ) -> Result<Discovery<u8, ()>, Failure> {
        let (lock, changed) = &*self.gate;
        let mut open = lock.lock().unwrap();
        while !*open && !cancel.load(Ordering::Acquire) {
            let waited = changed
                .wait_timeout(open, Duration::from_millis(5))
                .unwrap();
            open = waited.0;
        }
        Err(Failure::Offline)
    }

    fn enroll(
        &mut self,
        _profile: ProfileId,
        _candidate: &u8,
        _attempt: &(),
        _cancel: &AtomicBool,
    ) -> Result<Completion<u8>, Failure> {
        unreachable!()
    }

    fn provision(
        &mut self,
        _profile: ProfileId,
        _completion: &u8,
        _credential: &Credential,
    ) -> Result<(), Failure> {
        Ok(())
    }

    fn discard(&mut self, _profile: ProfileId) {}
}

#[test]
fn cached_status_and_cancel_remain_responsive_while_network_is_slow() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let control = Control::start(
        Slow {
            gate: Arc::clone(&gate),
        },
        |_| {},
    );
    assert!(control.select(profile(), Credential(1)));
    std::thread::sleep(Duration::from_millis(20));
    let started = Instant::now();
    let _ = control.report();
    assert!(control.cancel_durable().is_ok());
    assert!(started.elapsed() < Duration::from_millis(20));
    gate.0.lock().map(|mut open| *open = true).unwrap();
    gate.1.notify_all();
}

#[test]
fn backoff_is_bounded_and_resets_on_explicit_retry() {
    let mut coordinator = selected(Fake::default());
    let cancel = AtomicBool::new(false);
    let mut last = Duration::ZERO;
    for attempt in 0..20 {
        last = coordinator.step(attempt * 100_000, &cancel).unwrap();
    }
    assert_eq!(last, MAX_BACKOFF);
    assert!(coordinator.retry());
    assert_eq!(coordinator.step(0, &cancel), Some(MIN_BACKOFF));
}

#[test]
fn hosted_discovery_outage_is_not_reported_as_no_peer_online() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(Err(Failure::Unavailable));
    let mut coordinator = selected(fake);
    assert_eq!(
        coordinator.step(0, &AtomicBool::new(false)),
        Some(MIN_BACKOFF)
    );
    assert_eq!(coordinator.report().state, EnrollmentState::Waiting);
    assert_eq!(
        coordinator.report().failure_code,
        Some(ENROLLMENT_UNAVAILABLE)
    );
}

#[test]
fn rate_limit_retains_intent_and_manual_retry_cannot_bypass_server_delay() {
    let mut fake = Fake::default();
    fake.discoveries
        .push_back(Err(Failure::RateLimited(Duration::from_secs(90))));
    fake.discoveries.push_back(candidates(&[1], false));
    let mut coordinator = selected(fake);
    let cancel = AtomicBool::new(false);
    assert_eq!(coordinator.step(0, &cancel), Some(Duration::from_secs(90)));
    assert_eq!(
        coordinator.report().failure_code,
        Some(ENROLLMENT_RATE_LIMITED)
    );
    assert!(!coordinator.retry());
    assert_eq!(
        coordinator.step(89_999, &cancel),
        Some(Duration::from_millis(1))
    );
    assert_eq!(coordinator.port.lock().unwrap().discoveries.len(), 1);
    coordinator.step(90_000, &cancel);
    assert!(coordinator.port.lock().unwrap().discoveries.is_empty());
}

#[test]
fn exhausted_candidate_dials_are_reported_as_no_peer_online() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(candidates(&[1, 2], false));
    fake.enrollments.push_back(Err(Failure::Offline));
    fake.enrollments.push_back(Err(Failure::Offline));
    let mut coordinator = selected(fake);
    assert_eq!(
        coordinator.step(0, &AtomicBool::new(false)),
        Some(MIN_BACKOFF)
    );
    assert_eq!(coordinator.port.lock().unwrap().enrolled, vec![1, 2]);
    assert_eq!(coordinator.report().failure_code, Some(NO_PEER_ONLINE));
}

#[test]
fn restored_session_resumes_the_exact_durable_profile_target() {
    let mut fake = Fake {
        restored_target: Some(profile()),
        ..Fake::default()
    };
    fake.reconcile.push_back(Ok(Some(Completion {
        value: 7,
        durable_revision: 91,
    })));
    let control = Control::start(SharedPort::new(fake), |_| {});
    assert!(control.update_authorization(Credential(2)));
    wait_until(|| control.report().state == EnrollmentState::Complete);
    assert_eq!(control.report().durable_revision, Some(91));
}

#[test]
fn worker_spawn_failure_degrades_to_typed_unavailable_state() {
    let control = Control::start_with(
        SharedPort::new(Fake::default()),
        |_| {},
        |_worker| Err(std::io::Error::other("forced spawn failure")),
    );
    assert_eq!(control.report().state, EnrollmentState::Failed);
    assert_eq!(control.report().failure_code, Some(ENROLLMENT_UNAVAILABLE));
    assert!(!control.select(profile(), Credential(1)));
}

#[test]
fn one_durable_revision_is_handed_off_once_across_provisioning_retries() {
    let mut fake = Fake::default();
    fake.discoveries.push_back(candidates(&[1], false));
    fake.enrollments.push_back(completion(1, 44));
    fake.provisions.push_back(Err(Failure::Offline));
    let events = Arc::new(Mutex::new(Vec::new()));
    let notices = events.clone();
    let clock = Arc::new(PauseClock {
        origin: Instant::now(),
        elapsed: std::sync::atomic::AtomicU64::new(0),
    });
    let control = Control::start_with_clock(SharedPort::new(fake), clock.clone(), move |event| {
        notices.lock().unwrap().push(event)
    });
    control.select(profile(), Credential(1));
    wait_until(|| control.report().failure_code == Some(PROVISIONING_PENDING));
    assert_eq!(control.report().durable_revision, Some(44));
    clock.elapsed.store(1000, Ordering::Release);
    control.wake();
    wait_until(|| {
        control.report().state == EnrollmentState::Complete
            && control.report().failure_code.is_none()
    });
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| event.durable_revision)
            .collect::<Vec<_>>(),
        vec![44]
    );
}

#[test]
fn report_notification_may_reenter_the_cached_report() {
    let holder: Arc<Mutex<Option<Arc<Control<SharedPort<Fake>>>>>> = Arc::default();
    let callback = holder.clone();
    let (send, receive) = mpsc::channel();
    let control = Arc::new(Control::start(
        SharedPort::new(Fake::default()),
        move |_| {
            if let Some(control) = callback.lock().unwrap().as_ref() {
                let _ = control.snapshot();
                let _ = send.send(());
            }
        },
    ));
    *holder.lock().unwrap() = Some(control.clone());
    control.select(profile(), Credential(1));
    receive.recv_timeout(Duration::from_secs(3)).unwrap();
    holder.lock().unwrap().take();
}

#[derive(Clone)]
struct SnapshotSwitch {
    completed: ProfileId,
    entered: mpsc::Sender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}
impl Port for SnapshotSwitch {
    fn fork_effects(&self) -> Result<Self, Failure> {
        Ok(self.clone())
    }
    type Scope = u8;
    fn scope(_: &Credential) -> u8 {
        0
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
    fn authorize_profile(&mut self, _: ProfileId, _: &Credential) -> Result<bool, Failure> {
        Ok(true)
    }
    fn same_authority(_: &Credential, _: &Credential) -> bool {
        true
    }
    fn restored_targets(&mut self, _: &Credential) -> Result<BTreeSet<ProfileId>, Failure> {
        Ok(BTreeSet::new())
    }
    fn replace_targets(
        &mut self,
        _: &Credential,
        _: &BTreeSet<ProfileId>,
        _: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        Ok(())
    }
    fn reconcile(&mut self, profile: ProfileId) -> Result<Option<Completion<u8>>, Failure> {
        Ok((profile == self.completed).then_some(Completion {
            value: 1,
            durable_revision: 1,
        }))
    }
    fn discover(
        &mut self,
        _: ProfileId,
        _: &Credential,
        _: &AtomicBool,
    ) -> Result<Discovery<u8, ()>, Failure> {
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        candidates(&[], false)
    }
    fn enroll(
        &mut self,
        _: ProfileId,
        _: &u8,
        _: &(),
        _: &AtomicBool,
    ) -> Result<Completion<u8>, Failure> {
        unreachable!()
    }
    fn provision(&mut self, _: ProfileId, _: &u8, _: &Credential) -> Result<(), Failure> {
        Ok(())
    }
    fn discard(&mut self, _: ProfileId) {}
}

#[test]
fn replacement_snapshot_cannot_label_previous_completion_with_the_new_target() {
    let a = profile_with_suffix(1);
    let b = profile_with_suffix(2);
    let (entered, started) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let control = Control::start(
        SnapshotSwitch {
            completed: a,
            entered,
            release: Arc::new(Mutex::new(blocked)),
        },
        |_| {},
    );
    control.select(a, Credential(1));
    wait_until(|| control.snapshot().report.state == EnrollmentState::Complete);
    assert_eq!(
        control.snapshot().target,
        Some(a),
        "completion retains report attribution after unfinished work is cleared"
    );
    control.select(b, Credential(2));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    let observed = control.snapshot();
    release.send(()).unwrap();
    assert_eq!(observed.target, Some(b));
    assert_eq!(observed.report.state, EnrollmentState::Enrolling);
    assert_eq!(observed.report.durable_revision, None);
    assert_eq!(observed.report.failure_code, None);
    assert_eq!(observed.report.retry_at_millis, None);
}

#[derive(Default)]
struct PressureState {
    entered: bool,
    release: bool,
    seen: Vec<ProfileId>,
    discarded: Vec<ProfileId>,
    persisted_targets: Vec<Option<ProfileId>>,
}

#[derive(Clone)]
struct QueuePressure {
    state: Arc<(Mutex<PressureState>, Condvar)>,
}

impl Port for QueuePressure {
    fn fork_effects(&self) -> Result<Self, Failure> {
        Ok(self.clone())
    }
    type Scope = u8;
    fn scope(_: &Credential) -> u8 {
        0
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
    fn authorize_profile(&mut self, _: ProfileId, _: &Credential) -> Result<bool, Failure> {
        Ok(true)
    }
    fn same_authority(_: &Credential, _: &Credential) -> bool {
        true
    }

    fn restored_targets(&mut self, _: &Credential) -> Result<BTreeSet<ProfileId>, Failure> {
        Ok(self
            .state
            .0
            .lock()
            .unwrap()
            .persisted_targets
            .last()
            .copied()
            .flatten()
            .into_iter()
            .collect())
    }
    fn replace_targets(
        &mut self,
        _: &Credential,
        _: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        assert!(next.len() <= 1);
        let profile = next.first().copied();
        self.state.0.lock().unwrap().persisted_targets.push(profile);
        Ok(())
    }
    fn reconcile(&mut self, _profile: ProfileId) -> Result<Option<Completion<u8>>, Failure> {
        Ok(None)
    }

    fn discover(
        &mut self,
        profile: ProfileId,
        _credential: &Credential,
        _cancel: &AtomicBool,
    ) -> Result<Discovery<u8, ()>, Failure> {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.seen.push(profile);
        let first = !state.entered;
        state.entered = true;
        changed.notify_all();
        while first && !state.release {
            state = changed.wait(state).unwrap();
        }
        Ok(Discovery {
            candidates: Vec::new(),
            only_older_peers: false,
            attempt: (),
        })
    }

    fn enroll(
        &mut self,
        _profile: ProfileId,
        _candidate: &u8,
        _attempt: &(),
        _cancel: &AtomicBool,
    ) -> Result<Completion<u8>, Failure> {
        unreachable!()
    }

    fn provision(
        &mut self,
        _profile: ProfileId,
        _completion: &u8,
        _credential: &Credential,
    ) -> Result<(), Failure> {
        Ok(())
    }

    fn discard(&mut self, profile: ProfileId) {
        self.state.0.lock().unwrap().discarded.push(profile);
    }
}

fn profile_with_suffix(suffix: u8) -> ProfileId {
    ProfileId::parse(&format!("30000000-0000-4000-8000-{suffix:012}")).unwrap()
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true");
        std::thread::yield_now();
    }
}

fn pressure_control() -> (Control<QueuePressure>, Arc<(Mutex<PressureState>, Condvar)>) {
    let state = Arc::new((Mutex::new(PressureState::default()), Condvar::new()));
    let control = Control::start(
        QueuePressure {
            state: Arc::clone(&state),
        },
        |_| {},
    );
    (control, state)
}

fn wait_for_first_discovery(state: &Arc<(Mutex<PressureState>, Condvar)>) {
    let (lock, changed) = &**state;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut guard = lock.lock().unwrap();
    while !guard.entered {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "discovery did not start");
        let waited = changed.wait_timeout(guard, remaining).unwrap();
        guard = waited.0;
    }
}

fn release_discovery(state: &Arc<(Mutex<PressureState>, Condvar)>) {
    let (lock, changed) = &**state;
    lock.lock().unwrap().release = true;
    changed.notify_all();
}

#[test]
fn queue_pressure_coalesces_to_the_latest_profile_selection() {
    let state = Arc::new((
        Mutex::new(PressureState {
            release: true,
            ..PressureState::default()
        }),
        Condvar::new(),
    ));
    let mut worker = None;
    let control = Control::start_with_clock_and_spawn(
        QueuePressure {
            state: state.clone(),
        },
        |_| {},
        Arc::new(SystemClock),
        true,
        |pending| {
            worker = Some(pending);
            Ok(())
        },
    );
    let first = profile_with_suffix(1);
    let second = profile_with_suffix(2);
    let latest = profile_with_suffix(3);
    assert!(control.select(first, Credential(1)));
    assert!(control.select(second, Credential(2)));
    assert!(
        control.select(latest, Credential(3)),
        "a full wake queue retains the latest owned intent"
    );
    let thread = std::thread::spawn(worker.take().unwrap());
    wait_until(|| state.0.lock().unwrap().seen.contains(&latest));
    assert_eq!(state.0.lock().unwrap().seen, vec![latest]);
    drop(control);
    thread.join().unwrap();
}

#[test]
fn queue_pressure_cannot_drop_explicit_cancellation() {
    let (control, state) = pressure_control();
    assert!(control.select(profile(), Credential(1)));
    wait_for_first_discovery(&state);
    assert!(control.wake());
    std::thread::scope(|scope| {
        let cancellation = scope.spawn(|| control.cancel_durable());
        wait_until(|| control.report() == Report::default());
        release_discovery(&state);
        assert!(
            cancellation.join().unwrap().is_ok(),
            "a full wake queue still accepts authoritative cancellation"
        );
    });
    wait_until(|| control.report() == Report::default());
    assert_eq!(state.0.lock().unwrap().discarded, vec![profile()]);
    assert_eq!(
        state.0.lock().unwrap().persisted_targets.last(),
        Some(&None)
    );
}

#[test]
fn failed_profile_replacement_preserves_prior_atomic_intent() {
    let first = profile_with_suffix(1);
    let second = profile_with_suffix(2);
    let mut coordinator = Harness::new(Fake::default());
    assert!(coordinator.select(first, Credential(1)));
    coordinator
        .port
        .lock()
        .unwrap()
        .target_persistence
        .push_back(Err(Failure::Persistence));
    assert!(!coordinator.select(second, Credential(1)));
    assert_eq!(coordinator.selection_error, Some(PERSISTENCE_FAILED));
    assert_eq!(coordinator.snapshot().target, Some(first));
    assert_eq!(
        coordinator.port.lock().unwrap().restored_target,
        Some(first)
    );
    assert!(coordinator.port.lock().unwrap().discarded.is_empty());
}

#[test]
fn durable_cancel_is_acknowledged_only_after_the_target_is_cleared() {
    let (control, state) = pressure_control();
    assert!(control.select(profile(), Credential(1)));
    wait_for_first_discovery(&state);
    release_discovery(&state);
    assert!(control.cancel_durable().is_ok());
    assert_eq!(
        state.0.lock().unwrap().persisted_targets.last(),
        Some(&None)
    );
    assert_eq!(control.report(), Report::default());
}

#[test]
fn failed_target_clear_is_not_reported_as_cancelled() {
    let mut coordinator = selected(Fake::default());
    coordinator
        .port
        .lock()
        .unwrap()
        .target_persistence
        .push_back(Err(Failure::Persistence));
    assert_eq!(coordinator.cancel(), Err(Failure::Persistence));
    assert_eq!(coordinator.snapshot().target, Some(profile()));
    assert_eq!(
        coordinator.port.lock().unwrap().restored_target,
        Some(profile())
    );
}

#[test]
fn logged_out_responder_refuses_new_enrollment() {
    let session = ResponderSessionGate::new();
    assert!(session.ticket(account(1)).is_err());
}

#[test]
fn logout_racing_an_in_flight_responder_prevents_its_trust_commit() {
    let session = ResponderSessionGate::new();
    session.enable_account(account(1), "session-a".into());
    let in_flight_epoch = session.ticket(account(1)).unwrap();
    session.disable();
    let committed = AtomicBool::new(false);

    assert!(
        session
            .commit(in_flight_epoch, || {
                committed.store(true, Ordering::Release);
                Ok(())
            })
            .is_err()
    );
    assert!(!committed.load(Ordering::Acquire));
    assert!(session.ticket(account(1)).is_err());
}

fn account(value: u8) -> koru_account_core::KoruIdentityId {
    format!("10000000-0000-4000-8000-{value:012}")
        .parse()
        .unwrap()
}

#[test]
fn responder_logout_invalidates_only_that_account_and_readding_never_reuses_ticket() {
    let gate = ResponderSessionGate::new();
    gate.enable_account(account(1), "session-a".into());
    gate.enable_account(account(2), "session-b".into());
    let a = gate.ticket(account(1)).unwrap();
    let b = gate.ticket(account(2)).unwrap();
    gate.disable_account_and_wait(account(1));
    assert!(
        gate.ticket(account(1)).is_err(),
        "B cannot admit A's verified attestation"
    );
    assert!(gate.commit(a, || Ok(())).is_err());
    assert!(gate.commit(b, || Ok(())).is_ok());
    gate.enable_account(account(1), "session-a".into());
    assert!(
        gate.ensure(a).is_err(),
        "same session re-added is a new incarnation"
    );
    assert!(gate.ensure(b).is_ok());
}

#[test]
fn responder_refresh_keeps_ticket_but_replacement_session_invalidates_only_its_account() {
    let gate = ResponderSessionGate::new();
    gate.enable_account(account(1), "session-a".into());
    gate.enable_account(account(2), "session-b".into());
    let a = gate.ticket(account(1)).unwrap();
    let b = gate.ticket(account(2)).unwrap();
    gate.enable_account(account(1), "session-a".into());
    assert!(gate.ensure(a).is_ok());
    gate.enable_account(account(1), "replacement-a".into());
    assert!(gate.ensure(a).is_err());
    assert!(gate.ensure(b).is_ok());
    let replacement = gate.ticket(account(1)).unwrap();
    gate.disable_account_session_and_wait(account(1), "session-a");
    assert!(
        gate.ensure(replacement).is_ok(),
        "late cleanup cannot disable the replacement session"
    );
    gate.disable_account_session(account(1), "replacement-a");
    assert!(gate.ensure(replacement).is_err());
    assert!(gate.ensure(b).is_ok());
}
