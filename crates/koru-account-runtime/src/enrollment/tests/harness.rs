// SPDX-License-Identifier: MPL-2.0

//! Drives the actual fixed-lane scheduler to an observable boundary, using
//! completion wakes rather than sleeps or a second scheduling implementation.
use super::super::scheduler::Scheduler;
use super::*;

pub(super) struct SharedPort<P>(pub Arc<Mutex<P>>);
impl<P> Clone for SharedPort<P> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<P> SharedPort<P> {
    pub fn new(port: P) -> Self {
        Self(Arc::new(Mutex::new(port)))
    }
}
impl<P: Port> Port for SharedPort<P> {
    fn cloud_primary(
        &mut self,
        profile: ProfileId,
        credential: &P::Credential,
    ) -> Result<bool, Failure> {
        self.0.lock().unwrap().cloud_primary(profile, credential)
    }
    type Scope = P::Scope;
    type Credential = P::Credential;
    type Candidate = P::Candidate;
    type Attempt = P::Attempt;
    type Completion = P::Completion;
    fn fork_effects(&self) -> Result<Self, Failure> {
        Ok(self.clone())
    }
    fn scope(value: &Self::Credential) -> Self::Scope {
        P::scope(value)
    }
    fn same_authority(a: &Self::Credential, b: &Self::Credential) -> bool {
        P::same_authority(a, b)
    }
    fn authority_successor(a: &Self::Credential, b: &Self::Credential) -> bool {
        P::authority_successor(a, b)
    }
    fn refresh_continuity(a: &Self::Credential, b: &Self::Credential) -> bool {
        P::refresh_continuity(a, b)
    }
    fn refresh_authority(
        &mut self,
        a: &Self::Credential,
        b: &Self::Credential,
        expected: &BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        self.0.lock().unwrap().refresh_authority(a, b, expected)
    }
    fn restored_targets(
        &mut self,
        credential: &Self::Credential,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        self.0.lock().unwrap().restored_targets(credential)
    }
    fn replace_targets(
        &mut self,
        credential: &Self::Credential,
        expected: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        self.0
            .lock()
            .unwrap()
            .replace_targets(credential, expected, next)
    }
    fn authorize_profile(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
    ) -> Result<bool, Failure> {
        self.0
            .lock()
            .unwrap()
            .authorize_profile(profile, credential)
    }
    fn reconcile(
        &mut self,
        profile: ProfileId,
    ) -> Result<Option<Completion<Self::Completion>>, Failure> {
        self.0.lock().unwrap().reconcile(profile)
    }
    fn completion_current(
        &mut self,
        profile: ProfileId,
        value: &Self::Completion,
    ) -> Result<bool, Failure> {
        self.0.lock().unwrap().completion_current(profile, value)
    }
    fn discover(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
        cancel: &AtomicBool,
    ) -> Result<Discovery<Self::Candidate, Self::Attempt>, Failure> {
        self.0.lock().unwrap().discover(profile, credential, cancel)
    }
    fn refresh_discovery(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
        previous: &Self::Attempt,
        cancel: &AtomicBool,
    ) -> Result<Discovery<Self::Candidate, Self::Attempt>, Failure> {
        self.0
            .lock()
            .unwrap()
            .refresh_discovery(profile, credential, previous, cancel)
    }
    fn enroll(
        &mut self,
        profile: ProfileId,
        candidate: &Self::Candidate,
        attempt: &Self::Attempt,
        cancel: &AtomicBool,
    ) -> Result<Completion<Self::Completion>, Failure> {
        self.0
            .lock()
            .unwrap()
            .enroll(profile, candidate, attempt, cancel)
    }
    fn attempt_expired(&self, attempt: &Self::Attempt) -> bool {
        self.0.lock().unwrap().attempt_expired(attempt)
    }
    fn provision(
        &mut self,
        profile: ProfileId,
        value: &Self::Completion,
        credential: &Self::Credential,
    ) -> Result<(), Failure> {
        self.0.lock().unwrap().provision(profile, value, credential)
    }
    fn discard(&mut self, profile: ProfileId) {
        self.0.lock().unwrap().discard(profile);
    }
}

struct TestClock {
    origin: Instant,
    now: std::sync::atomic::AtomicI64,
}
impl Clock for TestClock {
    fn now_millis(&self) -> Option<i64> {
        Some(self.now.load(Ordering::Acquire))
    }
    fn monotonic(&self) -> Instant {
        self.origin + Duration::from_millis(self.now_millis().unwrap().max(0) as u64)
    }
}
pub(super) struct Harness<P: Port> {
    pub port: Arc<Mutex<P>>,
    engine: Scheduler<SharedPort<P>>,
    wakes: mpsc::Receiver<()>,
    clock: TestClock,
    pub credential: Option<P::Credential>,
    pub report: Report,
    pub selection_error: Option<&'static str>,
}
impl<P: Port> Harness<P> {
    pub fn new(port: P) -> Self {
        Self::from_shared(Arc::new(Mutex::new(port)))
    }
    pub fn from_shared(port: Arc<Mutex<P>>) -> Self {
        let port = SharedPort(port);
        let (wake, wakes) = mpsc::sync_channel(1);
        Self {
            port: port.0.clone(),
            engine: Scheduler::new(port, wake).unwrap(),
            wakes,
            clock: TestClock {
                origin: Instant::now(),
                now: std::sync::atomic::AtomicI64::new(0),
            },
            credential: None,
            report: Report::default(),
            selection_error: None,
        }
    }
    fn settle(&mut self, foreground: bool) {
        loop {
            self.engine.drain(&self.clock);
            self.engine.dispatch(&self.clock, foreground);
            if self.engine.idle_for_test(&self.clock, foreground) {
                break;
            }
            self.wakes
                .recv_timeout(Duration::from_secs(3))
                .expect("scheduler effect did not report completion");
        }
        self.report = self.snapshot().report;
    }
    pub fn snapshot_for(&self, credential: &P::Credential) -> Snapshot {
        self.engine.snapshot_for(&P::scope(credential))
    }
    pub fn authority_for(&self, credential: &P::Credential) -> Option<P::Credential> {
        self.engine
            .authority_for_test(&P::scope(credential))
            .cloned()
    }
    pub fn snapshot(&self) -> Snapshot {
        self.credential
            .as_ref()
            .map(|value| self.snapshot_for(value))
            .unwrap_or_default()
    }
    pub fn report(&self) -> &Report {
        &self.report
    }
    pub fn activate(&mut self, credential: Option<P::Credential>) -> bool {
        let before = self.snapshot();
        self.engine.activate(credential.clone());
        self.credential = credential;
        self.settle(false);
        before != self.snapshot()
    }
    pub fn update_credential(&mut self, credential: P::Credential) -> bool {
        self.activate(Some(credential))
    }
    pub fn select_set(
        &mut self,
        profiles: BTreeSet<ProfileId>,
        credential: P::Credential,
        visible: bool,
    ) -> bool {
        if visible {
            self.activate(Some(credential.clone()));
        }
        let (send, receive) = mpsc::sync_channel(1);
        self.engine.select(profiles, credential, send);
        self.settle(false);
        let result = receive.recv_timeout(Duration::from_secs(3)).unwrap();
        self.selection_error = result.as_ref().err().copied();
        result.is_ok()
    }
    pub fn select(&mut self, profile: ProfileId, credential: P::Credential) -> bool {
        self.select_set(BTreeSet::from([profile]), credential, true)
    }
    pub fn select_parked(&mut self, profile: ProfileId, credential: P::Credential) -> bool {
        self.select_set(BTreeSet::from([profile]), credential, false)
    }
    pub fn step(&mut self, now: i64, cancel: &AtomicBool) -> Option<Duration> {
        self.clock.now.store(now, Ordering::Release);
        if cancel.load(Ordering::Acquire) {
            self.engine.interrupt();
        }
        self.settle(!cancel.load(Ordering::Acquire));
        if cancel.load(Ordering::Acquire) {
            None
        } else {
            self.report
                .retry_at_millis
                .map(|at| Duration::from_millis((at - now).max(0) as u64))
        }
    }
    pub fn retry(&mut self) -> bool {
        let Some(credential) = &self.credential else {
            return false;
        };
        let before = self.snapshot();
        self.engine.retry_for(credential, None);
        self.report = self.snapshot().report;
        before != self.snapshot()
    }
    pub fn cancel_for(&mut self, credential: &P::Credential) -> Result<(), Failure> {
        let result = self.engine.cancel(credential);
        self.report = self.snapshot().report;
        result
    }
    pub fn cancel(&mut self) -> Result<(), Failure> {
        match self.credential.clone() {
            Some(value) => self.cancel_for(&value),
            None => Ok(()),
        }
    }
    pub fn refresh_authorization(&mut self, previous: P::Credential, next: P::Credential) -> bool {
        self.engine.refresh(previous.clone(), next.clone());
        self.settle(false);
        let accepted = self
            .engine
            .authority_for_test(&P::scope(&next))
            .is_some_and(|credential| P::same_authority(credential, &next));
        if accepted
            && self
                .credential
                .as_ref()
                .is_some_and(|value| P::same_authority(value, &previous))
        {
            self.credential = Some(next);
        }
        accepted
    }
}
