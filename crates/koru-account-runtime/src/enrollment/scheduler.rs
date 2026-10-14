// SPDX-License-Identifier: MPL-2.0

//! The sole selected-set writer. Fixed lanes return owned effects; they never
//! mutate target sets or publish UI state.
use super::member::{EffectResult, Effects};
use super::*;

const EFFECT_LANES: usize = 2;

pub(super) type SelectionReply = mpsc::SyncSender<Result<Snapshot, &'static str>>;

enum Reply {
    Selection(SelectionReply),
    Cancel(mpsc::SyncSender<Result<(), &'static str>>),
}

impl Reply {
    fn send(self, result: Result<Snapshot, &'static str>) {
        match self {
            Self::Selection(reply) => {
                let _ = reply.try_send(result);
            }
            Self::Cancel(reply) => {
                let _ = reply.try_send(result.map(|_| ()));
            }
        }
    }
}

pub(super) struct Member<P: Port> {
    incarnation: u64,
    effects: Option<Effects<P>>,
    cancel: Option<Arc<AtomicBool>>,
    report: Report,
    failures: u32,
    retry_at: Option<Instant>,
    runnable: bool,
}

impl<P: Port> Member<P> {
    fn new(incarnation: u64) -> Self {
        Self {
            incarnation,
            effects: Some(Effects::default()),
            cancel: None,
            report: Report {
                state: EnrollmentState::Enrolling,
                ..Report::default()
            },
            failures: 0,
            retry_at: None,
            runnable: true,
        }
    }
}

struct Account<P: Port> {
    credential: P::Credential,
    persisted: BTreeSet<ProfileId>,
    members: BTreeMap<ProfileId, Member<P>>,
    report: Report,
    pending_refresh: Option<(P::Credential, P::Credential)>,
    initialized: bool,
    refresh_inflight: Option<u64>,
    refresh_retry_at: Option<Instant>,
}

enum Work<P: Port> {
    Member {
        credential: P::Credential,
        profile: ProfileId,
        incarnation: u64,
        effects: Effects<P>,
        cancel: Arc<AtomicBool>,
    },
    Load {
        epoch: u64,
        credential: P::Credential,
        next: Option<BTreeSet<ProfileId>>,
        reply: Option<Reply>,
    },
    Refresh {
        job: u64,
        previous: P::Credential,
        next: P::Credential,
        expected: BTreeSet<ProfileId>,
    },
}

enum Done<P: Port> {
    Member {
        credential: P::Credential,
        profile: ProfileId,
        incarnation: u64,
        result: EffectResult<P>,
    },
    Load {
        epoch: u64,
        credential: P::Credential,
        next: Option<BTreeSet<ProfileId>>,
        reply: Option<Reply>,
        result: Result<(BTreeSet<ProfileId>, BTreeSet<ProfileId>), Failure>,
    },
    Refresh {
        job: u64,
        previous: P::Credential,
        next: P::Credential,
        expected: BTreeSet<ProfileId>,
        result: Result<BTreeSet<ProfileId>, Failure>,
    },
}

struct Lane<P: Port> {
    send: mpsc::SyncSender<Work<P>>,
    busy: bool,
}

pub(super) struct Scheduler<P: Port> {
    port: P,
    accounts: BTreeMap<P::Scope, Account<P>>,
    selected: Option<P::Scope>,
    lanes: Vec<Lane<P>>,
    completed: mpsc::Receiver<(usize, Done<P>)>,
    pending: VecDeque<Work<P>>,
    next_incarnation: u64,
    cursor: Option<ProfileId>,
    control_last: bool,
    revision: Option<u64>,
    epochs: BTreeMap<P::Scope, u64>,
}

impl<P: Port> Scheduler<P> {
    pub fn new(port: P, wake: mpsc::SyncSender<()>) -> Result<Self, Failure> {
        let (finished, completed) = mpsc::channel();
        let mut lanes = Vec::new();
        for index in 0..EFFECT_LANES {
            let mut effects = port.fork_effects()?;
            let (send, receive) = mpsc::sync_channel(1);
            let finished = finished.clone();
            let wake = wake.clone();
            std::thread::Builder::new()
                .name(format!("koru-enrollment-effect-{index}"))
                .spawn(move || {
                    while let Ok(work) = receive.recv() {
                        let done = match work {
                            Work::Member {
                                credential,
                                profile,
                                incarnation,
                                effects: state,
                                cancel,
                            } => {
                                let result =
                                    state.step(&mut effects, profile, &credential, &cancel);
                                Done::Member {
                                    credential,
                                    profile,
                                    incarnation,
                                    result,
                                }
                            }
                            Work::Load {
                                epoch,
                                credential,
                                next,
                                reply,
                            } => {
                                let result =
                                    effects.restored_targets(&credential).and_then(|stored| {
                                        let mut revoked = BTreeSet::new();
                                        for profile in next.as_ref().unwrap_or(&stored) {
                                            if !effects.authorize_profile(*profile, &credential)? {
                                                if next.is_some() {
                                                    return Err(Failure::AccessRevoked);
                                                }
                                                revoked.insert(*profile);
                                            }
                                        }
                                        Ok((stored, revoked))
                                    });
                                Done::Load {
                                    epoch,
                                    credential,
                                    next,
                                    reply,
                                    result,
                                }
                            }
                            Work::Refresh {
                                job,
                                previous,
                                next,
                                expected,
                            } => {
                                let result = effects.refresh_authority(&previous, &next, &expected);
                                Done::Refresh {
                                    job,
                                    previous,
                                    next,
                                    expected,
                                    result,
                                }
                            }
                        };
                        if finished.send((index, done)).is_err() {
                            break;
                        }
                        let _ = wake.try_send(());
                    }
                })
                .map_err(|_| Failure::Unavailable)?;
            lanes.push(Lane { send, busy: false });
        }
        Ok(Self {
            port,
            accounts: BTreeMap::new(),
            selected: None,
            lanes,
            completed,
            pending: VecDeque::new(),
            next_incarnation: 1,
            cursor: None,
            control_last: false,
            revision: None,
            epochs: BTreeMap::new(),
        })
    }

    pub fn activate(&mut self, credential: Option<P::Credential>) {
        if self.selected == credential.as_ref().map(P::scope)
            && credential.as_ref().is_some_and(|credential| {
                self.accounts
                    .get(&P::scope(credential))
                    .is_some_and(|account| {
                        P::same_authority(&account.credential, credential)
                            || account
                                .pending_refresh
                                .as_ref()
                                .is_some_and(|(_, next)| P::same_authority(next, credential))
                    })
            })
        {
            return;
        }
        self.interrupt();
        self.selected = credential.as_ref().map(P::scope);
        self.cursor = None;
        let Some(credential) = credential else {
            return;
        };
        let scope = P::scope(&credential);
        if self
            .accounts
            .get(&scope)
            .is_some_and(|account| P::same_authority(&account.credential, &credential))
        {
            return;
        }
        if let Some(account) = self.accounts.get(&scope) {
            if account
                .pending_refresh
                .as_ref()
                .is_some_and(|(_, next)| P::same_authority(next, &credential))
            {
                return;
            }
            if !P::authority_successor(&account.credential, &credential) {
                return;
            }
            for member in account.members.values() {
                if let Some(cancel) = &member.cancel {
                    cancel.store(true, Ordering::Release);
                }
            }
        }
        self.accounts.insert(
            scope,
            Account {
                credential: credential.clone(),
                persisted: BTreeSet::new(),
                members: BTreeMap::new(),
                report: Report::default(),
                pending_refresh: None,
                initialized: false,
                refresh_inflight: None,
                refresh_retry_at: None,
            },
        );
        self.load(credential, None, None);
    }

    pub fn select(
        &mut self,
        profiles: BTreeSet<ProfileId>,
        credential: P::Credential,
        reply: SelectionReply,
    ) {
        self.load(credential, Some(profiles), Some(Reply::Selection(reply)));
    }

    fn load(
        &mut self,
        credential: P::Credential,
        next: Option<BTreeSet<ProfileId>>,
        reply: Option<Reply>,
    ) {
        if self
            .accounts
            .get(&P::scope(&credential))
            .is_some_and(|account| !P::same_authority(&account.credential, &credential))
        {
            if let Some(reply) = reply {
                reply.send(Err(PERSISTENCE_FAILED));
            }
            return;
        }
        if self.pending.len() >= 32 {
            if let Some(reply) = reply {
                reply.send(Err(ENROLLMENT_UNAVAILABLE));
            }
            return;
        }
        let epoch = self.epochs.entry(P::scope(&credential)).or_default();
        *epoch = epoch.checked_add(1).expect("selection epoch exhausted");
        self.pending.push_back(Work::Load {
            epoch: *epoch,
            credential,
            next,
            reply,
        });
    }

    pub fn refresh(&mut self, previous: P::Credential, next: P::Credential) {
        if !P::refresh_continuity(&previous, &next) {
            return;
        }
        if P::scope(&previous) != P::scope(&next) {
            return;
        }
        let scope = P::scope(&previous);
        let Some(account) = self.accounts.get_mut(&scope) else {
            return;
        };
        if !P::same_authority(&account.credential, &previous)
            && !account
                .pending_refresh
                .as_ref()
                .is_some_and(|(_, pending)| P::same_authority(pending, &previous))
        {
            return;
        }
        account.pending_refresh = Some((account.credential.clone(), next));
        account.refresh_retry_at = None;
        self.queue_refresh(&scope);
    }

    fn queue_refresh(&mut self, scope: &P::Scope) {
        if self.pending.len() >= 32 {
            return;
        }
        let Some(account) = self.accounts.get_mut(scope) else {
            return;
        };
        if account.refresh_inflight.is_some() {
            return;
        }
        if let Some((previous, next)) = account.pending_refresh.clone() {
            let job = self.next_incarnation;
            self.next_incarnation = self
                .next_incarnation
                .checked_add(1)
                .expect("control incarnation exhausted");
            account.refresh_inflight = Some(job);
            self.pending.push_back(Work::Refresh {
                job,
                previous,
                next,
                expected: account.persisted.clone(),
            });
        }
    }

    pub fn cancel(&mut self, credential: &P::Credential) -> Result<(), Failure> {
        let scope = P::scope(credential);
        let Some(account) = self.accounts.get_mut(&scope) else {
            self.port
                .replace_targets(credential, &BTreeSet::new(), &BTreeSet::new())?;
            let epoch = self.epochs.entry(scope).or_default();
            *epoch = epoch.checked_add(1).expect("selection epoch exhausted");
            return Ok(());
        };
        if !P::same_authority(&account.credential, credential)
            && !account
                .pending_refresh
                .as_ref()
                .is_some_and(|(_, next)| P::same_authority(next, credential))
        {
            return Err(Failure::Persistence);
        }
        // One CAS clears every unfinished member. An absent removed account
        // succeeds through the repository's existing empty-set idempotence.
        self.port
            .replace_targets(credential, &account.persisted, &BTreeSet::new())?;
        let epoch = self.epochs.entry(scope).or_default();
        *epoch = epoch.checked_add(1).expect("selection epoch exhausted");
        account.credential = credential.clone();
        account.pending_refresh = None;
        account.refresh_inflight = None;
        account.refresh_retry_at = None;
        account.persisted.clear();
        for (profile, member) in &mut account.members {
            if let Some(cancel) = &member.cancel {
                cancel.store(true, Ordering::Release);
            }
            if member.runnable {
                self.port.discard(*profile);
            }
        }
        account.members.retain(|_, member| {
            member.report.state == EnrollmentState::Complete && !member.runnable
        });
        account.report = Report::default();
        self.pending.retain(|work| match work {
            Work::Load {
                credential: captured,
                ..
            } => !P::same_authority(captured, credential),
            _ => true,
        });
        Ok(())
    }

    pub fn cancel_requested(
        &mut self,
        credential: P::Credential,
        reply: mpsc::SyncSender<Result<(), &'static str>>,
    ) {
        let scope = P::scope(&credential);
        if self
            .accounts
            .get(&scope)
            .is_none_or(|account| !account.initialized)
        {
            if self.cancel(&credential).is_ok() {
                let _ = reply.try_send(Ok(()));
                return;
            }
            // The account may still be loading its persisted set. Replace it
            // only after a fresh owned load, invalidating earlier load epochs.
            self.load(
                credential,
                Some(BTreeSet::new()),
                Some(Reply::Cancel(reply)),
            );
            return;
        }
        let result = self.cancel(&credential).map_err(failure_code);
        let _ = reply.try_send(result);
    }

    pub fn retry_for(&mut self, credential: &P::Credential, profile: Option<ProfileId>) {
        let scope = P::scope(credential);
        let Some(account) = self.accounts.get_mut(&scope) else {
            self.load(credential.clone(), None, None);
            return;
        };
        if !P::same_authority(&account.credential, credential)
            && !account
                .pending_refresh
                .as_ref()
                .is_some_and(|(_, next)| P::same_authority(next, credential))
        {
            return;
        }
        if account.pending_refresh.is_some() {
            account.refresh_retry_at = None;
            self.queue_refresh(&scope);
            return;
        }
        if !account.initialized {
            self.load(credential.clone(), None, None);
            return;
        }
        for (id, member) in &mut account.members {
            if profile.is_some_and(|profile| profile != *id) || !account.persisted.contains(id) {
                continue;
            }
            // Manual Retry must not erase an authoritative server cooldown.
            // Dispatch still wakes the member normally when its deadline arrives.
            if member.report.failure_code == Some(ENROLLMENT_RATE_LIMITED)
                && member.retry_at.is_some()
            {
                continue;
            }
            if matches!(
                member.report.state,
                EnrollmentState::Waiting | EnrollmentState::Failed
            ) || member.report.failure_code.is_some()
            {
                member.retry_at = None;
                member.report.retry_at_millis = None;
                member.report.failure_code = None;
                member.failures = 0;
                member.runnable = true;
            }
        }
    }

    pub fn interrupt(&self) {
        for account in self.accounts.values() {
            for member in account.members.values() {
                if let Some(cancel) = &member.cancel {
                    cancel.store(true, Ordering::Release);
                }
            }
        }
    }

    pub fn drain(&mut self, clock: &dyn Clock) {
        while let Ok((index, done)) = self.completed.try_recv() {
            self.lanes[index].busy = false;
            match done {
                Done::Member {
                    credential,
                    profile,
                    incarnation,
                    result,
                } => {
                    self.finish_member(credential, profile, incarnation, result, clock);
                }
                Done::Load {
                    epoch,
                    credential,
                    next,
                    reply,
                    result,
                } => {
                    let result = if self.epochs.get(&P::scope(&credential)) != Some(&epoch) {
                        Err(Failure::Persistence)
                    } else {
                        result.and_then(|(stored, revoked)| {
                            self.finish_load(credential.clone(), stored, next, revoked)
                        })
                    };
                    if let Some(reply) = reply {
                        reply.send(
                            result
                                .map(|_| self.snapshot_for(&P::scope(&credential)))
                                .map_err(failure_code),
                        );
                    } else if let Err(error) = result {
                        if self.epochs.get(&P::scope(&credential)) != Some(&epoch) {
                            continue;
                        }
                        if let Some(account) = self
                            .accounts
                            .get_mut(&P::scope(&credential))
                            .filter(|account| P::same_authority(&account.credential, &credential))
                        {
                            account.report = failed(failure_code(error));
                        }
                    }
                }
                Done::Refresh {
                    job,
                    previous,
                    next,
                    expected,
                    result,
                } => {
                    if let Some(account) = self.accounts.get_mut(&P::scope(&previous)) {
                        if account.refresh_inflight != Some(job) {
                            continue;
                        }
                        account.refresh_inflight = None;
                        if P::same_authority(&account.credential, &previous)
                            && account.persisted == expected
                        {
                            let result = result.and_then(|revoked| {
                                if !revoked.is_empty() {
                                    let retained = expected.difference(&revoked).copied().collect();
                                    self.port.replace_targets(&next, &expected, &retained)?;
                                    account.persisted = retained;
                                    for profile in &revoked {
                                        if let Some(member) = account.members.get_mut(profile) {
                                            if let Some(cancel) = &member.cancel {
                                                cancel.store(true, Ordering::Release);
                                            }
                                            member.incarnation = self.next_incarnation;
                                            self.next_incarnation += 1;
                                            member.runnable = false;
                                            member.report = failed(ACCESS_REVOKED);
                                        }
                                    }
                                }
                                Ok(())
                            });
                            match result {
                                Ok(()) => {
                                    account.credential = next.clone();
                                    if let Some((_, latest)) = account.pending_refresh.take() {
                                        if !P::same_authority(&latest, &next) {
                                            account.pending_refresh = Some((next, latest));
                                        }
                                    }
                                    account.refresh_retry_at = None;
                                    account.report = Report::default();
                                }
                                Err(error) => {
                                    account.report = failed(failure_code(error));
                                    account.refresh_retry_at =
                                        clock.monotonic().checked_add(MIN_BACKOFF);
                                    account.report.retry_at_millis = checked_retry_at(
                                        clock.now_millis().unwrap_or(0),
                                        MIN_BACKOFF,
                                    );
                                }
                            }
                        } else if account.pending_refresh.is_some() {
                            account.refresh_retry_at = clock.monotonic().checked_add(MIN_BACKOFF);
                        }
                    }
                }
            }
        }
    }

    fn finish_load(
        &mut self,
        credential: P::Credential,
        stored: BTreeSet<ProfileId>,
        next: Option<BTreeSet<ProfileId>>,
        revoked: BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        let scope = P::scope(&credential);
        if let Some(account) = self.accounts.get(&scope) {
            if !P::same_authority(&account.credential, &credential) {
                return Err(Failure::Persistence);
            }
        }
        let next = if let Some(next) = next {
            self.port.replace_targets(&credential, &stored, &next)?;
            next
        } else {
            if !revoked.is_empty() {
                let authorized = stored.difference(&revoked).copied().collect();
                self.port
                    .replace_targets(&credential, &stored, &authorized)?;
            }
            stored
        };
        let account = self.accounts.entry(scope).or_insert_with(|| Account {
            credential,
            persisted: BTreeSet::new(),
            members: BTreeMap::new(),
            report: Report::default(),
            pending_refresh: None,
            initialized: false,
            refresh_inflight: None,
            refresh_retry_at: None,
        });
        for (profile, member) in &account.members {
            if !next.contains(profile) {
                if let Some(cancel) = &member.cancel {
                    cancel.store(true, Ordering::Release);
                }
                self.port.discard(*profile);
            }
        }
        // Unchanged members keep their exact proof, candidate, deadline and
        // running job. A removed then re-added ID gets a new incarnation.
        account.members.retain(|profile, _| next.contains(profile));
        for profile in &next {
            if !account.members.contains_key(profile) || !account.persisted.contains(profile) {
                let incarnation = self.next_incarnation;
                self.next_incarnation = self
                    .next_incarnation
                    .checked_add(1)
                    .expect("member incarnation exhausted");
                account.members.insert(*profile, Member::new(incarnation));
            }
        }
        account.persisted = next;
        for profile in revoked {
            account.persisted.remove(&profile);
            let member = account.members.get_mut(&profile).expect("restored member");
            if let Some(cancel) = &member.cancel {
                cancel.store(true, Ordering::Release);
            }
            member.runnable = false;
            member.incarnation = self.next_incarnation;
            self.next_incarnation += 1;
            member.report = failed(ACCESS_REVOKED);
        }
        account.initialized = true;
        account.report = Report::default();
        Ok(())
    }

    fn finish_member(
        &mut self,
        credential: P::Credential,
        profile: ProfileId,
        incarnation: u64,
        result: EffectResult<P>,
        clock: &dyn Clock,
    ) {
        // Even retired or replaced work can return a valid durable commit.
        // Its revision is handed to replication independently of UI ownership.
        if let Some(revision) = result.durable_revision {
            self.revision = Some(self.revision.unwrap_or(0).max(revision));
        }
        let Some(account) = self.accounts.get_mut(&P::scope(&credential)) else {
            return;
        };
        let Some(member) = account.members.get_mut(&profile) else {
            return;
        };
        if member.incarnation != incarnation {
            return;
        }
        member.cancel = None;
        member.effects = Some(result.effects);
        if result.cloud_primary {
            // Retain the durable selected set and all existing trust. Check
            // the local choice again later so disabling Cloud resumes peers.
            member.report = Report::default();
            member.failures = 0;
            member.runnable = true;
            member.retry_at = clock.monotonic().checked_add(MAX_BACKOFF);
            return;
        }
        if result.interrupted && result.waiting_code.is_none() {
            if let Some(revision) = result.durable_revision {
                member.report = Report {
                    state: EnrollmentState::Complete,
                    durable_revision: Some(revision),
                    ..Report::default()
                };
            }
            member.runnable = true;
            return;
        }
        let mut failure = result.failure;
        if (result.remove || failure == Some(Failure::AccessRevoked))
            && account.pending_refresh.is_some()
        {
            // Keep the completed effect until the sole set owner rebases its
            // authority. The refresh transaction sees one unchanged set.
            member.runnable = true;
            return;
        }
        if result.remove || failure == Some(Failure::AccessRevoked) {
            let mut next = account.persisted.clone();
            next.remove(&profile);
            match self
                .port
                .replace_targets(&account.credential, &account.persisted, &next)
            {
                Ok(()) => {
                    account.persisted = next;
                    member.runnable = false;
                    member.report = if result.remove {
                        Report {
                            state: EnrollmentState::Complete,
                            durable_revision: result.durable_revision,
                            ..Report::default()
                        }
                    } else {
                        failed(ACCESS_REVOKED)
                    };
                    if !result.remove {
                        self.port.discard(profile);
                    }
                    return;
                }
                Err(error) => failure = Some(error),
            }
        }
        let complete = member.effects.as_ref().is_some_and(Effects::complete);
        if let Some(code) = result.waiting_code.or_else(|| failure.map(failure_code)) {
            let automatic = result.waiting_code.is_some()
                || matches!(
                    failure,
                    Some(
                        Failure::Offline
                            | Failure::Unavailable
                            | Failure::Uncertain
                            | Failure::RateLimited(_)
                    )
                )
                || (complete
                    && !matches!(
                        failure,
                        Some(Failure::SessionExpired | Failure::AccessRevoked)
                    ));
            let delay = MIN_BACKOFF
                .checked_mul(1_u32 << member.failures.min(6))
                .unwrap_or(MAX_BACKOFF)
                .min(MAX_BACKOFF);
            let delay = match failure {
                Some(Failure::RateLimited(retry_after)) => delay.max(retry_after),
                _ => delay,
            };
            member.failures = member.failures.saturating_add(1);
            member.retry_at = automatic
                .then(|| clock.monotonic().checked_add(delay))
                .flatten();
            member.runnable = automatic;
            member.report = Report {
                state: if complete {
                    EnrollmentState::Complete
                } else if automatic {
                    EnrollmentState::Waiting
                } else {
                    EnrollmentState::Failed
                },
                failure_code: Some(
                    if complete
                        && failure != Some(Failure::Persistence)
                        && !matches!(failure, Some(Failure::RateLimited(_)))
                        && automatic
                    {
                        PROVISIONING_PENDING
                    } else {
                        code
                    },
                ),
                retry_at_millis: automatic
                    .then(|| checked_retry_at(clock.now_millis().unwrap_or(0), delay))
                    .flatten(),
                durable_revision: result.durable_revision,
            };
        } else {
            member.runnable = true;
            member.retry_at = None;
            member.report = Report {
                state: if complete {
                    EnrollmentState::Complete
                } else {
                    EnrollmentState::Enrolling
                },
                durable_revision: result.durable_revision,
                ..Report::default()
            };
        }
    }

    pub fn dispatch(&mut self, clock: &dyn Clock, foreground: bool) {
        let refreshes = self
            .accounts
            .iter()
            .filter(|(_, account)| {
                account.pending_refresh.is_some()
                    && account.refresh_inflight.is_none()
                    && account
                        .refresh_retry_at
                        .is_none_or(|at| at <= clock.monotonic())
            })
            .map(|(scope, _)| scope.clone())
            .collect::<Vec<_>>();
        for scope in refreshes {
            self.queue_refresh(&scope);
        }
        for index in 0..self.lanes.len() {
            if self.lanes[index].busy {
                continue;
            }
            let member = if foreground {
                self.next_member(clock)
            } else {
                None
            };
            let work = if !self.control_last && !self.pending.is_empty() {
                self.control_last = true;
                self.pending.pop_front()
            } else if let Some(profile) = member {
                self.control_last = false;
                let account = self
                    .accounts
                    .get_mut(self.selected.as_ref().expect("selected account"))
                    .expect("account");
                let member = account.members.get_mut(&profile).expect("member");
                let cancel = Arc::new(AtomicBool::new(false));
                member.cancel = Some(cancel.clone());
                let effects = member.effects.take().expect("idle member owns effects");
                self.cursor = Some(profile);
                Some(Work::Member {
                    credential: account.credential.clone(),
                    profile,
                    incarnation: member.incarnation,
                    effects,
                    cancel,
                })
            } else {
                self.control_last = true;
                self.pending.pop_front()
            };
            if let Some(work) = work {
                if self.lanes[index].send.send(work).is_ok() {
                    self.lanes[index].busy = true;
                }
            }
        }
    }

    fn next_member(&self, clock: &dyn Clock) -> Option<ProfileId> {
        let account = self.accounts.get(self.selected.as_ref()?)?;
        if account.pending_refresh.is_some() {
            return None;
        }
        let runnable = |(_, member): &(&ProfileId, &Member<P>)| {
            member.runnable
                && member.effects.is_some()
                && member.retry_at.is_none_or(|at| at <= clock.monotonic())
        };
        account
            .members
            .iter()
            .filter(runnable)
            .map(|(id, _)| *id)
            .find(|id| self.cursor.is_none_or(|cursor| *id > cursor))
            .or_else(|| {
                account
                    .members
                    .iter()
                    .filter(runnable)
                    .map(|(id, _)| *id)
                    .next()
            })
    }

    pub fn deadline(&self, foreground: bool) -> Option<Instant> {
        if !foreground || self.lanes.iter().all(|lane| lane.busy) {
            return None;
        }
        let member = self
            .selected
            .as_ref()
            .and_then(|scope| self.accounts.get(scope))
            .filter(|account| account.pending_refresh.is_none())
            .into_iter()
            .flat_map(|account| account.members.values())
            .filter(|member| member.runnable && member.effects.is_some())
            .filter_map(|member| member.retry_at)
            .min();
        member
            .into_iter()
            .chain(
                self.accounts
                    .values()
                    .filter(|account| account.refresh_inflight.is_none())
                    .filter_map(|account| account.refresh_retry_at),
            )
            .min()
    }

    pub fn snapshot_for(&self, scope: &P::Scope) -> Snapshot {
        let Some(account) = self.accounts.get(scope) else {
            return Snapshot::default();
        };
        let profiles: Vec<_> = account
            .members
            .iter()
            .map(|(profile, member)| {
                let report = if account.pending_refresh.is_some()
                    && account.report.failure_code.is_some()
                    && account.persisted.contains(profile)
                {
                    Report {
                        durable_revision: member.report.durable_revision,
                        ..account.report.clone()
                    }
                } else {
                    member.report.clone()
                };
                ProfileReport {
                    profile: *profile,
                    report,
                }
            })
            .collect();
        let report = aggregate(&profiles, &account.report);
        Snapshot {
            owner_scope: None,
            account_id: P::snapshot_account_id(&account.credential),
            target: (profiles.len() == 1).then(|| profiles[0].profile),
            profiles,
            report,
        }
    }

    pub fn snapshots(&self) -> impl Iterator<Item = (P::Credential, Snapshot)> + '_ {
        self.accounts.iter().map(|(scope, account)| {
            // Explicit same-session refresh continuity permits a read-only
            // projection while revalidation is pending. Effects remain paused.
            let visible = account
                .pending_refresh
                .as_ref()
                .map(|(_, next)| next)
                .unwrap_or(&account.credential);
            (visible.clone(), self.snapshot_for(scope))
        })
    }

    pub fn take_revision(&mut self) -> Option<u64> {
        self.revision.take()
    }

    #[cfg(test)]
    pub(super) fn idle_for_test(&self, clock: &dyn Clock, foreground: bool) -> bool {
        self.lanes.iter().all(|lane| !lane.busy)
            && self.pending.is_empty()
            && (!foreground || self.next_member(clock).is_none())
    }

    #[cfg(test)]
    pub(super) fn authority_for_test(&self, scope: &P::Scope) -> Option<&P::Credential> {
        self.accounts.get(scope).map(|account| &account.credential)
    }
}

fn aggregate(profiles: &[ProfileReport], fallback: &Report) -> Report {
    if fallback.failure_code.is_some() {
        return fallback.clone();
    }
    if profiles.is_empty() {
        return fallback.clone();
    }
    let state = if profiles
        .iter()
        .any(|profile| profile.report.state == EnrollmentState::Enrolling)
    {
        EnrollmentState::Enrolling
    } else if profiles
        .iter()
        .any(|profile| profile.report.state == EnrollmentState::Waiting)
    {
        EnrollmentState::Waiting
    } else if profiles
        .iter()
        .any(|profile| profile.report.state == EnrollmentState::Failed)
    {
        EnrollmentState::Failed
    } else if profiles
        .iter()
        .any(|profile| profile.report.state == EnrollmentState::Idle)
    {
        EnrollmentState::Idle
    } else {
        EnrollmentState::Complete
    };
    Report {
        state,
        failure_code: profiles
            .iter()
            .find_map(|profile| profile.report.failure_code),
        retry_at_millis: profiles
            .iter()
            .filter_map(|profile| profile.report.retry_at_millis)
            .min(),
        durable_revision: profiles
            .iter()
            .filter_map(|profile| profile.report.durable_revision)
            .max(),
    }
}

impl<P: Port> Drop for Scheduler<P> {
    fn drop(&mut self) {
        self.interrupt();
    }
}

#[cfg(test)]
mod tests;
