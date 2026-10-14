// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::*;
use crate::global_input::{DirectInsertion, Refusal as TargetRefusal, TargetState};

struct FakeInsert {
    hint_captures: usize,
    hint_restore_calls: usize,
    hint_refusal: Option<TargetRefusal>,
    hint_restore_refusal: Option<TargetRefusal>,
    captures: usize,
    capture_refusal: Option<TargetRefusal>,
    hinted_capture_refusal: Option<TargetRefusal>,
    states: VecDeque<TargetState>,
    direct: DirectInsertion,
    insert_calls: usize,
    inserted: Vec<String>,
}

impl Default for FakeInsert {
    fn default() -> Self {
        Self {
            captures: 0,
            hint_captures: 0,
            hint_restore_calls: 0,
            hint_refusal: None,
            hint_restore_refusal: None,
            capture_refusal: None,
            hinted_capture_refusal: None,
            states: VecDeque::new(),
            direct: DirectInsertion::Complete,
            insert_calls: 0,
            inserted: Vec::new(),
        }
    }
}

impl InsertBackend for FakeInsert {
    type Target = u8;
    type RestoreHint = u8;

    fn capture_restore_hint(&mut self) -> Result<Self::RestoreHint, TargetRefusal> {
        self.hint_captures += 1;
        self.hint_refusal.take().map_or(Ok(7), Err)
    }

    fn restore_hint(&mut self, _hint: &Self::RestoreHint) -> Result<(), TargetRefusal> {
        self.hint_restore_calls += 1;
        self.hint_restore_refusal.take().map_or(Ok(()), Err)
    }

    fn capture_target(&mut self) -> Result<Self::Target, TargetRefusal> {
        self.captures += 1;
        self.capture_refusal.take().map_or(Ok(1), Err)
    }

    fn capture_target_for_hint(
        &mut self,
        _hint: &Self::RestoreHint,
    ) -> Result<Self::Target, TargetRefusal> {
        self.captures += 1;
        self.hinted_capture_refusal.take().map_or(Ok(1), Err)
    }

    fn target_state(&mut self, _target: &Self::Target) -> TargetState {
        self.states.pop_front().unwrap_or(TargetState::Editable)
    }

    fn current_display(&mut self) -> Option<crate::global_input::TargetDisplay> {
        Some(crate::global_input::TargetDisplay {
            work_area: (-1920, 0, 0, 1080),
            dpi: 96,
        })
    }

    fn target_display(
        &mut self,
        _target: &Self::Target,
    ) -> Option<crate::global_input::TargetDisplay> {
        Some(crate::global_input::TargetDisplay {
            work_area: (-2560, 40, 0, 1400),
            dpi: 144,
        })
    }

    fn insert_unicode(&mut self, _target: &Self::Target, text: &str) -> DirectInsertion {
        self.insert_calls += 1;
        if self.direct == DirectInsertion::Complete {
            self.inserted.push(text.to_owned());
        }
        self.direct
    }
}

#[test]
fn restore_hint_prepare_is_content_free_and_policy_is_checked_only_at_start() {
    let published = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&published);
    let mut coordinator = Coordinator::new(
        koru_dictation::Dictation::new(),
        FakeInsert::default(),
        move |snapshot| seen.lock().unwrap().push(snapshot),
    );

    assert!(matches!(
        coordinator.prepare_target(),
        PressResult::Started(_)
    ));
    assert_eq!(coordinator.backend.hint_captures, 1);
    assert_eq!(
        coordinator.backend.captures, 0,
        "prepare must never use UIA"
    );
    assert!(coordinator.snapshot().prepared);
    assert_eq!(
        coordinator.snapshot().display,
        Some(crate::global_input::TargetDisplay {
            work_area: (-1920, 0, 0, 1080),
            dpi: 96,
        })
    );
    assert_eq!(
        coordinator.start_prepared(Instant::now()),
        PressResult::Refused
    );
    assert_eq!(coordinator.backend.hint_restore_calls, 0);
    assert_eq!(coordinator.backend.captures, 0);
    assert!(!coordinator.snapshot().prepared, "start consumes the hint");
}

#[test]
fn live_snapshot_replaces_attempt_geometry_with_the_exact_target_display() {
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
    let now = Instant::now();
    assert!(matches!(coordinator.press(now), PressResult::Started(_)));
    assert_eq!(
        coordinator.snapshot().display,
        Some(crate::global_input::TargetDisplay {
            work_area: (-2560, 40, 0, 1400),
            dpi: 144,
        })
    );
}

#[test]
fn unchanged_policy_and_grants_retain_hint_but_actual_revocations_discard_it() {
    let published = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&published);
    let mut coordinator = Coordinator::new(
        koru_dictation::Dictation::new(),
        FakeInsert::default(),
        move |snapshot| seen.lock().unwrap().push(snapshot),
    );
    let now = Instant::now();
    coordinator.prepare_target();
    coordinator.configure(Policy::default(), now);
    assert!(coordinator.snapshot().prepared);
    coordinator.configure(
        Policy {
            enabled: true,
            consent: Some(grant()),
            ..Policy::default()
        },
        now,
    );
    assert_eq!(
        coordinator.start_prepared(now),
        PressResult::Started(koru_dictation::Mode::PushToTalk)
    );
    assert_eq!(coordinator.backend.hint_restore_calls, 1);
    assert_eq!(coordinator.backend.captures, 1);

    coordinator.cancel(koru_dictation::Interruption::Suspended, now);
    coordinator.prepare_target();
    coordinator.configure(
        Policy {
            enabled: false,
            consent: Some(grant()),
            ..Policy::default()
        },
        now,
    );
    assert!(!coordinator.snapshot().prepared);

    coordinator.configure(
        Policy {
            enabled: true,
            consent: Some(grant()),
            ..Policy::default()
        },
        now,
    );
    coordinator.prepare_target();
    coordinator.configure(
        Policy {
            enabled: true,
            consent: None,
            ..Policy::default()
        },
        now,
    );
    assert!(!coordinator.snapshot().prepared);
}

#[test]
fn restore_and_fresh_accessibility_failures_refuse_before_voice_begins() {
    for refusal in [
        TargetRefusal::TargetDestroyed,
        TargetRefusal::TargetChanged,
        TargetRefusal::TargetUnavailable,
    ] {
        let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
        coordinator.prepare_target();
        coordinator.backend.hint_restore_refusal = Some(refusal);
        assert_eq!(
            coordinator.start_prepared(Instant::now()),
            PressResult::Refused
        );
        assert_eq!(coordinator.backend.captures, 0);
        assert_eq!(
            coordinator.snapshot().refusal,
            Some(Refusal::Target(refusal))
        );
    }
    for (state, refusal) in [
        (TargetState::Protected, TargetRefusal::ProtectedTarget),
        (TargetState::Changed, TargetRefusal::TargetChanged),
        (TargetState::Destroyed, TargetRefusal::TargetDestroyed),
        (TargetState::Unavailable, TargetRefusal::TargetUnavailable),
    ] {
        let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
        coordinator.prepare_target();
        coordinator.backend.states.push_back(state);
        assert_eq!(
            coordinator.start_prepared(Instant::now()),
            PressResult::Refused
        );
        assert_eq!(
            coordinator.snapshot().refusal,
            Some(Refusal::Target(refusal))
        );
        assert!(coordinator.target.is_none());
    }

    let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
    coordinator.prepare_target();
    coordinator.backend.hinted_capture_refusal = Some(TargetRefusal::TargetChanged);
    assert_eq!(
        coordinator.start_prepared(Instant::now()),
        PressResult::Refused
    );
    assert_eq!(
        coordinator.snapshot().refusal,
        Some(Refusal::Target(TargetRefusal::TargetChanged))
    );
    assert!(coordinator.target.is_none());
}

#[test]
fn prepared_hint_is_one_shot_replaceable_discardable_and_redacted() {
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
    coordinator.prepare_target();
    coordinator.prepare_target();
    assert_eq!(coordinator.backend.hint_captures, 2);
    coordinator.discard_prepared();
    assert_eq!(
        coordinator.start_prepared(Instant::now()),
        PressResult::Refused
    );
    let hint = global_input::capture_restore_hint(&mut coordinator.backend).unwrap();
    assert_eq!(format!("{hint:?}"), "CapturedRestoreHint([REDACTED])");
}

fn grant() -> koru_voice_consent::Granted {
    koru_voice_consent::Granted::recorded(
        koru_voice_consent::Terms {
            model: "model".into(),
            version: "1".into(),
            licence: "Apache-2.0".into(),
            attribution: "test".into(),
            download_bytes: 1,
        },
        1,
    )
}

fn coordinator(mode: koru_dictation::Mode) -> (Coordinator<FakeInsert>, Arc<Mutex<Vec<Snapshot>>>) {
    assert!(koru_dictation::ENGINE_OFF, "tests must never dial Voice");
    let published = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&published);
    let mut coordinator = Coordinator::new(
        koru_dictation::Dictation::new(),
        FakeInsert::default(),
        move |snapshot| seen.lock().unwrap().push(snapshot),
    );
    coordinator.configure(
        Policy {
            enabled: true,
            preferences: koru_dictation::Preferences {
                mode,
                ..koru_dictation::Preferences::default()
            },
            consent: Some(grant()),
        },
        Instant::now(),
    );
    (coordinator, published)
}

fn final_event(coordinator: &Coordinator<FakeInsert>, text: &str) -> koru_dictation::Event {
    let (session, generation) = coordinator.live();
    koru_dictation::Event::Final {
        session,
        generation,
        text: koru_dictation::Transcript::new(text),
    }
}

fn ended_event(
    coordinator: &Coordinator<FakeInsert>,
    reason: koru_dictation::EndReason,
) -> koru_dictation::Event {
    let (session, generation) = coordinator.live();
    koru_dictation::Event::Ended {
        session,
        generation,
        reason,
    }
}

fn complete(coordinator: &mut Coordinator<FakeInsert>, now: Instant, text: &str) {
    let final_event = final_event(coordinator, text);
    let ended = ended_event(coordinator, koru_dictation::EndReason::Completed);
    coordinator.observe(final_event);
    coordinator.observe(ended);
    coordinator.service(now);
}

#[test]
fn disabled_and_missing_consent_are_typed_and_open_no_session() {
    let published = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&published);
    let mut disabled = Coordinator::new(
        koru_dictation::Dictation::new(),
        FakeInsert::default(),
        move |snapshot| seen.lock().unwrap().push(snapshot),
    );
    assert_eq!(disabled.press(Instant::now()), PressResult::Refused);
    assert_eq!(disabled.snapshot().refusal, Some(Refusal::Disabled));
    assert_eq!(disabled.backend.captures, 0);

    disabled.configure(
        Policy {
            enabled: true,
            ..Policy::default()
        },
        Instant::now(),
    );
    assert_eq!(disabled.press(Instant::now()), PressResult::Refused);
    assert_eq!(
        disabled.backend.captures, 0,
        "consent is checked before UIA"
    );
    assert_eq!(
        disabled.snapshot().refusal,
        Some(Refusal::Voice(koru_dictation::ErrorCode::ConsentRequired))
    );
    assert!(disabled.target.is_none());
}

#[test]
fn hold_release_and_timeout_finish_while_explicit_cancel_discards() {
    for outcome in [HoldOutcome::Released, HoldOutcome::TimedOut] {
        let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
        let now = Instant::now();
        assert_eq!(
            coordinator.press(now),
            PressResult::Started(koru_dictation::Mode::PushToTalk)
        );
        coordinator.hold_finished(outcome, now);
        assert_eq!(coordinator.snapshot().phase, Phase::Finalising);
        complete(&mut coordinator, now, "kept");
        assert_eq!(coordinator.backend.inserted, ["kept"]);
    }

    let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
    let now = Instant::now();
    coordinator.press(now);
    coordinator.cancel(koru_dictation::Interruption::Suspended, now);
    assert_eq!(coordinator.snapshot().outcome, Some(Outcome::Cancelled));
    assert!(coordinator.target.is_none());
    assert!(coordinator.backend.inserted.is_empty());
}

#[test]
fn toggle_ignores_release_and_the_second_press_finishes_without_recapture() {
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
    let now = Instant::now();
    assert_eq!(
        coordinator.press(now),
        PressResult::Started(koru_dictation::Mode::Toggle)
    );
    coordinator.release(now);
    assert_eq!(coordinator.snapshot().phase, Phase::Listening);
    assert_eq!(coordinator.press(now), PressResult::Finished);
    assert_eq!(coordinator.snapshot().phase, Phase::Finalising);
    assert_eq!(coordinator.backend.captures, 1);
}

#[test]
fn hud_finish_finalises_the_live_flight_in_every_mode() {
    for mode in [
        koru_dictation::Mode::PushToTalk,
        koru_dictation::Mode::Toggle,
    ] {
        let (mut coordinator, _) = coordinator(mode);
        let now = Instant::now();
        assert!(matches!(coordinator.press(now), PressResult::Started(_)));
        coordinator.finish(now);
        assert_eq!(coordinator.snapshot().phase, Phase::Finalising);
        assert_eq!(coordinator.backend.captures, 1);
        complete(&mut coordinator, now, "kept");
        assert_eq!(coordinator.backend.inserted, ["kept"]);
    }
}

#[test]
fn repeat_press_is_inert_and_one_target_belongs_to_one_session() {
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
    let now = Instant::now();
    coordinator.press(now);
    assert_eq!(coordinator.press(now), PressResult::Ignored);
    assert_eq!(coordinator.backend.captures, 1);
    coordinator.release(now);
    complete(&mut coordinator, now, "once");
    assert_eq!(coordinator.backend.inserted, ["once"]);
}

#[test]
fn target_loss_worker_death_and_model_refusal_drop_the_target() {
    let (mut target_loss, _) = coordinator(koru_dictation::Mode::PushToTalk);
    target_loss.backend.states = [TargetState::Editable, TargetState::Changed].into();
    let now = Instant::now();
    target_loss.press(now);
    target_loss.release(now);
    complete(&mut target_loss, now, "never");
    assert!(target_loss.backend.inserted.is_empty());
    assert_eq!(
        target_loss.snapshot().refusal,
        Some(Refusal::Target(TargetRefusal::TargetChanged))
    );

    for finalising in [false, true] {
        for (code, message) in [
            (koru_dictation::ErrorCode::WorkerUnavailable, "closed"),
            (koru_dictation::ErrorCode::ModelMissing, "missing"),
        ] {
            let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
            coordinator.press(now);
            if finalising {
                coordinator.release(now);
            }
            let ended = ended_event(
                &coordinator,
                koru_dictation::EndReason::Failed {
                    code,
                    message: message.into(),
                },
            );
            coordinator.observe(ended);
            coordinator.service(now);
            assert!(coordinator.target.is_none());
            assert!(coordinator.backend.inserted.is_empty());
            assert_eq!(coordinator.snapshot().outcome, Some(Outcome::Cancelled));
            assert_eq!(coordinator.snapshot().refusal, Some(Refusal::Voice(code)));
        }
    }
}

#[test]
fn duplicate_final_and_end_can_insert_at_most_once() {
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
    let now = Instant::now();
    coordinator.press(now);
    coordinator.release(now);
    let final_event = final_event(&coordinator, "one insertion");
    let ended = ended_event(&coordinator, koru_dictation::EndReason::Completed);
    coordinator.observe(final_event.clone());
    coordinator.observe(ended.clone());
    coordinator.service(now);
    coordinator.observe(final_event);
    coordinator.observe(ended);
    coordinator.service(now);
    assert_eq!(coordinator.backend.inserted, ["one insertion"]);
    assert!(coordinator.target.is_none());
}

#[test]
fn partials_never_publish_content_and_levels_coalesce_to_one_wake() {
    let (mut coordinator, published) = coordinator(koru_dictation::Mode::PushToTalk);
    let now = Instant::now();
    coordinator.press(now);
    let after_start = published.lock().unwrap().len();
    let (session, generation) = coordinator.live();
    coordinator.observe(koru_dictation::Event::Partial {
        session,
        generation,
        text: koru_dictation::Transcript::new("secret partial"),
    });
    coordinator.service(now);
    assert_eq!(published.lock().unwrap().len(), after_start);

    for step in [2, 5, 8] {
        coordinator.observe(koru_dictation::Event::Level {
            session,
            generation,
            bucket: koru_dictation::LevelBucket::new(step),
        });
    }
    coordinator.service(now);
    let published = published.lock().unwrap();
    assert_eq!(published.len(), after_start + 1);
    assert_eq!(
        published.last().unwrap().level,
        koru_dictation::LevelBucket::new(8)
    );
}

#[test]
fn a_restarted_coordinator_is_idle_disabled_and_holds_no_target() {
    let restarted = Coordinator::new(
        koru_dictation::Dictation::new(),
        FakeInsert::default(),
        |_| {},
    );
    assert_eq!(restarted.snapshot(), Snapshot::default());
    assert!(restarted.target.is_none());
}

#[cfg(windows)]
#[test]
fn handle_shutdown_joins_and_a_restarted_actor_has_no_stale_state() {
    use std::time::Duration;

    fn wait_for_disabled(handle: &Handle) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while handle.snapshot().refusal != Some(Refusal::Disabled) {
            assert!(
                Instant::now() < deadline,
                "coordinator did not answer the press"
            );
            std::thread::yield_now();
        }
    }

    fn shutdown_and_join(mut handle: Handle) {
        let _ = handle.sender.send(Command::Shutdown);
        handle
            .thread
            .take()
            .expect("coordinator thread")
            .join()
            .expect("coordinator shutdown");
    }

    let (first_waker, first_events) = crate::event::Waker::collecting();
    let first = Handle::spawn(
        crate::Paths::for_root("C:/unused-global-dictation-test".into()),
        first_waker,
    );
    first.press(None, global_input::VirtualKey::new(b'D' as u16));
    wait_for_disabled(&first);
    shutdown_and_join(first);
    let first_count = first_events.lock().unwrap().len();

    let (second_waker, second_events) = crate::event::Waker::collecting();
    let second = Handle::spawn(
        crate::Paths::for_root("C:/unused-global-dictation-test".into()),
        second_waker,
    );
    assert_eq!(second.snapshot(), Snapshot::default());
    second.press(None, global_input::VirtualKey::new(b'D' as u16));
    wait_for_disabled(&second);
    shutdown_and_join(second);

    assert_eq!(first_events.lock().unwrap().len(), first_count);
    let second_events = second_events.lock().unwrap();
    assert_eq!(second_events.len(), 1);
    assert!(matches!(
        second_events[0],
        crate::event::AgentEvent::GlobalDictationChanged(_)
    ));
}

#[test]
fn disabling_or_revoking_consent_cancels_live_and_finalising_sessions() {
    for finalising in [false, true] {
        for revoke_consent in [false, true] {
            let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
            let now = Instant::now();
            coordinator.press(now);
            if finalising {
                coordinator.release(now);
            }
            coordinator.configure(
                Policy {
                    enabled: revoke_consent,
                    consent: (!revoke_consent).then(grant),
                    ..Policy::default()
                },
                now,
            );
            assert_eq!(coordinator.snapshot().phase, Phase::Idle);
            assert_eq!(coordinator.snapshot().outcome, Some(Outcome::Cancelled));
            assert!(coordinator.target.is_none());
            assert!(coordinator.backend.inserted.is_empty());
        }
    }
}

#[test]
fn unavailable_release_observation_refuses_before_capture_or_session() {
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
    assert_eq!(
        coordinator.press_with_release_ready(Instant::now(), false),
        PressResult::Refused
    );
    assert_eq!(coordinator.backend.captures, 0);
    assert!(coordinator.target.is_none());
    assert_eq!(
        coordinator.snapshot().refusal,
        Some(Refusal::Target(TargetRefusal::TargetUnavailable))
    );
}

#[test]
fn every_target_refusal_is_typed_before_voice_begins() {
    for refusal in [TargetRefusal::NoTarget, TargetRefusal::TargetUnavailable] {
        let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
        coordinator.backend.capture_refusal = Some(refusal);
        assert_eq!(coordinator.press(Instant::now()), PressResult::Refused);
        assert_eq!(
            coordinator.snapshot().refusal,
            Some(Refusal::Target(refusal))
        );
        assert!(coordinator.target.is_none());
    }
    for (state, refusal) in [
        (TargetState::Protected, TargetRefusal::ProtectedTarget),
        (TargetState::Changed, TargetRefusal::TargetChanged),
        (TargetState::Destroyed, TargetRefusal::TargetDestroyed),
        (TargetState::Unavailable, TargetRefusal::TargetUnavailable),
    ] {
        let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
        coordinator.backend.states.push_back(state);
        assert_eq!(coordinator.press(Instant::now()), PressResult::Refused);
        assert_eq!(
            coordinator.snapshot().refusal,
            Some(Refusal::Target(refusal))
        );
        assert!(coordinator.target.is_none());
    }
}

#[test]
fn every_cancel_reason_discards_and_stale_hold_finish_cannot_end_a_new_flight() {
    for reason in [
        global_input::CancelReason::Explicit,
        global_input::CancelReason::SecureDesktop,
        global_input::CancelReason::Resume,
        global_input::CancelReason::Shutdown,
    ] {
        let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
        let now = Instant::now();
        coordinator.press(now);
        coordinator.hold_finished(HoldOutcome::Cancelled(reason), now);
        assert_eq!(coordinator.snapshot().phase, Phase::Idle);
        assert!(coordinator.target.is_none());
    }

    let old = global_input::HoldFlight(1);
    let new = global_input::HoldFlight(2);
    let mut active = Some(old);
    assert_eq!(
        active.take(),
        Some(old),
        "explicit cancel clears the old flight"
    );
    active = Some(new);
    assert!(!take_matching_flight(&mut active, old));
    assert_eq!(active, Some(new));
    assert!(take_matching_flight(&mut active, new));
}

#[test]
fn voice_wakes_are_coalesced_until_the_worker_drains_them() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let armed = AtomicBool::new(false);
    let enqueued = AtomicUsize::new(0);
    for _ in 0..8 {
        arm_pump_once(&armed, || {
            enqueued.fetch_add(1, Ordering::Relaxed);
        });
    }
    assert_eq!(enqueued.load(Ordering::Relaxed), 1);
    armed.store(false, Ordering::Release);
    arm_pump_once(&armed, || {
        enqueued.fetch_add(1, Ordering::Relaxed);
    });
    assert_eq!(enqueued.load(Ordering::Relaxed), 2);
}

#[test]
fn direct_partial_and_refused_insertion_each_attempt_once_and_never_retry() {
    for (direct, expected) in [
        (DirectInsertion::Partial, Refusal::DirectPartial),
        (
            DirectInsertion::Rejected,
            Refusal::Target(TargetRefusal::ClipboardCompletionUnprovable),
        ),
    ] {
        let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
        coordinator.backend.direct = direct;
        let now = Instant::now();
        coordinator.press(now);
        coordinator.press(now);
        complete(&mut coordinator, now, "not retried");
        assert_eq!(coordinator.backend.insert_calls, 1);
        assert!(coordinator.backend.inserted.is_empty());
        assert_eq!(coordinator.snapshot().refusal, Some(expected));
    }
}

#[test]
fn old_generation_events_are_stale_and_worker_failure_can_reconnect() {
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::Toggle);
    let now = Instant::now();
    coordinator.press(now);
    let (old_session, old_generation) = coordinator.live();
    let failed = ended_event(
        &coordinator,
        koru_dictation::EndReason::Failed {
            code: koru_dictation::ErrorCode::WorkerUnavailable,
            message: "closed".into(),
        },
    );
    coordinator.observe(failed);
    coordinator.service(now);
    assert!(coordinator.target.is_none());

    assert_eq!(
        coordinator.press(now),
        PressResult::Started(koru_dictation::Mode::Toggle)
    );
    coordinator.observe(koru_dictation::Event::Final {
        session: old_session,
        generation: old_generation,
        text: koru_dictation::Transcript::new("stale"),
    });
    coordinator.observe(koru_dictation::Event::Ended {
        session: old_session,
        generation: old_generation,
        reason: koru_dictation::EndReason::Completed,
    });
    coordinator.service(now);
    assert!(coordinator.backend.inserted.is_empty());
    coordinator.press(now);
    complete(&mut coordinator, now, "fresh");
    assert_eq!(coordinator.backend.inserted, ["fresh"]);
}

#[test]
fn voice_failure_retires_flight_one_before_immediate_flight_two() {
    use std::sync::mpsc;
    use std::time::Duration;

    struct Keys(VecDeque<global_input::KeyState>);
    impl global_input::KeyStateSource for Keys {
        fn state(&mut self, _key: global_input::VirtualKey) -> global_input::KeyState {
            self.0.pop_front().unwrap_or(global_input::KeyState::Down)
        }
    }

    let config = global_input::ObserverConfig {
        poll_interval: Duration::from_millis(2),
        max_hold: Duration::from_secs(1),
        resume_gap: Duration::from_secs(1),
    };
    let (send, receive) = mpsc::channel();
    let old_send = send.clone();
    let old = global_input::HoldFlight(1);
    let new = global_input::HoldFlight(2);
    let key = global_input::VirtualKey::new(b'D' as u16);
    let old_observer = global_input::ReleaseObserver::spawn_with_config(
        Keys(VecDeque::new()),
        config,
        move |notice| {
            let _ = old_send.send(notice);
        },
    )
    .unwrap();
    let (mut coordinator, _) = coordinator(koru_dictation::Mode::PushToTalk);
    let now = Instant::now();
    coordinator.press_with_release_ready(now, true);
    let mut active = Some(old);
    old_observer.press(key, old);
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(1)).unwrap(),
        global_input::HoldNotice::Pressed { flight: old }
    );

    let failed = ended_event(
        &coordinator,
        koru_dictation::EndReason::Failed {
            code: koru_dictation::ErrorCode::WorkerUnavailable,
            message: "closed".into(),
        },
    );
    coordinator.observe(failed);
    coordinator.service(now);
    assert_eq!(active.take(), Some(old));
    old_observer.shutdown();

    let new_observer = global_input::ReleaseObserver::spawn_with_config(
        Keys([global_input::KeyState::Up].into()),
        config,
        move |notice| {
            let _ = send.send(notice);
        },
    )
    .unwrap();
    assert_eq!(
        coordinator.press_with_release_ready(now, true),
        PressResult::Started(koru_dictation::Mode::PushToTalk)
    );
    active = Some(new);
    new_observer.press(key, new);

    let mut new_finished = false;
    for _ in 0..3 {
        match receive.recv_timeout(Duration::from_secs(1)).unwrap() {
            global_input::HoldNotice::Pressed { .. } => {}
            global_input::HoldNotice::Finished { flight, outcome } => {
                if take_matching_flight(&mut active, flight) {
                    assert_eq!(flight, new);
                    assert_eq!(outcome, HoldOutcome::Released);
                    coordinator.hold_finished(outcome, now);
                    new_finished = true;
                    break;
                }
            }
        }
    }
    assert!(new_finished);
    complete(&mut coordinator, now, "flight two");
    assert_eq!(coordinator.backend.inserted, ["flight two"]);
    new_observer.shutdown();
}
