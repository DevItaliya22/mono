// SPDX-License-Identifier: MPL-2.0

//! The state machine, proved with no thread, no device and no model.
//!
//! Two pieces of scaffolding do all the work. [`FakeEngine`] is a
//! [`SpeechEngine`] with a script: a partial per audio frame, a final on
//! `finish`, and counters saying what it was asked. [`Host`] is the other half
//! of the boundary — it performs every [`Effect`] the session emits and feeds
//! the [`HostReply`] straight back, recording the effect sequence as it goes.
//!
//! Together they are the argument that `Session` is worth having: the whole
//! lifecycle, including the failure paths and the late-message races, runs in
//! microseconds against neither a microphone nor an inference runtime.

use super::*;
use crate::engine::{EngineId, EngineInfo, SpeechEngine};
use crate::error::EngineError;
use crate::language::Requested;
use crate::transcript::Transcript;
use std::collections::VecDeque;

const SENTINEL: &str = "quenderoth-vasp-1471";

// ------------------------------------------------------------- the scaffolding

/// A recogniser that does exactly what its script says.
struct FakeEngine {
    partials: VecDeque<&'static str>,
    final_text: &'static str,
    queued: VecDeque<EngineEvent>,
    begun: u32,
    finished: u32,
    cancelled: u32,
    /// A language to announce once, if the script has one.
    detects: Option<VoiceLanguage>,
}

impl FakeEngine {
    fn new(partials: &[&'static str], final_text: &'static str) -> FakeEngine {
        FakeEngine {
            partials: partials.iter().copied().collect(),
            final_text,
            queued: VecDeque::new(),
            begun: 0,
            finished: 0,
            cancelled: 0,
            detects: None,
        }
    }
}

impl SpeechEngine for FakeEngine {
    fn describe(&self) -> EngineInfo {
        EngineInfo {
            engine: EngineId::parse("fake").unwrap(),
            version: "0".into(),
            model: None,
            languages: VoiceLanguage::ALL.to_vec(),
            streaming: true,
            detects_language: true,
        }
    }

    fn begin(&mut self, _request: &SessionRequest) -> Result<(), EngineError> {
        self.begun += 1;
        if let Some(language) = self.detects {
            self.queued
                .push_back(EngineEvent::LanguageDetected(language));
        }
        Ok(())
    }

    fn accept_audio(&mut self, _samples: &[f32]) -> Result<(), EngineError> {
        if let Some(partial) = self.partials.pop_front() {
            self.queued
                .push_back(EngineEvent::Partial(Transcript::new(partial)));
        }
        Ok(())
    }

    fn poll(&mut self) -> Option<EngineEvent> {
        self.queued.pop_front()
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        self.finished += 1;
        self.queued
            .push_back(EngineEvent::Final(Transcript::new(self.final_text)));
        Ok(())
    }

    fn cancel(&mut self) {
        self.cancelled += 1;
        self.queued.clear();
    }
}

/// The other half of the boundary: it performs effects and answers replies.
struct Host {
    session: Session,
    engine: FakeEngine,
    /// The generation the *engine run* was begun with, which is what a poll is
    /// correlated against — not the session's current one.
    engine_generation: Option<u64>,
    performed: Vec<Effect>,
    capturing: bool,
    model_load_fails: Option<ErrorCode>,
    capture_fails: Option<ErrorCode>,
    finish_fails: Option<ErrorCode>,
}

impl Host {
    fn new(partials: &[&'static str], final_text: &'static str) -> Host {
        Host {
            session: Session::new(),
            engine: FakeEngine::new(partials, final_text),
            engine_generation: None,
            performed: Vec::new(),
            capturing: false,
            model_load_fails: None,
            capture_fails: None,
            finish_fails: None,
        }
    }

    fn request(&self, generation: u64) -> SessionRequest {
        SessionRequest {
            id: SessionId::new(),
            generation,
            model: ModelId::parse("fake-model").unwrap(),
            language: Requested::Fixed(VoiceLanguage::En),
        }
    }

    fn begin(&mut self, generation: u64) -> (SessionId, u64) {
        let request = self.request(generation);
        let id = request.id;
        let effects = self.session.begin(request).expect("begin");
        self.perform(effects);
        (id, generation)
    }

    /// One frame of audio, and whatever the engine made of it.
    fn speak(&mut self) {
        let frame = [0.0f32; 160];
        self.session.audio(&frame).expect("audio");
        self.engine.accept_audio(&frame).expect("accept_audio");
        let effects = self.drain_engine();
        self.perform(effects);
    }

    fn finish(&mut self, session: SessionId, generation: u64) {
        let effects = self.session.finish(session, generation).expect("finish");
        self.perform(effects);
    }

    fn perform(&mut self, effects: Vec<Effect>) {
        let mut queue: VecDeque<Effect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            self.performed.push(effect.clone());
            let generation = self.session.current().map(|(_, g)| g).unwrap_or_default();
            let more = match effect {
                Effect::LoadModel(_) => match self.model_load_fails {
                    Some(code) => self
                        .session
                        .host_reply(HostReply::ModelLoadFailed { generation, code }),
                    None => self
                        .session
                        .host_reply(HostReply::ModelLoaded { generation }),
                },
                Effect::StartCapture => match self.capture_fails {
                    Some(code) => self
                        .session
                        .host_reply(HostReply::CaptureFailed { generation, code }),
                    None => {
                        self.capturing = true;
                        self.engine_generation = Some(generation);
                        let request = self.session.request().cloned().expect("a live request");
                        self.engine.begin(&request).expect("engine begin");
                        let mut out = self.drain_engine();
                        out.extend(
                            self.session
                                .host_reply(HostReply::CaptureStarted { generation }),
                        );
                        out
                    }
                },
                Effect::StopCapture => {
                    self.capturing = false;
                    self.session
                        .host_reply(HostReply::CaptureStopped { generation })
                }
                Effect::Finish => match self.finish_fails {
                    Some(code) => self
                        .session
                        .host_reply(HostReply::FinishFailed { generation, code }),
                    None => {
                        self.engine.finish().expect("engine finish");
                        self.drain_engine()
                    }
                },
                Effect::Release => {
                    self.engine.cancel();
                    self.engine_generation = None;
                    Vec::new()
                }
            };
            queue.extend(more);
        }
    }

    fn drain_engine(&mut self) -> Vec<Effect> {
        let generation = self.engine_generation.unwrap_or_default();
        let mut out = Vec::new();
        while let Some(event) = self.engine.poll() {
            out.extend(self.session.engine_event(generation, event));
        }
        out
    }

    fn events(&mut self) -> Vec<Event> {
        self.session.take_events()
    }
}

/// A shorthand so an assertion reads as a sequence of names rather than a wall
/// of struct literals.
fn shapes(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            Event::State {
                state: State::Loading,
                ..
            } => "state:loading",
            Event::State {
                state: State::Listening,
                ..
            } => "state:listening",
            Event::State {
                state: State::Finalising,
                ..
            } => "state:finalising",
            Event::State {
                state: State::Idle, ..
            } => "state:idle",
            Event::Partial { .. } => "partial",
            Event::Final { .. } => "final",
            Event::Language { .. } => "language",
            Event::Level { .. } => "level",
            Event::Ended {
                reason: EndReason::Completed,
                ..
            } => "ended:completed",
            Event::Ended {
                reason: EndReason::Cancelled,
                ..
            } => "ended:cancelled",
            Event::Ended {
                reason: EndReason::Failed { .. },
                ..
            } => "ended:failed",
            Event::Refused { .. } => "refused",
            // The session machine never produces one: a device list is not a
            // dictation, and this arm exists so that stops being an
            // assumption.
            Event::Devices { .. } => unreachable!("a session never answers a device query"),
        })
        .collect()
}

// ------------------------------------------------------------------- the tests

/// **The plan's central test.** A scripted engine drives the session end to
/// end, and the emitted effect sequence is asserted — no thread, no device, no
/// model.
#[test]
fn a_scripted_engine_drives_the_session_end_to_end() {
    let mut host = Host::new(
        &["Create a new", "Create a new project"],
        "Create a new project plan",
    );
    let (id, generation) = host.begin(1);
    host.speak();
    host.speak();
    host.finish(id, generation);

    assert_eq!(
        host.performed,
        vec![
            Effect::LoadModel(ModelId::parse("fake-model").unwrap()),
            Effect::StartCapture,
            Effect::StopCapture,
            Effect::Finish,
            Effect::Release,
        ]
    );
    let events = host.events();
    assert_eq!(
        shapes(&events),
        vec![
            "state:loading",
            "state:listening",
            "partial",
            "partial",
            "state:finalising",
            "final",
            "ended:completed",
        ]
    );
    assert_eq!(host.session.state(), State::Idle);
    assert_eq!(host.session.current(), None);
    assert!(!host.capturing);
    assert_eq!(host.engine.begun, 1);
    assert_eq!(host.engine.finished, 1);

    let stats = host.session.stats();
    assert_eq!(stats.sessions, 1);
    assert_eq!(stats.partials, 2);
    assert_eq!(stats.finals, 1);
    assert_eq!(stats.samples, 320);
    assert_eq!(stats.refusals, 0);
    assert_eq!(stats.unexpected_finals, 0);

    // The final is the text `finish` produced, not the last partial.
    let Some(Event::Final { text, .. }) = events.iter().find(|e| matches!(e, Event::Final { .. }))
    else {
        panic!("no final");
    };
    assert_eq!(text.as_str(), "Create a new project plan");
}

/// Two sessions in a row through one machine. The second is a whole fresh
/// lifecycle, and its generation is ahead of the first's.
#[test]
fn one_machine_runs_one_session_after_another() {
    let mut host = Host::new(&["one", "two"], "one");
    let (id, generation) = host.begin(1);
    host.speak();
    host.finish(id, generation);
    host.events();

    let (id, generation) = host.begin(2);
    host.speak();
    host.finish(id, generation);
    assert_eq!(
        shapes(&host.events()),
        vec![
            "state:loading",
            "state:listening",
            "partial",
            "state:finalising",
            "final",
            "ended:completed",
        ]
    );
    assert_eq!(host.session.stats().sessions, 2);
}

/// The plan's refusal test, first half. There is nothing running, so there is
/// nothing for audio to be about.
#[test]
fn audio_before_begin_is_refused_with_a_stable_code() {
    let mut session = Session::new();
    let refused = session.audio(&[0.0; 16]).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NoSession);
    assert_eq!(session.stats().samples, 0);
    assert_eq!(session.stats().refusals, 1);
    assert_eq!(session.stats().last_error, Some(ErrorCode::NoSession));
}

/// Audio that arrives while the model is still loading is a different refusal
/// from audio with no session at all, and a caller that branches on the code
/// can tell them apart.
#[test]
fn audio_before_listening_is_refused_as_the_wrong_state() {
    let mut session = Session::new();
    let request = SessionRequest {
        id: SessionId::new(),
        generation: 1,
        model: ModelId::parse("fake-model").unwrap(),
        language: Requested::Auto,
    };
    session.begin(request).unwrap();
    assert_eq!(session.state(), State::Loading);
    assert_eq!(
        session.audio(&[0.0; 16]).unwrap_err().code,
        ErrorCode::WrongState
    );
}

/// The plan's refusal test, second half.
#[test]
fn finish_after_cancel_is_refused_with_a_stable_code() {
    let mut host = Host::new(&["one"], "one");
    let (id, generation) = host.begin(1);
    host.speak();
    let effects = host.session.cancel(id, generation).unwrap();
    host.perform(effects);

    let refused = host.session.finish(id, generation).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NoSession);
    assert_eq!(
        shapes(&host.events()),
        vec![
            "state:loading",
            "state:listening",
            "partial",
            "ended:cancelled"
        ]
    );
    // Nothing was finalised, so no final exists to be committed.
    assert_eq!(host.engine.finished, 0);
    assert_eq!(host.engine.cancelled, 1);
}

/// One dictation at a time. The second asker is told so with a code rather than
/// silently taking the microphone.
#[test]
fn a_second_begin_while_one_is_live_is_refused_as_busy() {
    let mut host = Host::new(&[], "one");
    host.begin(1);
    let second = host.request(2);
    let refused = host.session.begin(second).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Busy);
    assert_eq!(host.session.state(), State::Listening);
}

/// A client that reconnects and replays its last request must not restart a
/// dictation the person has already finished.
#[test]
fn a_replayed_generation_is_refused_as_stale() {
    let mut host = Host::new(&["one"], "one");
    let (id, generation) = host.begin(7);
    host.finish(id, generation);
    assert_eq!(host.session.state(), State::Idle);

    for replay in [1, 7] {
        let request = host.request(replay);
        let refused = host.session.begin(request).unwrap_err();
        assert_eq!(refused.code, ErrorCode::StaleGeneration);
    }
    // And the next real one is accepted.
    let next = host.request(8);
    assert!(host.session.begin(next).is_ok());
}

/// **The race this whole mechanism exists for.** A decoder is still running
/// when the person cancels; its `Final` arrives while the *next* dictation is
/// open. It must produce no event at all.
#[test]
fn a_final_from_the_previous_generation_is_dropped() {
    let mut host = Host::new(&[], "the abandoned sentence");
    let (id, generation) = host.begin(1);
    let effects = host.session.cancel(id, generation).unwrap();
    host.perform(effects);

    // A second dictation is opened and is listening.
    let (_, next) = host.begin(2);
    assert_eq!(host.session.state(), State::Listening);
    host.events();

    let stale = host.session.engine_event(
        generation,
        EngineEvent::Final(Transcript::new("the abandoned sentence")),
    );
    assert!(stale.is_empty());
    assert!(host.events().is_empty(), "a stale final produced an event");
    assert_eq!(host.session.stats().dropped_engine_events, 1);
    assert_eq!(host.session.stats().finals, 0);
    assert_eq!(host.session.state(), State::Listening);
    assert_eq!(host.session.current().map(|(_, g)| g), Some(next));
}

/// The same rule for a partial: a revision from an ended session must not paint
/// itself into the one that followed.
#[test]
fn a_partial_from_the_previous_generation_is_dropped() {
    let mut host = Host::new(&[], "one");
    let (id, generation) = host.begin(1);
    let effects = host.session.cancel(id, generation).unwrap();
    host.perform(effects);
    host.begin(2);
    host.events();

    host.session
        .engine_event(generation, EngineEvent::Partial(Transcript::new(SENTINEL)));
    assert!(host.events().is_empty());
    assert_eq!(host.session.stats().dropped_engine_events, 1);
}

/// A `cancel` for a dictation that has already ended must not cancel the one in
/// progress. This is the request-shaped half of the same race, and it is
/// *refused* rather than dropped because somebody asked.
#[test]
fn a_late_cancel_does_not_cancel_the_next_session() {
    let mut host = Host::new(&[], "one");
    let (old_id, old_generation) = host.begin(1);
    let effects = host.session.cancel(old_id, old_generation).unwrap();
    host.perform(effects);
    host.begin(2);

    let refused = host.session.cancel(old_id, old_generation).unwrap_err();
    assert_eq!(refused.code, ErrorCode::StaleGeneration);
    assert_eq!(host.session.state(), State::Listening);
}

/// A host answering for a session that has ended is not a fault — it is what a
/// device callback in flight looks like — so it is counted and ignored rather
/// than surfaced.
///
/// **The first drop happens without anybody arranging it**, and it is worth
/// naming: `cancel` is terminal *before* the host performs the `StopCapture` it
/// returned, so the `CaptureStopped` that answers it always arrives at an idle
/// machine. That is the design — a cancelled session has stopped caring what
/// the device does next — and the counter moving on an ordinary cancel is not
/// evidence of a fault.
#[test]
fn a_host_reply_from_a_dead_session_is_dropped_and_counted() {
    let mut host = Host::new(&[], "one");
    let (id, generation) = host.begin(1);
    let effects = host.session.cancel(id, generation).unwrap();
    host.perform(effects);
    host.events();
    assert_eq!(host.session.stats().dropped_replies, 1);

    let effects = host
        .session
        .host_reply(HostReply::CaptureStopped { generation });
    assert!(effects.is_empty());
    assert!(host.events().is_empty());
    assert_eq!(host.session.stats().dropped_replies, 2);
}

/// Finalisation is requested, never volunteered. An adapter over an endpointing
/// recogniser reports segment results as partials; one that emits a `Final`
/// mid-utterance is breaking the contract, and the separate counter is what
/// makes that visible instead of letting it hide in the ordinary race.
#[test]
fn a_final_volunteered_while_listening_is_dropped_and_counted_apart() {
    let mut host = Host::new(&[], "one");
    host.begin(1);
    host.events();

    let effects = host
        .session
        .engine_event(1, EngineEvent::Final(Transcript::new("half a sentence")));
    assert!(effects.is_empty());
    assert!(host.events().is_empty());
    assert_eq!(host.session.stats().unexpected_finals, 1);
    assert_eq!(host.session.stats().dropped_engine_events, 0);
    assert_eq!(host.session.state(), State::Listening);
}

/// A model that will not load ends the session with the host's own code, and
/// the machine is back at `Idle` ready for the next attempt.
#[test]
fn a_model_that_will_not_load_ends_the_session_with_its_code() {
    let mut host = Host::new(&[], "one");
    host.model_load_fails = Some(ErrorCode::ModelMissing);
    let request = host.request(1);
    let effects = host.session.begin(request).unwrap();
    host.perform(effects);

    assert_eq!(
        host.performed,
        vec![
            Effect::LoadModel(ModelId::parse("fake-model").unwrap()),
            Effect::Release
        ]
    );
    let events = host.events();
    assert_eq!(shapes(&events), vec!["state:loading", "ended:failed"]);
    let Some(Event::Ended {
        reason: EndReason::Failed { code, .. },
        ..
    }) = events.last()
    else {
        panic!("no failure");
    };
    assert_eq!(*code, ErrorCode::ModelMissing);
    assert_eq!(host.session.state(), State::Idle);
    assert_eq!(
        host.session.stats().last_error,
        Some(ErrorCode::ModelMissing)
    );
}

/// Consent refused, or no device. The session ends and the worker keeps
/// answering.
#[test]
fn a_microphone_that_will_not_open_ends_the_session() {
    let mut host = Host::new(&[], "one");
    host.capture_fails = Some(ErrorCode::CaptureFailed);
    let request = host.request(1);
    let effects = host.session.begin(request).unwrap();
    host.perform(effects);

    assert_eq!(
        host.performed,
        vec![
            Effect::LoadModel(ModelId::parse("fake-model").unwrap()),
            Effect::StartCapture,
            Effect::Release,
        ]
    );
    assert_eq!(
        shapes(&host.events()),
        vec!["state:loading", "ended:failed"]
    );
    assert_eq!(host.session.state(), State::Idle);
}

/// Finalisation failing means there is no final transcript, so the composition
/// is discarded rather than committed from the last partial — text the engine
/// had not finished revising.
#[test]
fn a_finalisation_that_fails_ends_without_a_final() {
    let mut host = Host::new(&["half"], "whole");
    host.finish_fails = Some(ErrorCode::FinishFailed);
    let (id, generation) = host.begin(1);
    host.speak();
    host.finish(id, generation);

    let events = host.events();
    assert_eq!(
        shapes(&events),
        vec![
            "state:loading",
            "state:listening",
            "partial",
            "state:finalising",
            "ended:failed",
        ]
    );
    assert!(!events.iter().any(|e| matches!(e, Event::Final { .. })));
}

/// Releasing the key before the model is ready. There is nothing to finalise,
/// so it discards — the answer every other interruption in this product gets.
#[test]
fn a_finish_before_listening_discards_rather_than_erroring() {
    let mut session = Session::new();
    let id = SessionId::new();
    session
        .begin(SessionRequest {
            id,
            generation: 1,
            model: ModelId::parse("fake-model").unwrap(),
            language: Requested::Auto,
        })
        .unwrap();
    assert_eq!(session.awaiting(), Some(Awaiting::ModelLoad));

    // No capture was ever started, so none is stopped.
    let effects = session.finish(id, 1).unwrap();
    assert_eq!(effects, vec![Effect::Release]);
    assert_eq!(session.state(), State::Idle);
    assert_eq!(
        shapes(&session.take_events()),
        vec!["state:loading", "ended:cancelled"]
    );
}

/// A cancel once the device is opening does stop it — asking a host to stop a
/// device it never opened is a line in its log every time somebody is quick,
/// and asking it to leave one running would be a live microphone.
#[test]
fn a_cancel_stops_capture_only_when_capture_could_be_running() {
    let mut session = Session::new();
    let id = SessionId::new();
    let request = SessionRequest {
        id,
        generation: 1,
        model: ModelId::parse("fake-model").unwrap(),
        language: Requested::Auto,
    };
    session.begin(request).unwrap();
    // Awaiting the model: nothing to stop.
    assert_eq!(session.cancel(id, 1).unwrap(), vec![Effect::Release]);

    let mut session = Session::new();
    let request = SessionRequest {
        id,
        generation: 1,
        model: ModelId::parse("fake-model").unwrap(),
        language: Requested::Auto,
    };
    session.begin(request).unwrap();
    session.host_reply(HostReply::ModelLoaded { generation: 1 });
    // Awaiting the device: it may be open by now, so stop it.
    assert_eq!(
        session.cancel(id, 1).unwrap(),
        vec![Effect::StopCapture, Effect::Release]
    );
}

/// `Auto` is answered out of band, and the answer is both forwarded and
/// remembered.
#[test]
fn a_detected_language_is_forwarded_and_remembered() {
    let mut host = Host::new(&[], "one");
    host.engine.detects = Some(VoiceLanguage::Ja);
    let request = SessionRequest {
        id: SessionId::new(),
        generation: 1,
        model: ModelId::parse("fake-model").unwrap(),
        language: Requested::Auto,
    };
    let effects = host.session.begin(request).unwrap();
    host.perform(effects);

    assert_eq!(
        shapes(&host.events()),
        vec!["state:loading", "language", "state:listening"]
    );
    assert_eq!(host.session.detected_language(), Some(VoiceLanguage::Ja));
}

/// A meter is only about a live session, and it never changes what the session
/// does.
#[test]
fn a_level_is_stamped_while_listening_and_dropped_otherwise() {
    let mut host = Host::new(&[], "one");
    host.session.level(LevelBucket::new(4));
    assert!(host.events().is_empty(), "a level with no session was kept");

    let (id, generation) = host.begin(1);
    host.events();
    host.session.level(LevelBucket::new(200));
    let events = host.events();
    assert_eq!(shapes(&events), vec!["level"]);
    let Some(Event::Level { bucket, .. }) = events.first() else {
        panic!("no level");
    };
    assert_eq!(bucket.step(), LevelBucket::MAX);

    host.finish(id, generation);
    host.events();
    host.session.level(LevelBucket::new(1));
    assert!(host.events().is_empty());
}

/// **The invariant that keeps `Ended` meaningful.** `Idle` is never announced
/// as a state, because the return to idle carries a reason and a bare state
/// cannot. Held here rather than left as a rule in a doc comment.
#[test]
fn a_state_event_never_reports_idle_and_ended_is_always_last() {
    let mut host = Host::new(&["one", "two"], "three");
    let (id, generation) = host.begin(1);
    host.speak();
    host.speak();
    host.finish(id, generation);
    let events = host.events();

    assert!(
        !events.iter().any(|event| matches!(
            event,
            Event::State {
                state: State::Idle,
                ..
            }
        )),
        "a State event reported Idle"
    );
    assert!(matches!(events.last(), Some(Event::Ended { .. })));
    // And everything in the session carries the same id and generation.
    for event in &events {
        assert_eq!(event.session(), id);
        assert_eq!(event.generation(), generation);
    }
}

/// The queue is a drain: what has been taken is gone, so a host that sends its
/// events cannot send them twice.
#[test]
fn taking_events_empties_the_queue() {
    let mut host = Host::new(&[], "one");
    host.begin(1);
    assert!(!host.events().is_empty());
    assert!(host.events().is_empty());
}
