// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::sync::{Condvar, atomic::AtomicU64};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Credential(u8, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Operation {
    Load(u8),
    Discover(ProfileId),
    Enroll(ProfileId),
    Refresh(u8),
}

struct Gate {
    entered: AtomicBool,
    released: Mutex<bool>,
    ready: Condvar,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: AtomicBool::new(false),
            released: Mutex::new(false),
            ready: Condvar::new(),
        })
    }
    fn wait(&self) {
        self.entered.store(true, Ordering::Release);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.ready.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.ready.notify_all();
    }
}

#[derive(Default)]
struct Database {
    revisions: BTreeMap<u8, u64>,
    targets: BTreeMap<u8, BTreeSet<ProfileId>>,
    grants: BTreeMap<u8, BTreeSet<ProfileId>>,
    gates: BTreeMap<Operation, Arc<Gate>>,
    discoveries: BTreeMap<ProfileId, usize>,
    enrollments: Vec<(ProfileId, u8, u64)>,
    replies: BTreeMap<ProfileId, VecDeque<Result<(), Failure>>>,
    empty: BTreeSet<ProfileId>,
    completions: BTreeMap<ProfileId, u64>,
    provisions: Vec<ProfileId>,
    provision_effects: BTreeSet<(ProfileId, u64)>,
    fail_write: bool,
    fail_refresh: bool,
    attempt: u64,
    writes: usize,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Database>>);

impl Fake {
    fn block(&self, operation: Operation) {
        let gate = self.0.lock().unwrap().gates.remove(&operation);
        if let Some(gate) = gate {
            gate.wait();
        }
    }
    fn gate(&self, operation: Operation) -> Arc<Gate> {
        let gate = Gate::new();
        self.0.lock().unwrap().gates.insert(operation, gate.clone());
        gate
    }
}

impl Port for Fake {
    type Scope = u8;
    type Credential = Credential;
    type Candidate = u8;
    type Attempt = u64;
    type Completion = u64;
    fn fork_effects(&self) -> Result<Self, Failure> {
        Ok(self.clone())
    }
    fn scope(credential: &Credential) -> u8 {
        credential.0
    }
    fn snapshot_account_id(credential: &Credential) -> Option<String> {
        Some(credential.0.to_string())
    }
    fn same_authority(a: &Credential, b: &Credential) -> bool {
        a == b
    }
    fn authority_successor(a: &Credential, b: &Credential) -> bool {
        a.0 == b.0 && b.1 > a.1
    }
    fn refresh_continuity(a: &Credential, b: &Credential) -> bool {
        Self::authority_successor(a, b)
    }
    fn refresh_authority(
        &mut self,
        previous: &Credential,
        next: &Credential,
        expected: &BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        assert_eq!(previous.0, next.0);
        self.block(Operation::Refresh(next.0));
        let mut db = self.0.lock().unwrap();
        if std::mem::take(&mut db.fail_refresh) {
            return Err(Failure::Unavailable);
        }
        if db.revisions.get(&next.0) != Some(&next.1) || db.targets.get(&next.0) != Some(expected) {
            return Err(Failure::Persistence);
        }
        Ok(expected
            .difference(db.grants.get(&next.0).unwrap())
            .copied()
            .collect())
    }
    fn restored_targets(
        &mut self,
        credential: &Credential,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        let stored = {
            let db = self.0.lock().unwrap();
            if db.revisions.get(&credential.0) != Some(&credential.1) {
                return Err(Failure::Persistence);
            }
            db.targets.get(&credential.0).cloned().unwrap_or_default()
        };
        self.block(Operation::Load(credential.0));
        Ok(stored)
    }
    fn replace_targets(
        &mut self,
        credential: &Credential,
        expected: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        let mut db = self.0.lock().unwrap();
        if std::mem::take(&mut db.fail_write) {
            return Err(Failure::Persistence);
        }
        if !db.revisions.contains_key(&credential.0) && next.is_empty() {
            return Ok(());
        }
        if db.revisions.get(&credential.0) != Some(&credential.1)
            || db.targets.get(&credential.0).cloned().unwrap_or_default() != *expected
        {
            return Err(Failure::Persistence);
        }
        db.targets.insert(credential.0, next.clone());
        db.writes += 1;
        Ok(())
    }
    fn authorize_profile(
        &mut self,
        profile: ProfileId,
        credential: &Credential,
    ) -> Result<bool, Failure> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .grants
            .get(&credential.0)
            .is_some_and(|grants| grants.contains(&profile)))
    }
    fn reconcile(&mut self, profile: ProfileId) -> Result<Option<Completion<u64>>, Failure> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .completions
            .get(&profile)
            .map(|revision| Completion {
                value: *revision,
                durable_revision: *revision,
            }))
    }
    fn discover(
        &mut self,
        profile: ProfileId,
        _: &Credential,
        _: &AtomicBool,
    ) -> Result<Discovery<u8, u64>, Failure> {
        self.block(Operation::Discover(profile));
        let mut db = self.0.lock().unwrap();
        *db.discoveries.entry(profile).or_default() += 1;
        db.attempt += 1;
        Ok(Discovery {
            candidates: if db.empty.contains(&profile) {
                vec![]
            } else {
                vec![1, 2]
            },
            only_older_peers: false,
            attempt: db.attempt,
        })
    }
    fn enroll(
        &mut self,
        profile: ProfileId,
        candidate: &u8,
        attempt: &u64,
        _: &AtomicBool,
    ) -> Result<Completion<u64>, Failure> {
        self.0
            .lock()
            .unwrap()
            .enrollments
            .push((profile, *candidate, *attempt));
        self.block(Operation::Enroll(profile));
        let mut db = self.0.lock().unwrap();
        db.replies
            .entry(profile)
            .or_default()
            .pop_front()
            .unwrap_or(Ok(()))?;
        let revision = db.completions.len() as u64 + 1;
        db.completions.insert(profile, revision);
        Ok(Completion {
            value: revision,
            durable_revision: revision,
        })
    }
    fn provision(
        &mut self,
        profile: ProfileId,
        completion: &u64,
        _: &Credential,
    ) -> Result<(), Failure> {
        let mut db = self.0.lock().unwrap();
        db.provisions.push(profile);
        db.provision_effects.insert((profile, *completion));
        Ok(())
    }
    fn discard(&mut self, _: ProfileId) {}
}

struct Time {
    epoch: Instant,
    millis: AtomicU64,
}
impl Clock for Time {
    fn now_millis(&self) -> Option<i64> {
        Some(self.millis.load(Ordering::Acquire) as i64)
    }
    fn monotonic(&self) -> Instant {
        self.epoch + Duration::from_millis(self.millis.load(Ordering::Acquire))
    }
}

struct Harness {
    scheduler: Scheduler<Fake>,
    fake: Fake,
    time: Arc<Time>,
    wake: mpsc::Receiver<()>,
    gates: Vec<Arc<Gate>>,
}

fn profile(value: u8) -> ProfileId {
    ProfileId::parse(&format!("10000000-0000-4000-8000-{value:012x}")).unwrap()
}

impl Harness {
    fn new() -> Self {
        let fake = Fake::default();
        for account in [1, 2] {
            let mut db = fake.0.lock().unwrap();
            db.revisions.insert(account, 1);
            db.targets.insert(account, BTreeSet::new());
            db.grants.insert(account, (1..=8).map(profile).collect());
        }
        let (send, wake) = mpsc::sync_channel(1);
        let scheduler = Scheduler::new(fake.clone(), send).unwrap();
        Self {
            scheduler,
            fake,
            time: Arc::new(Time {
                epoch: Instant::now(),
                millis: AtomicU64::new(0),
            }),
            wake,
            gates: Vec::new(),
        }
    }
    fn gate(&mut self, operation: Operation) -> Arc<Gate> {
        let gate = self.fake.gate(operation);
        self.gates.push(gate.clone());
        gate
    }
    fn pump(&mut self, foreground: bool) {
        self.scheduler.drain(self.time.as_ref());
        self.scheduler.dispatch(self.time.as_ref(), foreground);
    }
    fn until(&mut self, foreground: bool, predicate: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump(foreground);
            if predicate(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "scheduler fixture did not reach its explicit predicate"
            );
            // A held effect can signal its injected barrier before returning a
            // scheduler event; the short timeout only observes that predicate.
            let _ = self.wake.recv_timeout(Duration::from_millis(10));
        }
    }
    fn idle(&mut self, foreground: bool) {
        self.until(foreground, |h| {
            h.scheduler.idle_for_test(h.time.as_ref(), foreground)
        });
    }
    fn activate(&mut self, account: u8) {
        self.scheduler.activate(Some(Credential(account, 1)));
        self.idle(false);
    }
    fn select(
        &mut self,
        account: u8,
        profiles: &[u8],
    ) -> mpsc::Receiver<Result<Snapshot, &'static str>> {
        let (reply, result) = mpsc::sync_channel(1);
        self.scheduler.select(
            profiles.iter().copied().map(profile).collect(),
            Credential(account, 1),
            reply,
        );
        result
    }
    fn advance(&self, millis: u64) {
        self.time.millis.fetch_add(millis, Ordering::AcqRel);
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        for gate in &self.gates {
            gate.release();
        }
    }
}

#[test]
fn slow_first_profile_does_not_hold_reachable_sibling() {
    let mut h = Harness::new();
    h.activate(1);
    let gate = h.gate(Operation::Enroll(profile(1)));
    let selected = h.select(1, &[1, 2]);
    h.idle(false);
    assert_eq!(selected.recv().unwrap().unwrap().profiles.len(), 2);
    h.until(true, |h| {
        gate.entered.load(Ordering::Acquire)
            && h.scheduler.snapshot_for(&1).profiles.iter().any(|row| {
                row.profile == profile(2) && row.report.state == EnrollmentState::Complete
            })
            && !h.fake.0.lock().unwrap().targets[&1].contains(&profile(2))
    });
    assert!(!*gate.released.lock().unwrap());
    assert_eq!(
        h.fake.0.lock().unwrap().targets[&1],
        BTreeSet::from([profile(1)])
    );
    gate.release();
    h.idle(true);
    assert!(h.fake.0.lock().unwrap().targets[&1].is_empty());
}

#[test]
fn unchanged_member_keeps_deadline_and_proof_when_set_is_replaced() {
    let mut h = Harness::new();
    h.activate(1);
    h.fake.0.lock().unwrap().empty.insert(profile(1));
    let selected = h.select(1, &[1]);
    h.idle(true);
    selected.recv().unwrap().unwrap();
    let before = h.scheduler.snapshot_for(&1).profiles[0].report.clone();
    let selected = h.select(1, &[1, 2]);
    h.idle(true);
    selected.recv().unwrap().unwrap();
    let after = h
        .scheduler
        .snapshot_for(&1)
        .profiles
        .into_iter()
        .find(|row| row.profile == profile(1))
        .unwrap()
        .report;
    assert_eq!(before, after);
    assert_eq!(h.fake.0.lock().unwrap().discoveries[&profile(1)], 1);
}

#[test]
fn revoked_completed_trust_waits_for_explicit_reselection_then_uses_a_fresh_attempt() {
    let mut h = Harness::new();
    h.activate(1);
    let selected = h.select(1, &[1]);
    h.idle(true);
    selected.recv().unwrap().unwrap();
    let first_attempt = h.fake.0.lock().unwrap().enrollments[0].2;

    // Unlink makes the old completion ineffective. Merely waking the scheduler
    // cannot revive it because completed targets are no longer runnable.
    h.fake.0.lock().unwrap().completions.remove(&profile(1));
    h.scheduler.dispatch(h.time.as_ref(), true);
    h.idle(true);
    assert_eq!(h.fake.0.lock().unwrap().enrollments.len(), 1);

    // Relink is the existing explicit Profile selection. With no peer online it
    // remains durable waiting intent; a later peer wake completes a new ceremony.
    h.fake.0.lock().unwrap().empty.insert(profile(1));
    let relinked = h.select(1, &[1]);
    h.idle(true);
    relinked.recv().unwrap().unwrap();
    assert_eq!(
        h.scheduler.snapshot_for(&1).profiles[0].report.failure_code,
        Some(NO_PEER_ONLINE)
    );
    assert_eq!(
        h.fake.0.lock().unwrap().targets[&1],
        BTreeSet::from([profile(1)])
    );
    h.fake.0.lock().unwrap().empty.remove(&profile(1));
    h.advance(1_000);
    h.idle(true);

    let db = h.fake.0.lock().unwrap();
    assert!(db.targets[&1].is_empty());
    assert_eq!(db.enrollments.len(), 2);
    assert_ne!(db.enrollments[1].2, first_attempt);
}

#[test]
fn offline_relink_waits_beyond_ten_minutes_and_restores_without_reselection() {
    let mut h = Harness::new();
    h.activate(1);
    h.fake.0.lock().unwrap().empty.insert(profile(1));
    let selected = h.select(1, &[1]);
    h.idle(true);
    selected.recv().unwrap().unwrap();

    // Advance the injected clock, keeping real backoff and admission intact.
    // No clock-driven UI timeout or second select is needed to retain intent.
    for minute in 1..=11 {
        h.advance(60_000);
        h.idle(true);
        assert_eq!(
            h.scheduler.snapshot_for(&1).profiles[0].report.state,
            EnrollmentState::Waiting
        );
        assert_eq!(
            h.fake.0.lock().unwrap().targets[&1],
            BTreeSet::from([profile(1)])
        );
        if minute == 5 {
            // A new scheduler reads only the persisted target set, as after
            // process recreation; no UI-side origin or timer is restored.
            let (send, wake) = mpsc::sync_channel(1);
            h.scheduler = Scheduler::new(h.fake.clone(), send).unwrap();
            h.wake = wake;
            h.activate(1);
            h.idle(true);
            assert_eq!(
                h.scheduler.snapshot_for(&1).profiles[0].report.state,
                EnrollmentState::Waiting
            );
        }
    }
    assert!(h.fake.0.lock().unwrap().enrollments.is_empty());
    h.fake.0.lock().unwrap().empty.remove(&profile(1));
    h.advance(60_000);
    h.idle(true);
    assert_eq!(
        h.scheduler.snapshot_for(&1).profiles[0].report.state,
        EnrollmentState::Complete
    );
    assert!(h.fake.0.lock().unwrap().targets[&1].is_empty());
    assert_eq!(h.fake.0.lock().unwrap().enrollments.len(), 1);
}

#[test]
fn invalid_or_failed_set_replacement_preserves_every_existing_member() {
    let mut h = Harness::new();
    h.activate(1);
    let selected = h.select(1, &[1, 2]);
    h.idle(false);
    selected.recv().unwrap().unwrap();
    h.fake
        .0
        .lock()
        .unwrap()
        .grants
        .get_mut(&1)
        .unwrap()
        .remove(&profile(3));
    let refused = h.select(1, &[1, 3]);
    h.idle(false);
    assert_eq!(refused.recv().unwrap(), Err(ACCESS_REVOKED));
    h.fake.0.lock().unwrap().fail_write = true;
    let failed = h.select(1, &[2]);
    h.idle(false);
    assert_eq!(failed.recv().unwrap(), Err(PERSISTENCE_FAILED));
    assert_eq!(
        h.fake.0.lock().unwrap().targets[&1],
        BTreeSet::from([profile(1), profile(2)])
    );
    assert_eq!(h.scheduler.snapshot_for(&1).profiles.len(), 2);
}

#[test]
fn held_selection_load_cannot_recreate_cancelled_intent() {
    let mut h = Harness::new();
    h.activate(1);
    let gate = h.gate(Operation::Load(1));
    let selected = h.select(1, &[1]);
    h.until(false, |_| gate.entered.load(Ordering::Acquire));
    h.scheduler.cancel(&Credential(1, 1)).unwrap();
    gate.release();
    h.idle(false);
    assert_eq!(selected.recv().unwrap(), Err(PERSISTENCE_FAILED));
    assert!(h.fake.0.lock().unwrap().targets[&1].is_empty());
    assert!(h.scheduler.snapshot_for(&1).profiles.is_empty());
}

#[test]
fn lost_complete_then_offline_retains_exact_candidate_and_attempt() {
    let mut h = Harness::new();
    h.activate(1);
    h.fake.0.lock().unwrap().replies.insert(
        profile(1),
        VecDeque::from([Err(Failure::Uncertain), Err(Failure::Offline), Ok(())]),
    );
    let selected = h.select(1, &[1, 2]);
    h.idle(true);
    selected.recv().unwrap().unwrap();
    h.advance(1_000);
    h.idle(true);
    h.advance(2_000);
    h.idle(true);
    let db = h.fake.0.lock().unwrap();
    let attempts = db
        .enrollments
        .iter()
        .filter(|(id, _, _)| *id == profile(1))
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 3);
    assert!(attempts.iter().all(|attempt| *attempt == attempts[0]));
    assert_eq!(db.discoveries[&profile(1)], 1);
    assert!(db.targets[&1].is_empty());
}

#[test]
fn late_removed_member_commit_reports_revision_without_set_bookkeeping() {
    let mut h = Harness::new();
    h.activate(1);
    let gate = h.gate(Operation::Enroll(profile(1)));
    let selected = h.select(1, &[1]);
    h.idle(false);
    selected.recv().unwrap().unwrap();
    h.until(true, |_| gate.entered.load(Ordering::Acquire));
    h.scheduler.cancel(&Credential(1, 1)).unwrap();
    gate.release();
    h.idle(true);
    assert_eq!(h.scheduler.take_revision(), Some(1));
    assert!(h.scheduler.snapshot_for(&1).profiles.is_empty());
    let db = h.fake.0.lock().unwrap();
    assert!(db.targets[&1].is_empty());
    assert!(db.completions.contains_key(&profile(1)));
    assert!(db.provisions.is_empty());
}

#[test]
fn refresh_waits_to_remove_completed_members_and_retries_without_resetting_proof() {
    let mut h = Harness::new();
    h.activate(1);
    let enrollment = h.gate(Operation::Enroll(profile(1)));
    let selected = h.select(1, &[1]);
    h.idle(false);
    selected.recv().unwrap().unwrap();
    h.until(true, |_| enrollment.entered.load(Ordering::Acquire));
    h.fake.0.lock().unwrap().revisions.insert(1, 2);
    h.fake.0.lock().unwrap().fail_refresh = true;
    let refresh = h.gate(Operation::Refresh(1));
    h.scheduler.refresh(Credential(1, 1), Credential(1, 2));
    h.until(true, |_| refresh.entered.load(Ordering::Acquire));
    enrollment.release();
    h.until(true, |h| {
        h.scheduler.snapshot_for(&1).profiles[0].report.state == EnrollmentState::Complete
    });
    assert_eq!(
        h.fake.0.lock().unwrap().targets[&1],
        BTreeSet::from([profile(1)])
    );
    refresh.release();
    h.idle(true);
    h.scheduler.retry_for(&Credential(1, 2), None);
    h.idle(true);
    let db = h.fake.0.lock().unwrap();
    assert!(db.targets[&1].is_empty());
    assert_eq!(db.discoveries[&profile(1)], 1);
    assert_eq!(db.enrollments.len(), 1);
}

#[test]
fn revoked_restored_member_does_not_block_authorized_sibling() {
    let mut h = Harness::new();
    h.fake
        .0
        .lock()
        .unwrap()
        .targets
        .insert(1, BTreeSet::from([profile(1), profile(2)]));
    h.fake
        .0
        .lock()
        .unwrap()
        .grants
        .get_mut(&1)
        .unwrap()
        .remove(&profile(1));
    h.activate(1);
    h.idle(true);
    let rows = h.scheduler.snapshot_for(&1).profiles;
    assert_eq!(rows[0].report.failure_code, Some(ACCESS_REVOKED));
    assert_eq!(rows[1].report.state, EnrollmentState::Complete);
    assert!(
        !h.fake
            .0
            .lock()
            .unwrap()
            .discoveries
            .contains_key(&profile(1))
    );
}

#[test]
fn captured_retry_and_whole_account_cancel_preserve_other_account() {
    let mut h = Harness::new();
    h.activate(1);
    h.fake
        .0
        .lock()
        .unwrap()
        .empty
        .extend([profile(1), profile(2)]);
    let selected = h.select(1, &[1, 2]);
    h.idle(true);
    selected.recv().unwrap().unwrap();
    h.activate(2);
    let selected = h.select(2, &[3, 4]);
    h.idle(false);
    selected.recv().unwrap().unwrap();
    let before = h.scheduler.snapshot_for(&2);
    h.scheduler.retry_for(&Credential(1, 1), Some(profile(1)));
    h.scheduler.cancel(&Credential(1, 1)).unwrap();
    assert_eq!(h.scheduler.snapshot_for(&2), before);
    assert_eq!(
        h.fake.0.lock().unwrap().targets[&2],
        BTreeSet::from([profile(3), profile(4)])
    );
    assert!(h.fake.0.lock().unwrap().targets[&1].is_empty());
}

#[test]
fn all_is_a_captured_set_and_does_not_subscribe_to_later_grants() {
    let mut h = Harness::new();
    h.activate(1);
    let all = h.fake.0.lock().unwrap().grants[&1]
        .iter()
        .copied()
        .collect();
    let (reply, accepted) = mpsc::sync_channel(1);
    h.scheduler.select(all, Credential(1, 1), reply);
    h.idle(false);
    assert_eq!(accepted.recv().unwrap().unwrap().profiles.len(), 8);
    h.fake
        .0
        .lock()
        .unwrap()
        .grants
        .get_mut(&1)
        .unwrap()
        .insert(profile(9));
    h.idle(true);
    let db = h.fake.0.lock().unwrap();
    assert_eq!(db.provision_effects.len(), 8);
    assert!(!db.discoveries.contains_key(&profile(9)));
    assert!(db.targets[&1].is_empty());
}

#[test]
fn restart_after_provision_ack_reconciles_trust_and_replays_same_provision_effect() {
    let mut h = Harness::new();
    h.activate(1);
    let selected = h.select(1, &[1]);
    h.idle(false);
    selected.recv().unwrap().unwrap();
    h.until(true, |h| !h.fake.0.lock().unwrap().provisions.is_empty());
    assert_eq!(
        h.fake.0.lock().unwrap().targets[&1],
        BTreeSet::from([profile(1)])
    );
    let (wake, receive) = mpsc::sync_channel(1);
    h.scheduler = Scheduler::new(h.fake.clone(), wake).unwrap();
    h.wake = receive;
    h.activate(1);
    h.idle(true);
    let db = h.fake.0.lock().unwrap();
    assert_eq!(db.enrollments.len(), 1);
    assert_eq!(db.provisions, vec![profile(1), profile(1)]);
    assert_eq!(db.provision_effects.len(), 1);
    assert!(db.targets[&1].is_empty());
}

#[test]
fn removed_account_cleanup_is_idempotent_for_all_siblings_and_keeps_completed_trust() {
    let mut h = Harness::new();
    h.activate(1);
    let selected = h.select(1, &[1, 2]);
    h.idle(false);
    selected.recv().unwrap().unwrap();
    h.fake
        .0
        .lock()
        .unwrap()
        .targets
        .insert(2, BTreeSet::from([profile(3)]));
    h.fake.0.lock().unwrap().completions.insert(profile(4), 88);
    h.fake.0.lock().unwrap().revisions.remove(&1);
    h.fake.0.lock().unwrap().targets.remove(&1);
    h.scheduler.cancel(&Credential(1, 1)).unwrap();
    h.scheduler.cancel(&Credential(1, 1)).unwrap();
    let db = h.fake.0.lock().unwrap();
    assert_eq!(db.targets[&2], BTreeSet::from([profile(3)]));
    assert_eq!(db.completions[&profile(4)], 88);
    assert!(h.scheduler.snapshot_for(&1).profiles.is_empty());
}

#[test]
fn newer_same_account_authority_cannot_receive_late_old_member_progress() {
    let mut h = Harness::new();
    h.activate(1);
    let gate = h.gate(Operation::Enroll(profile(1)));
    let selected = h.select(1, &[1]);
    h.idle(false);
    selected.recv().unwrap().unwrap();
    h.until(true, |_| gate.entered.load(Ordering::Acquire));
    {
        let mut db = h.fake.0.lock().unwrap();
        db.revisions.insert(1, 2);
        db.grants.get_mut(&1).unwrap().remove(&profile(1));
    }
    h.scheduler.activate(Some(Credential(1, 2)));
    h.until(false, |h| {
        h.scheduler
            .snapshot_for(&1)
            .profiles
            .first()
            .is_some_and(|row| row.report.failure_code == Some(ACCESS_REVOKED))
    });
    gate.release();
    h.idle(true);
    let snapshot = h.scheduler.snapshot_for(&1);
    assert_eq!(
        snapshot.profiles[0].report.failure_code,
        Some(ACCESS_REVOKED)
    );
    assert_eq!(h.scheduler.take_revision(), Some(1));
    assert!(h.fake.0.lock().unwrap().targets[&1].is_empty());
}

#[test]
fn late_replaced_activation_load_cannot_publish_a_failure_on_the_new_incarnation() {
    let mut h = Harness::new();
    let gate = h.gate(Operation::Load(1));
    h.scheduler.activate(Some(Credential(1, 1)));
    h.until(false, |_| gate.entered.load(Ordering::Acquire));
    h.fake.0.lock().unwrap().revisions.insert(1, 2);
    h.scheduler.activate(Some(Credential(1, 2)));
    h.until(false, |h| h.scheduler.accounts[&1].initialized);
    let before = h.scheduler.snapshot_for(&1);
    gate.release();
    h.idle(false);
    assert_eq!(h.scheduler.snapshot_for(&1), before);
    assert_eq!(h.scheduler.authority_for_test(&1), Some(&Credential(1, 2)));
}

#[test]
fn identical_activation_and_unchanged_selection_keep_a_held_member_running() {
    let mut h = Harness::new();
    h.activate(1);
    let gate = h.gate(Operation::Enroll(profile(1)));
    let accepted = h.select(1, &[1]);
    h.idle(false);
    accepted.recv().unwrap().unwrap();
    h.until(true, |_| gate.entered.load(Ordering::Acquire));
    let incarnation = h.scheduler.accounts[&1].members[&profile(1)].incarnation;
    for _ in 0..20 {
        h.scheduler.activate(Some(Credential(1, 1)));
    }
    let accepted = h.select(1, &[1, 2]);
    h.until(false, |h| {
        h.fake.0.lock().unwrap().targets[&1].contains(&profile(2))
    });
    accepted.recv().unwrap().unwrap();
    let member = &h.scheduler.accounts[&1].members[&profile(1)];
    assert_eq!(member.incarnation, incarnation);
    assert!(!member.cancel.as_ref().unwrap().load(Ordering::Acquire));
    gate.release();
    h.idle(true);
    assert_eq!(h.fake.0.lock().unwrap().discoveries[&profile(1)], 1);
    assert_eq!(
        h.fake
            .0
            .lock()
            .unwrap()
            .enrollments
            .iter()
            .filter(|entry| entry.0 == profile(1))
            .count(),
        1
    );
}

#[test]
fn held_and_failed_refresh_keep_read_only_progress_until_scoped_retry_succeeds() {
    let mut h = Harness::new();
    h.activate(1);
    h.fake
        .0
        .lock()
        .unwrap()
        .empty
        .extend([profile(1), profile(2)]);
    let accepted = h.select(1, &[1, 2]);
    h.idle(true);
    accepted.recv().unwrap().unwrap();
    let before = h.scheduler.snapshot_for(&1);
    h.fake.0.lock().unwrap().revisions.insert(1, 2);
    h.fake.0.lock().unwrap().fail_refresh = true;
    let gate = h.gate(Operation::Refresh(1));
    h.scheduler.refresh(Credential(1, 1), Credential(1, 2));
    h.until(true, |_| gate.entered.load(Ordering::Acquire));
    let display = h
        .scheduler
        .snapshots()
        .into_iter()
        .find(|(credential, _)| credential.0 == 1)
        .unwrap();
    assert_eq!(display.0, Credential(1, 2));
    assert_eq!(display.1.profiles, before.profiles);
    assert_eq!(h.scheduler.authority_for_test(&1), Some(&Credential(1, 1)));
    gate.release();
    h.idle(true);
    let failed = h.scheduler.snapshot_for(&1);
    assert_eq!(failed.report.failure_code, Some(ENROLLMENT_UNAVAILABLE));
    assert!(failed.profiles.iter().all(|row| row.report.failure_code
        == Some(ENROLLMENT_UNAVAILABLE)
        && row.report.retry_at_millis.is_some()));
    h.scheduler.retry_for(&Credential(1, 2), None);
    h.idle(true);
    assert_eq!(h.scheduler.authority_for_test(&1), Some(&Credential(1, 2)));
    assert_eq!(h.scheduler.snapshot_for(&1).profiles, before.profiles);
    assert!(h.fake.0.lock().unwrap().enrollments.is_empty());
}

#[test]
fn old_cancel_and_refresh_cannot_invalidate_replacement_selection_or_refresh_job() {
    let mut h = Harness::new();
    h.activate(1);
    h.fake.0.lock().unwrap().revisions.insert(1, 2);
    let old_refresh = h.gate(Operation::Refresh(1));
    h.scheduler.refresh(Credential(1, 1), Credential(1, 2));
    h.until(false, |_| old_refresh.entered.load(Ordering::Acquire));
    h.fake.0.lock().unwrap().revisions.insert(1, 3);
    h.scheduler.activate(Some(Credential(1, 3)));
    h.until(false, |h| h.scheduler.accounts[&1].initialized);
    let selection = h.gate(Operation::Load(1));
    let (reply, accepted) = mpsc::sync_channel(1);
    h.scheduler
        .select(BTreeSet::from([profile(2)]), Credential(1, 3), reply);
    h.until(false, |_| selection.entered.load(Ordering::Acquire));
    let epoch = h.scheduler.epochs[&1];
    assert_eq!(
        h.scheduler.cancel(&Credential(1, 1)),
        Err(Failure::Persistence)
    );
    assert_eq!(h.scheduler.epochs[&1], epoch);
    selection.release();
    h.until(false, |h| {
        h.fake.0.lock().unwrap().targets[&1].contains(&profile(2))
    });
    assert_eq!(
        accepted.recv().unwrap().unwrap().profiles[0].profile,
        profile(2)
    );
    h.fake.0.lock().unwrap().revisions.insert(1, 4);
    let current_refresh = h.gate(Operation::Refresh(1));
    h.scheduler.refresh(Credential(1, 3), Credential(1, 4));
    h.until(false, |_| current_refresh.entered.load(Ordering::Acquire));
    let job = h.scheduler.accounts[&1].refresh_inflight;
    assert!(job.is_some());
    let before = h.scheduler.snapshot_for(&1);
    old_refresh.release();
    h.until(false, |h| {
        h.scheduler.lanes.iter().filter(|lane| lane.busy).count() == 1
    });
    assert_eq!(h.scheduler.accounts[&1].refresh_inflight, job);
    assert_eq!(h.scheduler.snapshot_for(&1), before);
    assert!(
        h.scheduler.pending.is_empty(),
        "old completion cannot queue a duplicate current refresh"
    );
    current_refresh.release();
    h.idle(false);
    assert_eq!(h.scheduler.authority_for_test(&1), Some(&Credential(1, 4)));
    assert_eq!(
        h.fake.0.lock().unwrap().targets[&1],
        BTreeSet::from([profile(2)])
    );
}
