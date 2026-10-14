// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use super::*;

#[derive(Clone)]
struct FakeKeySource {
    states: Arc<Mutex<VecDeque<KeyState>>>,
    calls: Arc<Mutex<Vec<VirtualKey>>>,
    default: KeyState,
}

impl FakeKeySource {
    fn new(states: impl IntoIterator<Item = KeyState>, default: KeyState) -> Self {
        Self {
            states: Arc::new(Mutex::new(states.into_iter().collect())),
            calls: Arc::new(Mutex::new(Vec::new())),
            default,
        }
    }
}

impl KeyStateSource for FakeKeySource {
    fn state(&mut self, key: VirtualKey) -> KeyState {
        self.calls.lock().unwrap().push(key);
        self.states
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(self.default)
    }
}

fn config() -> ObserverConfig {
    ObserverConfig {
        poll_interval: Duration::from_millis(2),
        max_hold: Duration::from_millis(40),
        resume_gap: Duration::from_secs(1),
    }
}

const FLIGHT: HoldFlight = HoldFlight(7);

fn observer(
    source: FakeKeySource,
) -> (
    ReleaseObserver,
    mpsc::Receiver<(HoldNotice, std::thread::ThreadId, Option<String>)>,
) {
    let (send, receive) = mpsc::channel();
    let observer = ReleaseObserver::spawn_with_config(source, config(), move |notice| {
        let thread = std::thread::current();
        let _ = send.send((notice, thread.id(), thread.name().map(str::to_owned)));
    })
    .unwrap();
    (observer, receive)
}

#[test]
fn press_repeat_and_release_are_one_typed_hold() {
    let source = FakeKeySource::new([KeyState::Down, KeyState::Up], KeyState::Up);
    let calls = Arc::clone(&source.calls);
    let (observer, notices) = observer(source);
    let key = VirtualKey::new(b'D' as u16);
    observer.press(key, FLIGHT);
    observer.press(key, FLIGHT);
    assert_eq!(
        notices.recv_timeout(Duration::from_secs(1)).unwrap().0,
        HoldNotice::Pressed { flight: FLIGHT }
    );
    assert_eq!(
        notices.recv_timeout(Duration::from_secs(1)).unwrap().0,
        HoldNotice::Finished {
            flight: FLIGHT,
            outcome: HoldOutcome::Released,
        }
    );
    assert!(notices.recv_timeout(Duration::from_millis(20)).is_err());
    assert!(calls.lock().unwrap().iter().all(|seen| *seen == key));
}

#[test]
fn a_release_that_never_arrives_times_out() {
    let source = FakeKeySource::new([], KeyState::Down);
    let (observer, notices) = observer(source);
    observer.press(VirtualKey::new(b'D' as u16), FLIGHT);
    let _ = notices.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        notices.recv_timeout(Duration::from_secs(1)).unwrap().0,
        HoldNotice::Finished {
            flight: FLIGHT,
            outcome: HoldOutcome::TimedOut,
        }
    );
}

#[test]
fn secure_desktop_never_turns_an_unavailable_zero_into_release() {
    let source = FakeKeySource::new([KeyState::Unavailable], KeyState::Up);
    let (observer, notices) = observer(source);
    observer.press(VirtualKey::new(b'D' as u16), FLIGHT);
    let _ = notices.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        notices.recv_timeout(Duration::from_secs(1)).unwrap().0,
        HoldNotice::Finished {
            flight: FLIGHT,
            outcome: HoldOutcome::Cancelled(CancelReason::SecureDesktop),
        }
    );
}

#[test]
fn explicit_cancel_resume_and_shutdown_have_distinct_outcomes() {
    for (act, expected) in [
        (0, CancelReason::Explicit),
        (1, CancelReason::Resume),
        (2, CancelReason::Shutdown),
    ] {
        let source = FakeKeySource::new([], KeyState::Down);
        let (observer, notices) = observer(source);
        observer.press(VirtualKey::new(b'D' as u16), FLIGHT);
        let _ = notices.recv_timeout(Duration::from_secs(1)).unwrap();
        match act {
            0 => observer.cancel(),
            1 => observer.resumed(),
            _ => observer.shutdown(),
        }
        assert_eq!(
            notices.recv_timeout(Duration::from_secs(1)).unwrap().0,
            HoldNotice::Finished {
                flight: FLIGHT,
                outcome: HoldOutcome::Cancelled(expected),
            }
        );
    }
}

#[test]
fn a_sleep_sized_sampling_gap_cancels_without_reading_a_key() {
    let source = FakeKeySource::new([], KeyState::Down);
    let calls = Arc::clone(&source.calls);
    let (send, receive) = mpsc::channel();
    let observer = ReleaseObserver::spawn_with_config(
        source,
        ObserverConfig {
            poll_interval: Duration::from_millis(20),
            max_hold: Duration::from_secs(1),
            resume_gap: Duration::from_millis(5),
        },
        move |notice| {
            let _ = send.send(notice);
        },
    )
    .unwrap();
    observer.press(VirtualKey::new(b'D' as u16), FLIGHT);
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(1)).unwrap(),
        HoldNotice::Pressed { flight: FLIGHT }
    );
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(1)).unwrap(),
        HoldNotice::Finished {
            flight: FLIGHT,
            outcome: HoldOutcome::Cancelled(CancelReason::Resume),
        }
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn the_observer_is_idle_without_work_and_callbacks_run_on_its_thread() {
    let source = FakeKeySource::new([KeyState::Up], KeyState::Up);
    let calls = Arc::clone(&source.calls);
    let event_thread = std::thread::current().id();
    let (observer, notices) = observer(source);
    std::thread::sleep(Duration::from_millis(15));
    assert!(
        calls.lock().unwrap().is_empty(),
        "idle means no key-state call"
    );
    observer.press(VirtualKey::new(b'D' as u16), FLIGHT);
    let (notice, callback_thread, name) = notices.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(notice, HoldNotice::Pressed { flight: FLIGHT });
    assert_ne!(callback_thread, event_thread);
    assert_eq!(name.as_deref(), Some("koru-agent dictation release"));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FakeTarget(&'static str);

struct FakeInsert {
    target: FakeTarget,
    states: VecDeque<TargetState>,
    direct: DirectInsertion,
    inserted: Vec<String>,
    actions: Vec<&'static str>,
}

impl FakeInsert {
    fn direct(result: DirectInsertion) -> Self {
        Self {
            target: FakeTarget("same-control"),
            states: VecDeque::new(),
            direct: result,
            inserted: Vec::new(),
            actions: Vec::new(),
        }
    }

    fn state(mut self, states: impl IntoIterator<Item = TargetState>) -> Self {
        self.states = states.into_iter().collect();
        self
    }
}

impl InsertBackend for FakeInsert {
    type Target = FakeTarget;
    type RestoreHint = ();

    fn capture_restore_hint(&mut self) -> Result<Self::RestoreHint, Refusal> {
        self.actions.push("capture_restore_hint");
        Ok(())
    }

    fn restore_hint(&mut self, _hint: &Self::RestoreHint) -> Result<(), Refusal> {
        self.actions.push("restore_hint");
        Ok(())
    }

    fn capture_target(&mut self) -> Result<Self::Target, Refusal> {
        self.actions.push("capture_target");
        Ok(self.target.clone())
    }

    fn capture_target_for_hint(
        &mut self,
        _hint: &Self::RestoreHint,
    ) -> Result<Self::Target, Refusal> {
        self.capture_target()
    }

    fn target_state(&mut self, _target: &Self::Target) -> TargetState {
        self.actions.push("target_state");
        self.states.pop_front().unwrap_or(TargetState::Editable)
    }

    fn insert_unicode(&mut self, _target: &Self::Target, text: &str) -> DirectInsertion {
        self.actions.push("direct");
        // Only the two outcomes that mean `SendInput` actually delivered
        // something leave text behind. `Rejected` is a zero count, and
        // `TargetUnavailable` is the adapter's own re-proof failing *before*
        // any input event is built — modelling either as "the text went in"
        // would make every assertion about refusals below vacuous.
        if matches!(
            self.direct,
            DirectInsertion::Complete | DirectInsertion::Partial
        ) {
            self.inserted.push(text.to_owned());
        }
        self.direct
    }
}

#[test]
fn the_platforms_own_re_proof_at_the_point_of_consequence_is_a_separate_refusal() {
    // **There are two checks, and this is the second one.** `commit` verifies
    // the target, and then the Windows adapter re-proves the exact element
    // again inside `insert_unicode`, immediately before `SendInput` — because
    // focus, the element and the input desktop can all change between the
    // policy's question and the platform's consequence.
    //
    // The fake stands for a target that was editable when `commit` asked and
    // was not by the time the platform was about to type into it. Nothing is
    // inserted, and the outcome is its own refusal rather than the rejected or
    // partial counts that mean `SendInput` actually ran.
    let mut backend = FakeInsert::direct(DirectInsertion::TargetUnavailable);
    let target = capture(&mut backend).unwrap();
    assert_eq!(
        commit(&mut backend, &target, "private"),
        Commit::Refused(Refusal::TargetUnavailable)
    );
    assert!(backend.inserted.is_empty());
    assert_eq!(
        backend.actions.last(),
        Some(&"direct"),
        "the platform must have been reached and must have refused there"
    );
}

#[test]
fn a_target_that_went_stale_after_a_successful_restore_is_still_refused_at_commit() {
    // The window between restoring focus and typing is real: Voice teardown,
    // the person clicking somewhere, the application closing its own dialog.
    // A restore that succeeded is not a licence to skip the commit check.
    for (gone, refusal) in [
        (TargetState::Destroyed, Refusal::TargetDestroyed),
        (TargetState::Changed, Refusal::TargetChanged),
        (TargetState::Unavailable, Refusal::TargetUnavailable),
        (TargetState::Protected, Refusal::ProtectedTarget),
    ] {
        let mut backend = FakeInsert::direct(DirectInsertion::Complete);
        let target = capture(&mut backend).unwrap();
        assert_eq!(restore(&mut backend, &target), Ok(()));
        backend.states.push_back(gone);
        assert_eq!(
            commit(&mut backend, &target, "private"),
            Commit::Refused(refusal),
            "{gone:?} after a good restore"
        );
        assert!(backend.inserted.is_empty());
        assert_ne!(
            backend.actions.last(),
            Some(&"direct"),
            "{gone:?} must be refused before the platform is reached at all"
        );
    }
}

#[test]
fn unicode_direct_insertion_preserves_mixed_direction_emoji_and_lines() {
    let text = "English العربية 中文 😀\nsecond line";
    let mut backend = FakeInsert::direct(DirectInsertion::Complete);
    let target = capture(&mut backend).unwrap();
    assert_eq!(commit(&mut backend, &target, text), Commit::Inserted);
    assert_eq!(backend.inserted, [text]);
}

#[test]
fn capture_and_commit_fail_closed_for_protected_changed_destroyed_and_unknown_targets() {
    for state in [
        TargetState::Protected,
        TargetState::Changed,
        TargetState::Destroyed,
        TargetState::Unavailable,
    ] {
        let mut backend = FakeInsert::direct(DirectInsertion::Complete).state([state]);
        assert!(capture(&mut backend).is_err());
    }

    for (state, refusal) in [
        (TargetState::Protected, Refusal::ProtectedTarget),
        (TargetState::Changed, Refusal::TargetChanged),
        (TargetState::Destroyed, Refusal::TargetDestroyed),
        (TargetState::Unavailable, Refusal::TargetUnavailable),
    ] {
        let mut backend = FakeInsert::direct(DirectInsertion::Complete);
        let target = capture(&mut backend).unwrap();
        backend.states.push_back(state);
        assert_eq!(
            commit(&mut backend, &target, "private"),
            Commit::Refused(refusal)
        );
        assert!(backend.inserted.is_empty());
    }
}

#[test]
fn a_different_control_inside_the_same_window_is_still_a_target_change() {
    let mut backend = FakeInsert::direct(DirectInsertion::Complete);
    let target = capture(&mut backend).unwrap();
    backend.states.push_back(TargetState::Changed);
    assert_eq!(
        commit(&mut backend, &target, "not inserted"),
        Commit::Refused(Refusal::TargetChanged)
    );
}

#[test]
fn zero_direct_insertion_refuses_with_no_second_mechanism() {
    let mut backend = FakeInsert::direct(DirectInsertion::Rejected);
    let target = capture(&mut backend).unwrap();
    assert_eq!(
        commit(&mut backend, &target, "private"),
        Commit::Refused(Refusal::ClipboardCompletionUnprovable)
    );
    assert!(backend.inserted.is_empty());
    assert_eq!(backend.actions.last(), Some(&"direct"));
}

#[test]
fn partial_direct_injection_never_falls_back() {
    let mut backend = FakeInsert::direct(DirectInsertion::Partial);
    let target = capture(&mut backend).unwrap();
    assert_eq!(
        commit(&mut backend, &target, "once only"),
        Commit::DirectPartial
    );
    assert_eq!(backend.actions.last(), Some(&"direct"));
}

#[test]
fn target_debug_never_reveals_identity() {
    const SENTINEL: &str = "KORU-PRIVATE-TARGET";
    let mut backend = FakeInsert::direct(DirectInsertion::Complete);
    backend.target = FakeTarget(SENTINEL);
    let target = capture(&mut backend).unwrap();
    assert!(!format!("{target:?}").contains(SENTINEL));
}

#[test]
fn press_registration_is_fakeable_without_owning_an_os_singleton() {
    #[derive(Default)]
    struct Registrar(Vec<(i32, &'static str)>);
    impl PressRegistrar for Registrar {
        type Chord = &'static str;
        type Registration = usize;

        fn register(
            &mut self,
            action: i32,
            chord: Self::Chord,
        ) -> Result<Self::Registration, PressError> {
            self.0.push((action, chord));
            Ok(self.0.len())
        }
    }
    let mut registrar = Registrar::default();
    assert_eq!(registrar.register(7, "Ctrl+Shift+D"), Ok(1));
    assert_eq!(registrar.0, [(7, "Ctrl+Shift+D")]);
}
