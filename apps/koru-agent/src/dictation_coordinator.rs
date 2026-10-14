// SPDX-License-Identifier: MPL-2.0

//! Global dictation policy and its off-event-thread worker.
//!
//! The shared koru-dictation controller remains the only transcript/session
//! state machine. This module adds only the global consequences around it:
//! capture one opaque target before opening Voice, consume that target on the
//! sole final commit, and publish content-free state for later Voice/HUD views.

use std::time::Instant;

use crate::global_input::{self, CapturedTarget, Commit, HoldOutcome, InsertBackend};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Listening,
    Finalising,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Disabled,
    Target(global_input::Refusal),
    Voice(koru_dictation::ErrorCode),
    DirectPartial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Inserted,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressResult {
    Started(koru_dictation::Mode),
    Finished,
    Ignored,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalState {
    Refused,
    Listening(koru_dictation::Mode),
    Finalising,
    Idle,
}

/// Everything a presentation surface may learn. It carries neither transcript
/// nor target identity, and equality is the wake-coalescing boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub phase: Phase,
    pub level: koru_dictation::LevelBucket,
    pub prepared: bool,
    pub refusal: Option<Refusal>,
    pub outcome: Option<Outcome>,
    pub display: Option<global_input::TargetDisplay>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            level: koru_dictation::LevelBucket::SILENT,
            prepared: false,
            refusal: None,
            outcome: None,
            display: None,
        }
    }
}

#[derive(Clone)]
pub struct Policy {
    pub enabled: bool,
    pub preferences: koru_dictation::Preferences,
    pub consent: Option<koru_voice_consent::Granted>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            preferences: koru_dictation::Preferences::default(),
            consent: None,
        }
    }
}

/// Deterministic coordinator core. Production owns it on the dedicated worker;
/// tests use a fake insertion backend and feed Voice events directly.
pub struct Coordinator<B: InsertBackend> {
    dictation: koru_dictation::Dictation,
    backend: B,
    /// Target held for an active session and consumed only by final commit.
    target: Option<CapturedTarget<B::Target>>,
    /// Content-free application identity captured before Voice takes focus.
    /// Accessibility/control capture happens only after current policy passes.
    prepared_hint: Option<global_input::CapturedRestoreHint<B::RestoreHint>>,
    policy: Policy,
    snapshot: Snapshot,
    published: Snapshot,
    notify: Box<dyn FnMut(Snapshot) + Send>,
}

impl<B: InsertBackend> Coordinator<B> {
    pub fn new(
        dictation: koru_dictation::Dictation,
        backend: B,
        notify: impl FnMut(Snapshot) + Send + 'static,
    ) -> Self {
        Self {
            dictation,
            backend,
            target: None,
            prepared_hint: None,
            policy: Policy::default(),
            snapshot: Snapshot::default(),
            published: Snapshot::default(),
            notify: Box::new(notify),
        }
    }

    pub fn client(&self) -> &koru_dictation::VoiceClient {
        self.dictation.client()
    }

    pub fn configure(&mut self, policy: Policy, now: Instant) {
        let revokes_live =
            self.dictation.is_live() && (!policy.enabled || policy.consent.is_none());
        let revokes_prepared = (self.policy.enabled && !policy.enabled)
            || (self.policy.consent.is_some() && policy.consent.is_none());
        self.policy = policy;
        if revokes_prepared {
            self.prepared_hint = None;
            self.snapshot.prepared = false;
            self.snapshot.display = None;
        }
        if revokes_live {
            self.dictation
                .interrupt(koru_dictation::Interruption::Suspended);
            self.target = None;
            self.snapshot.prepared = false;
            self.service(now);
        } else if revokes_prepared {
            self.publish();
        }
    }

    pub fn press(&mut self, now: Instant) -> PressResult {
        self.press_with_release_ready(now, true)
    }

    pub fn prepare_target(&mut self) -> PressResult {
        if self.dictation.is_live() {
            return PressResult::Ignored;
        }
        self.snapshot.refusal = None;
        self.snapshot.outcome = None;
        self.snapshot.prepared = false;
        self.snapshot.display = None;
        self.prepared_hint = None;
        // A repeated preparation replaces the prior one. Publish the cleared
        // state first so a host waiting to open Voice cannot miss a same-shape
        // success or refusal through snapshot deduplication.
        self.publish();
        self.snapshot.display = self.backend.current_display();
        match global_input::capture_restore_hint(&mut self.backend) {
            Ok(hint) => {
                self.prepared_hint = Some(hint);
                self.snapshot.prepared = true;
                self.publish();
                PressResult::Started(self.policy.preferences.mode)
            }
            Err(reason) => {
                self.prepared_hint = None;
                self.snapshot.refusal = Some(Refusal::Target(reason));
                self.publish();
                PressResult::Refused
            }
        }
    }

    pub fn start_prepared(&mut self, now: Instant) -> PressResult {
        self.snapshot.prepared = false;
        if !self.policy.enabled {
            self.prepared_hint = None;
            self.snapshot.refusal = Some(Refusal::Disabled);
            self.publish();
            return PressResult::Refused;
        }
        if self.policy.consent.is_none() {
            self.prepared_hint = None;
            self.snapshot.refusal =
                Some(Refusal::Voice(koru_dictation::ErrorCode::ConsentRequired));
            self.publish();
            return PressResult::Refused;
        }
        let Some(hint) = self.prepared_hint.take() else {
            self.snapshot.refusal = Some(Refusal::Target(global_input::Refusal::TargetUnavailable));
            self.publish();
            return PressResult::Refused;
        };
        if let Err(reason) = global_input::restore_hint(&mut self.backend, &hint) {
            self.snapshot.refusal = Some(Refusal::Target(reason));
            self.publish();
            return PressResult::Refused;
        }
        let target = match global_input::capture_for_hint(&mut self.backend, &hint) {
            Ok(target) => target,
            Err(reason) => {
                self.snapshot.refusal = Some(Refusal::Target(reason));
                self.publish();
                return PressResult::Refused;
            }
        };
        if let Err(reason) = global_input::restore(&mut self.backend, &target) {
            self.snapshot.refusal = Some(Refusal::Target(reason));
            self.publish();
            return PressResult::Refused;
        }
        self.policy.preferences.apply_to(&mut self.dictation);
        self.dictation.set_consent(self.policy.consent.clone());
        match self.dictation.press(now) {
            Some(koru_dictation::DictationAction::Begin { .. }) => {
                self.target = Some(target);
                self.dictation.began(true);
                let mode = self.dictation.mode();
                self.service(now);
                PressResult::Started(mode)
            }
            Some(_) => unreachable!("press only produces Begin"),
            None => {
                self.take_voice_refusal();
                self.service(now);
                PressResult::Refused
            }
        }
    }

    pub fn discard_prepared(&mut self) {
        if !self.dictation.is_live() {
            self.prepared_hint = None;
            self.snapshot.prepared = false;
            self.snapshot.display = None;
            self.publish();
        }
    }

    fn press_with_release_ready(&mut self, now: Instant, release_ready: bool) -> PressResult {
        self.snapshot.refusal = None;
        self.snapshot.outcome = None;

        // A second toggle press finishes the existing session. It must not
        // capture a second target or re-check consent before allowing stop.
        if self.dictation.is_live() {
            let before = self.dictation.stage();
            let _ = self.dictation.press(now);
            self.service(now);
            return if before == koru_dictation::Stage::Live
                && self.dictation.stage() == koru_dictation::Stage::Finalising
            {
                PressResult::Finished
            } else {
                PressResult::Ignored
            };
        }
        self.snapshot.display = self.backend.current_display();
        if !self.policy.enabled {
            self.snapshot.refusal = Some(Refusal::Disabled);
            self.publish();
            return PressResult::Refused;
        }
        if self.policy.consent.is_none() {
            self.snapshot.refusal =
                Some(Refusal::Voice(koru_dictation::ErrorCode::ConsentRequired));
            self.publish();
            return PressResult::Refused;
        }
        if self.policy.preferences.mode == koru_dictation::Mode::PushToTalk && !release_ready {
            self.snapshot.refusal = Some(Refusal::Target(global_input::Refusal::TargetUnavailable));
            self.publish();
            return PressResult::Refused;
        }

        // Capture and classify before Voice can open a microphone.
        let target = match global_input::capture(&mut self.backend) {
            Ok(target) => target,
            Err(reason) => {
                self.snapshot.refusal = Some(Refusal::Target(reason));
                self.publish();
                return PressResult::Refused;
            }
        };
        self.policy.preferences.apply_to(&mut self.dictation);
        self.dictation.set_consent(self.policy.consent.clone());
        match self.dictation.press(now) {
            Some(koru_dictation::DictationAction::Begin { .. }) => {
                self.target = Some(target);
                self.dictation.began(true);
                let mode = self.dictation.mode();
                self.service(now);
                return PressResult::Started(mode);
            }
            Some(_) => unreachable!("press only produces Begin"),
            None => {
                self.target = None;
                self.take_voice_refusal();
            }
        }
        self.service(now);
        PressResult::Refused
    }

    pub fn release(&mut self, now: Instant) {
        let _ = self.dictation.release(now);
        self.service(now);
    }

    pub fn finish(&mut self, now: Instant) {
        let _ = self.dictation.stop(now);
        self.service(now);
    }

    pub fn hold_finished(&mut self, outcome: HoldOutcome, now: Instant) {
        match outcome {
            HoldOutcome::Released | HoldOutcome::TimedOut => {
                let _ = if matches!(outcome, HoldOutcome::TimedOut) {
                    self.dictation.stop(now)
                } else {
                    self.dictation.release(now)
                };
            }
            HoldOutcome::Cancelled(_) => {
                self.dictation
                    .interrupt(koru_dictation::Interruption::Suspended);
            }
        }
        self.service(now);
    }

    pub fn cancel(&mut self, why: koru_dictation::Interruption, now: Instant) {
        self.dictation.interrupt(why);
        self.service(now);
    }

    pub fn observe(&mut self, event: koru_dictation::Event) {
        self.dictation.observe(event);
    }

    pub fn service(&mut self, now: Instant) {
        for action in self.dictation.pump(now) {
            match action {
                koru_dictation::DictationAction::Begin { .. } => {
                    // Defensive answer for a future controller that queues a
                    // Begin: no command may leave one hanging.
                    self.dictation.began(false);
                }
                koru_dictation::DictationAction::Update { .. } => {
                    // Partials stay inside the shared semantic controller.
                }
                koru_dictation::DictationAction::Commit { text, .. } => {
                    let Some(target) = self.target.take() else {
                        self.snapshot.refusal =
                            Some(Refusal::Target(global_input::Refusal::TargetUnavailable));
                        continue;
                    };
                    match global_input::commit(&mut self.backend, &target, text.as_str()) {
                        Commit::Inserted => self.snapshot.outcome = Some(Outcome::Inserted),
                        Commit::DirectPartial => {
                            self.snapshot.refusal = Some(Refusal::DirectPartial)
                        }
                        Commit::Refused(reason) => {
                            self.snapshot.refusal = Some(Refusal::Target(reason))
                        }
                    }
                }
                koru_dictation::DictationAction::Cancel { .. } => {
                    self.target = None;
                    self.snapshot.outcome = Some(Outcome::Cancelled);
                }
            }
        }
        self.take_voice_refusal();
        if self.dictation.is_live() {
            self.snapshot.display = self
                .target
                .as_ref()
                .and_then(|target| global_input::target_display(&mut self.backend, target));
        } else {
            self.target = None;
            if self.snapshot.refusal.is_none() {
                self.snapshot.display = None;
            }
        }
        self.snapshot.phase = match self.dictation.stage() {
            koru_dictation::Stage::Idle => Phase::Idle,
            koru_dictation::Stage::Live => Phase::Listening,
            koru_dictation::Stage::Finalising => Phase::Finalising,
        };
        self.snapshot.level = self.dictation.level();
        self.publish();
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    // Read by the Windows `Handle`; the other platforms' handle answers `None`.
    #[cfg(windows)]
    fn deadline(&self) -> Option<Instant> {
        self.dictation.deadline()
    }

    fn take_voice_refusal(&mut self) {
        if let Some(refusal) = self.dictation.take_refusal() {
            self.snapshot.refusal = Some(Refusal::Voice(refusal.code));
        }
    }

    fn publish(&mut self) {
        if self.snapshot != self.published {
            self.published = self.snapshot;
            (self.notify)(self.snapshot);
        }
    }

    #[cfg(test)]
    fn live(&self) -> (koru_dictation::SessionId, u64) {
        (
            self.dictation.session().expect("test session"),
            self.dictation.generation().expect("test generation"),
        )
    }
}

#[cfg(windows)]
enum Command {
    Configure(Policy),
    PrepareTarget,
    StartPrepared,
    DiscardPrepared,
    Press(
        Option<crate::event::LocalDictationOwner>,
        global_input::VirtualKey,
    ),
    LocalState(crate::event::LocalDictationOwner, u64, LocalState),
    ShellGone(crate::registry::ConnectionId),
    HoldFinished(global_input::HoldFlight, HoldOutcome),
    Pump,
    Cancel(global_input::CancelReason),
    Finish,
    Shutdown,
}

#[cfg(any(windows, test))]
fn arm_pump_once(armed: &std::sync::atomic::AtomicBool, enqueue: impl FnOnce()) {
    use std::sync::atomic::Ordering;
    if !armed.swap(true, Ordering::AcqRel) {
        enqueue();
    }
}

#[cfg(any(windows, test))]
fn take_matching_flight(
    active: &mut Option<global_input::HoldFlight>,
    arrived: global_input::HoldFlight,
) -> bool {
    if *active != Some(arrived) {
        return false;
    }
    *active = None;
    true
}

#[cfg(windows)]
fn service_and_retire_idle_flight<B: InsertBackend>(
    coordinator: &mut Coordinator<B>,
    active_flight: &mut Option<global_input::HoldFlight>,
    observer: &mut Option<global_input::ReleaseObserver>,
    now: Instant,
) {
    coordinator.service(now);
    if coordinator.snapshot().phase == Phase::Idle
        && active_flight.take().is_some()
        && let Some(observer) = observer.take()
    {
        observer.shutdown();
    }
}

/// The Agent-facing half of the coordinator. Every potentially blocking
/// operation remains behind this command queue.
#[cfg(windows)]
pub struct Handle {
    sender: std::sync::mpsc::Sender<Command>,
    snapshot: std::sync::Arc<std::sync::Mutex<Snapshot>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl Handle {
    pub fn spawn(paths: crate::Paths, waker: crate::event::Waker) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        let snapshot = std::sync::Arc::new(std::sync::Mutex::new(Snapshot::default()));
        let worker_snapshot = std::sync::Arc::clone(&snapshot);
        let worker_sender = sender.clone();
        let pump_queued = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_pump_queued = std::sync::Arc::clone(&pump_queued);
        let worker_waker = waker.clone();
        let thread = std::thread::Builder::new()
            .name("koru-agent global dictation".into())
            .spawn(move || {
                run_windows(
                    paths,
                    worker_waker,
                    worker_snapshot,
                    worker_sender,
                    receiver,
                    worker_pump_queued,
                )
            })
            .ok();
        if thread.is_none() {
            publish_snapshot(
                &snapshot,
                &waker,
                Snapshot {
                    refusal: Some(Refusal::Target(global_input::Refusal::TargetUnavailable)),
                    ..Snapshot::default()
                },
            );
        }
        Self {
            sender,
            snapshot,
            thread,
        }
    }

    pub fn configure(&self, policy: Policy) {
        let _ = self.sender.send(Command::Configure(policy));
    }

    pub fn prepare_target(&self) {
        let _ = self.sender.send(Command::PrepareTarget);
    }

    pub fn start_prepared(&self) {
        let _ = self.sender.send(Command::StartPrepared);
    }

    pub fn discard_prepared(&self) {
        let _ = self.sender.send(Command::DiscardPrepared);
    }

    pub fn press(
        &self,
        local: Option<crate::event::LocalDictationOwner>,
        key: global_input::VirtualKey,
    ) {
        let _ = self.sender.send(Command::Press(local, key));
    }

    pub fn local_state(
        &self,
        owner: crate::event::LocalDictationOwner,
        route: u64,
        state: LocalState,
    ) {
        let _ = self.sender.send(Command::LocalState(owner, route, state));
    }

    pub fn shell_gone(&self, connection: crate::registry::ConnectionId) {
        let _ = self.sender.send(Command::ShellGone(connection));
    }

    pub fn cancel(&self, reason: global_input::CancelReason) {
        let _ = self.sender.send(Command::Cancel(reason));
    }

    pub fn finish(&self) {
        let _ = self.sender.send(Command::Finish);
    }

    pub fn snapshot(&self) -> Snapshot {
        *self
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = std::thread::Builder::new()
                .name("koru-agent global dictation reaper".into())
                .spawn(move || {
                    let _ = thread.join();
                });
        }
    }
}

#[cfg(windows)]
fn run_windows(
    paths: crate::Paths,
    waker: crate::event::Waker,
    snapshot: std::sync::Arc<std::sync::Mutex<Snapshot>>,
    sender: std::sync::mpsc::Sender<Command>,
    receiver: std::sync::mpsc::Receiver<Command>,
    pump_queued: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use crate::global_input::windows::WindowsKeyState;

    let mut pending_policy = Policy::default();
    let mut coordinator = None;
    let mut observer: Option<global_input::ReleaseObserver> = None;
    let mut next_flight = 0_u64;
    let mut active_flight = None;
    let mut active_local: Option<(
        u64,
        crate::event::LocalDictationOwner,
        global_input::VirtualKey,
        Option<global_input::HoldFlight>,
    )> = None;

    loop {
        let command = match coordinator.as_ref().and_then(Coordinator::deadline) {
            Some(deadline) => receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|error| match error {
                    std::sync::mpsc::RecvTimeoutError::Timeout => None,
                    std::sync::mpsc::RecvTimeoutError::Disconnected => Some(Command::Shutdown),
                }),
            None => receiver.recv().map_err(|_| Some(Command::Shutdown)),
        };
        let command = match command {
            Ok(command) => command,
            Err(None) => {
                if let Some(coordinator) = &mut coordinator {
                    service_and_retire_idle_flight(
                        coordinator,
                        &mut active_flight,
                        &mut observer,
                        Instant::now(),
                    );
                }
                continue;
            }
            Err(Some(command)) => command,
        };

        match command {
            Command::Configure(next) => {
                if let Some(coordinator) = &mut coordinator {
                    if !next.enabled || next.consent.is_none() {
                        active_flight = None;
                        if let Some(observer) = observer.take() {
                            observer.shutdown();
                        }
                    }
                    coordinator.configure(next, Instant::now());
                } else {
                    pending_policy = next;
                }
            }
            Command::PrepareTarget => {
                if coordinator.is_none() {
                    coordinator = make_windows_coordinator(
                        &paths,
                        &pending_policy,
                        &snapshot,
                        &waker,
                        &sender,
                        &pump_queued,
                    );
                }
                if let Some(coordinator) = &mut coordinator {
                    let _ = coordinator.prepare_target();
                } else {
                    publish_snapshot(
                        &snapshot,
                        &waker,
                        Snapshot {
                            refusal: Some(Refusal::Target(
                                global_input::Refusal::TargetUnavailable,
                            )),
                            ..Snapshot::default()
                        },
                    );
                }
            }
            Command::StartPrepared => {
                if let Some(coordinator) = &mut coordinator {
                    let _ = coordinator.start_prepared(Instant::now());
                } else {
                    publish_snapshot(
                        &snapshot,
                        &waker,
                        Snapshot {
                            refusal: Some(Refusal::Target(
                                global_input::Refusal::TargetUnavailable,
                            )),
                            ..Snapshot::default()
                        },
                    );
                }
            }
            Command::DiscardPrepared => {
                if let Some(coordinator) = &mut coordinator {
                    coordinator.discard_prepared();
                }
            }
            Command::Press(proposed_local, key) => {
                if let Some((route, owner, owned_key, _)) = active_local {
                    waker.wake(crate::event::AgentEvent::LocalDictation(
                        crate::event::LocalDictationGesture::Press {
                            route,
                            owner,
                            key: owned_key,
                        },
                    ));
                    continue;
                }
                // A live global session keeps ownership even if focus moved to
                // a Koru window before its toggle-finishing press.
                if coordinator
                    .as_ref()
                    .is_some_and(|value| value.dictation.is_live())
                {
                    let coordinator = coordinator.as_mut().expect("checked above");
                    let _ = coordinator.press_with_release_ready(Instant::now(), true);
                    continue;
                }
                if let Some(owner) = proposed_local {
                    // Preflight release observation before the receiving local
                    // controller can open Voice. Toggle will retire it after
                    // the correlated Started answer without ever polling.
                    let Ok(source) = WindowsKeyState::new() else {
                        continue;
                    };
                    let hold_sender = sender.clone();
                    observer = global_input::ReleaseObserver::spawn(source, move |notice| {
                        if let global_input::HoldNotice::Finished { flight, outcome } = notice {
                            let _ = hold_sender.send(Command::HoldFinished(flight, outcome));
                        }
                    })
                    .ok();
                    if observer.is_none() {
                        continue;
                    }
                    next_flight = next_flight.wrapping_add(1);
                    let route = next_flight;
                    active_local = Some((route, owner, key, None));
                    waker.wake(crate::event::AgentEvent::LocalDictation(
                        crate::event::LocalDictationGesture::Press { route, owner, key },
                    ));
                    continue;
                }
                let policy = coordinator
                    .as_ref()
                    .map(|coordinator| &coordinator.policy)
                    .unwrap_or(&pending_policy);
                if !policy.enabled {
                    publish_snapshot(
                        &snapshot,
                        &waker,
                        Snapshot {
                            refusal: Some(Refusal::Disabled),
                            ..Snapshot::default()
                        },
                    );
                    continue;
                }
                if policy.consent.is_none() {
                    publish_snapshot(
                        &snapshot,
                        &waker,
                        Snapshot {
                            refusal: Some(Refusal::Voice(
                                koru_dictation::ErrorCode::ConsentRequired,
                            )),
                            ..Snapshot::default()
                        },
                    );
                    continue;
                }
                let push_to_talk = policy.preferences.mode == koru_dictation::Mode::PushToTalk;
                if push_to_talk && observer.is_none() {
                    let Ok(source) = WindowsKeyState::new() else {
                        if let Some(coordinator) = &mut coordinator {
                            let _ = coordinator.press_with_release_ready(Instant::now(), false);
                        } else {
                            publish_snapshot(
                                &snapshot,
                                &waker,
                                Snapshot {
                                    refusal: Some(Refusal::Target(
                                        global_input::Refusal::TargetUnavailable,
                                    )),
                                    ..Snapshot::default()
                                },
                            );
                        }
                        continue;
                    };
                    let hold_sender = sender.clone();
                    observer = global_input::ReleaseObserver::spawn(source, move |notice| {
                        if let global_input::HoldNotice::Finished { flight, outcome } = notice {
                            let _ = hold_sender.send(Command::HoldFinished(flight, outcome));
                        }
                    })
                    .ok();
                    if observer.is_none() {
                        if let Some(coordinator) = &mut coordinator {
                            let _ = coordinator.press_with_release_ready(Instant::now(), false);
                        } else {
                            publish_snapshot(
                                &snapshot,
                                &waker,
                                Snapshot {
                                    refusal: Some(Refusal::Target(
                                        global_input::Refusal::TargetUnavailable,
                                    )),
                                    ..Snapshot::default()
                                },
                            );
                        }
                        continue;
                    }
                }
                if coordinator.is_none() {
                    coordinator = make_windows_coordinator(
                        &paths,
                        &pending_policy,
                        &snapshot,
                        &waker,
                        &sender,
                        &pump_queued,
                    );
                    if coordinator.is_none() {
                        publish_snapshot(
                            &snapshot,
                            &waker,
                            Snapshot {
                                refusal: Some(Refusal::Target(
                                    global_input::Refusal::TargetUnavailable,
                                )),
                                ..Snapshot::default()
                            },
                        );
                        continue;
                    }
                }
                let coordinator = coordinator.as_mut().expect("constructed above");
                let result = coordinator
                    .press_with_release_ready(Instant::now(), !push_to_talk || observer.is_some());
                if result == PressResult::Started(koru_dictation::Mode::PushToTalk) {
                    next_flight = next_flight.wrapping_add(1);
                    let flight = global_input::HoldFlight(next_flight);
                    active_flight = Some(flight);
                    observer
                        .as_ref()
                        .expect("push-to-talk was preflighted")
                        .press(key, flight);
                } else if push_to_talk
                    && !coordinator.dictation.is_live()
                    && let Some(observer) = observer.take()
                {
                    observer.shutdown();
                }
            }
            Command::HoldFinished(flight, outcome) => {
                if let Some((route, owner, key, local_flight)) = active_local
                    && local_flight == Some(flight)
                {
                    if matches!(outcome, HoldOutcome::Released | HoldOutcome::TimedOut) {
                        waker.wake(crate::event::AgentEvent::LocalDictation(
                            crate::event::LocalDictationGesture::Release { route, owner, key },
                        ));
                        active_local = Some((route, owner, key, None));
                    } else {
                        waker.wake(crate::event::AgentEvent::LocalDictation(
                            crate::event::LocalDictationGesture::Cancel { route, owner, key },
                        ));
                        active_local = None;
                    }
                    if let Some(observer) = observer.take() {
                        observer.shutdown();
                    }
                    continue;
                }
                if take_matching_flight(&mut active_flight, flight)
                    && let Some(coordinator) = &mut coordinator
                {
                    coordinator.hold_finished(outcome, Instant::now());
                }
            }
            Command::Pump => {
                use std::sync::atomic::Ordering;
                pump_queued.store(false, Ordering::Release);
                if let Some(coordinator) = &mut coordinator {
                    service_and_retire_idle_flight(
                        coordinator,
                        &mut active_flight,
                        &mut observer,
                        Instant::now(),
                    );
                }
            }
            Command::LocalState(owner, route, state) => {
                let Some((current_route, current_owner, key, flight)) = active_local else {
                    continue;
                };
                if current_route != route || current_owner != owner {
                    continue;
                }
                match state {
                    LocalState::Refused | LocalState::Idle => {
                        active_local = None;
                        if let Some(observer) = observer.take() {
                            observer.shutdown();
                        }
                    }
                    LocalState::Listening(koru_dictation::Mode::Toggle) => {
                        active_local = Some((route, owner, key, None));
                        if let Some(observer) = observer.take() {
                            observer.shutdown();
                        }
                    }
                    LocalState::Listening(koru_dictation::Mode::PushToTalk) => {
                        if flight.is_none() {
                            next_flight = next_flight.wrapping_add(1);
                            let next = global_input::HoldFlight(next_flight);
                            active_local = Some((route, owner, key, Some(next)));
                            if let Some(observer) = &observer {
                                observer.press(key, next);
                            }
                        }
                    }
                    LocalState::Finalising => {
                        active_local = Some((route, owner, key, None));
                        if let Some(observer) = observer.take() {
                            observer.shutdown();
                        }
                    }
                }
            }
            Command::ShellGone(connection) => {
                if matches!(active_local, Some((_, crate::event::LocalDictationOwner::Workspace(owner), _, _)) if owner == connection)
                {
                    active_local = None;
                    if let Some(observer) = observer.take() {
                        observer.shutdown();
                    }
                }
            }
            Command::Cancel(reason) => {
                let _ = reason;
                if let Some((route, owner, key, _)) = active_local.take() {
                    waker.wake(crate::event::AgentEvent::LocalDictation(
                        crate::event::LocalDictationGesture::Cancel { route, owner, key },
                    ));
                }
                active_flight = None;
                if let Some(observer) = observer.take() {
                    observer.shutdown();
                }
                if let Some(coordinator) = &mut coordinator {
                    coordinator.cancel(koru_dictation::Interruption::Suspended, Instant::now());
                }
            }
            Command::Finish => {
                active_flight = None;
                if let Some(observer) = observer.take() {
                    observer.shutdown();
                }
                if let Some(coordinator) = &mut coordinator {
                    coordinator.finish(Instant::now());
                }
            }
            Command::Shutdown => {
                if let Some((route, owner, key, _)) = active_local.take() {
                    waker.wake(crate::event::AgentEvent::LocalDictation(
                        crate::event::LocalDictationGesture::Cancel { route, owner, key },
                    ));
                }
                if let Some(observer) = observer.take() {
                    observer.shutdown();
                }
                if let Some(coordinator) = &mut coordinator {
                    coordinator.cancel(koru_dictation::Interruption::Suspended, Instant::now());
                }
                return;
            }
        }
    }
}

#[cfg(windows)]
fn publish_snapshot(
    shared: &std::sync::Mutex<Snapshot>,
    waker: &crate::event::Waker,
    next: Snapshot,
) {
    let changed = {
        let mut current = shared.lock().unwrap_or_else(|error| error.into_inner());
        if *current == next {
            false
        } else {
            *current = next;
            true
        }
    };
    if changed {
        waker.wake(crate::event::AgentEvent::GlobalDictationChanged(next));
    }
}

#[cfg(windows)]
fn make_windows_coordinator(
    paths: &crate::Paths,
    policy: &Policy,
    snapshot: &std::sync::Arc<std::sync::Mutex<Snapshot>>,
    waker: &crate::event::Waker,
    sender: &std::sync::mpsc::Sender<Command>,
    pump_queued: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Option<Coordinator<crate::global_input::windows::WindowsInsertBackend>> {
    let backend = crate::global_input::windows::WindowsInsertBackend::new().ok()?;
    let client_paths = paths.clone();
    let wake_sender = sender.clone();
    let wake_armed = std::sync::Arc::clone(pump_queued);
    let mut made = Coordinator::new(
        koru_dictation::Dictation::at(Some(crate::service::voice_service_root(paths))),
        backend,
        {
            let snapshot = std::sync::Arc::clone(snapshot);
            let waker = waker.clone();
            move |next| publish_snapshot(&snapshot, &waker, next)
        },
    );
    made.client().install_wake(move || {
        arm_pump_once(&wake_armed, || {
            let _ = wake_sender.send(Command::Pump);
        });
    });
    made.client().install_ensure(move || {
        let _ = crate::service::ensure_voice(&client_paths);
    });
    made.configure(policy.clone(), Instant::now());
    Some(made)
}

#[cfg(not(windows))]
pub struct Handle;

#[cfg(not(windows))]
impl Handle {
    pub fn spawn(_paths: crate::Paths, _waker: crate::event::Waker) -> Self {
        Self
    }

    pub fn configure(&self, _policy: Policy) {}
    pub fn prepare_target(&self) {}
    pub fn start_prepared(&self) {}
    pub fn discard_prepared(&self) {}
    pub fn press(
        &self,
        _local: Option<crate::event::LocalDictationOwner>,
        _key: global_input::VirtualKey,
    ) {
    }
    pub fn local_state(
        &self,
        _owner: crate::event::LocalDictationOwner,
        _route: u64,
        _state: LocalState,
    ) {
    }
    pub fn shell_gone(&self, _connection: crate::registry::ConnectionId) {}
    pub fn cancel(&self, _reason: global_input::CancelReason) {}
    pub fn finish(&self) {}
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::default()
    }
}

#[cfg(test)]
mod tests;
