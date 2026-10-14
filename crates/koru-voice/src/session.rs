// SPDX-License-Identifier: MPL-2.0

//! **One dictation, as a state machine that performs nothing.**
//!
//! [`Session`] steps `Idle → Loading → Listening → Finalising → Idle`. It holds
//! no engine, opens no device, spawns no thread and touches no disk. Every
//! input returns a `Vec<Effect>` — instructions for whoever *does* own those
//! things — and the host feeds back what happened as a [`HostReply`].
//!
//! ## Why the state machine is not in the worker binary
//!
//! Because a phone has no worker binary. M4 hosts this same machine inside the
//! app process, where `Effect::StartCapture` is an `AudioRecord` and there is no
//! socket at all; the desktop hosts it in `apps/koru-voice`, where it is `cpal`
//! and there is. Keeping the decisions as *values* is what lets one machine
//! serve both — and it is also what makes the machine testable against neither,
//! which is the whole of `session/tests.rs`.
//!
//! ## The two enums are the complete boundary
//!
//! [`Effect`] out, [`HostReply`] back, and nothing else crosses. That is worth
//! stating because the tempting fifth effect is always "…and send this text
//! somewhere". Delivery is deliberately not an effect: on the desktop it is a
//! socket write and on a phone it is a JNI call, and a session that named
//! either would have acquired a platform.
//!
//! Instead the session **queues** what it decided, as [`Event`] values, and the
//! host drains them with [`Session::take_events`]. The queue earns its place by
//! making one invariant structural rather than a habit: **every event that
//! leaves is stamped with the session id and generation here, in one place.**
//! A host that built its own events would stamp them at a dozen call sites, and
//! the one that forgot would be the one that committed a cancelled sentence
//! into the next dictation.
//!
//! ## Generations, and what "dropped" means
//!
//! Every request, event and reply carries a generation. Three different things
//! happen to a stale one, and the difference is deliberate:
//!
//! - a **request** (`begin`, `finish`, `cancel`) is *refused*, with
//!   [`ErrorCode::StaleGeneration`], because somebody asked and is owed an
//!   answer;
//! - a **host reply** or an **engine event** is *dropped silently and counted*,
//!   because nobody asked — a `Final` from a session that was cancelled two
//!   seconds ago is not an error, it is the ordinary consequence of a decoder
//!   still running when the person let go of the key. It must produce no
//!   `Event`, and `a_final_from_the_previous_generation_is_dropped` is what says
//!   so;
//! - a **`Final` volunteered while still listening** is dropped and counted
//!   too, but for a different reason, and the counter is separate so the two
//!   never explain each other away. See [`EngineEvent::Final`].

use crate::engine::{EngineEvent, ModelId, SessionId, SessionRequest};
use crate::error::{ErrorCode, VoiceError};
use crate::language::VoiceLanguage;
use crate::wire::{Event, LevelBucket};
use serde::{Deserialize, Serialize};

/// Where a session is.
///
/// The four the roadmap names, and only those four. Getting the model loaded
/// and getting the device open are two waits inside `Loading`, not two states:
/// a person watching an indicator is told *preparing* either way, and the
/// difference between them is a diagnostic rather than a state. It is available
/// as [`Session::awaiting`] for exactly that purpose.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Nothing is running. The only state in which [`Session::begin`] is
    /// accepted.
    #[default]
    Idle,
    /// Getting ready: loading the model, then opening the device.
    Loading,
    /// Audio is being accepted and partials are coming back.
    Listening,
    /// No more audio; waiting for the final result. The composition stays
    /// provisional through this state — a key going up is not a reason to
    /// commit the last partial.
    Finalising,
}

/// Which reply the session is waiting for. A diagnostic, not a state.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Awaiting {
    ModelLoad,
    CaptureStart,
    CaptureStop,
    FinalResult,
}

/// **What [`Session`] asks its host to do.**
///
/// Loading a model, opening a device and stopping capture are all things a
/// *host* performs — a worker with `cpal` and a socket, or a phone with
/// `AudioRecord` and a JNI shim. Keeping them as values is what lets one state
/// machine serve both and be tested against neither.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Get this model into memory, then answer [`HostReply::ModelLoaded`] or
    /// [`HostReply::ModelLoadFailed`]. It may already be loaded, in which case
    /// the reply is immediate and the session never notices the difference.
    LoadModel(ModelId),
    /// Open the microphone and start filling the ring, then answer
    /// [`HostReply::CaptureStarted`] or [`HostReply::CaptureFailed`].
    StartCapture,
    /// Stop capturing and answer [`HostReply::CaptureStopped`]. Emitted before
    /// [`Effect::Finish`] rather than beside it, so the engine is asked to
    /// finalise only once no more audio can arrive.
    StopCapture,
    /// **Call [`SpeechEngine::finish`] and report back.**
    ///
    /// The engine method is the mechanism and the effect is the instruction.
    /// The host owns the call because the host owns the engine, and `Session`
    /// never holds one. Failure comes back as [`HostReply::FinishFailed`];
    /// success arrives as [`EngineEvent::Final`] through the ordinary poll.
    ///
    /// [`SpeechEngine::finish`]: crate::SpeechEngine::finish
    Finish,
    /// The session is over. Let go of whatever was held for it: the engine's
    /// per-session state, the ring, a model lease's session hold.
    ///
    /// **Not "unload the model".** A person who dictates twice in ten seconds
    /// must not pay a several-hundred-megabyte load twice; unloading is a
    /// separate idle timer in the worker (`KORU_VOICE_IDLE_UNLOAD`, Plan 1.3).
    Release,
}

/// **What the host tells the session happened.**
///
/// The other half, without which `Loading` cannot advance and an asynchronous
/// failure has no way back to `Idle`. Each is correlated to the generation of
/// the session that asked, so a reply to a session that has already ended is
/// dropped rather than applied to its successor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostReply {
    ModelLoaded { generation: u64 },
    ModelLoadFailed { generation: u64, code: ErrorCode },
    CaptureStarted { generation: u64 },
    CaptureFailed { generation: u64, code: ErrorCode },
    CaptureStopped { generation: u64 },
    FinishFailed { generation: u64, code: ErrorCode },
}

impl HostReply {
    /// The generation this reply is about.
    pub fn generation(&self) -> u64 {
        match self {
            HostReply::ModelLoaded { generation }
            | HostReply::ModelLoadFailed { generation, .. }
            | HostReply::CaptureStarted { generation }
            | HostReply::CaptureFailed { generation, .. }
            | HostReply::CaptureStopped { generation }
            | HostReply::FinishFailed { generation, .. } => *generation,
        }
    }
}

/// Why a session stopped.
///
/// Three outcomes and no fourth. A session that ends any other way has a bug,
/// and the exhaustiveness is what makes that a compile error in every consumer.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndReason {
    /// A final transcript was produced, and the [`Event::Final`] immediately
    /// before this one carries it. The **only** reason a host may commit.
    Completed,
    /// Thrown away deliberately: the key was released with a discard, focus was
    /// lost, the window closed, the machine slept. Nothing was written and
    /// nothing is owed. Half a sentence landing in a document you were not
    /// looking at is worse than losing it.
    Cancelled,
    /// Something failed. The composition is discarded rather than committed
    /// from the last partial, because a partial is text the engine had not
    /// finished revising.
    Failed { code: ErrorCode, message: String },
}

/// What this session machine has done since the process started.
///
/// Counters, not content. **Ids, counts, stages and milliseconds** is the whole
/// vocabulary a voice diagnostic is allowed, and every field here is a count.
/// Free — a handful of integers — which is what makes it cheap enough to leave
/// on, and a diagnostic somebody has to enable and rebuild for is one that is
/// never there during the incident.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SessionStats {
    /// Sessions begun this run.
    pub sessions: u64,
    /// Audio samples the session authorised. Divide by [`SAMPLE_RATE`] for
    /// seconds heard.
    ///
    /// [`SAMPLE_RATE`]: crate::SAMPLE_RATE
    pub samples: u64,
    /// Partials forwarded.
    pub partials: u64,
    /// Finals forwarded — at most one per completed session.
    pub finals: u64,
    /// Host replies dropped on their generation. A steady stream of these means
    /// a host is answering for sessions that ended, which is worth knowing and
    /// is not itself a fault.
    pub dropped_replies: u64,
    /// Engine events dropped on their generation. The ordinary reason is a
    /// decoder still running after a cancel.
    pub dropped_engine_events: u64,
    /// Finals an engine volunteered while still listening. **Separate from the
    /// count above on purpose**: this one means an adapter is breaking the
    /// contract in [`EngineEvent::Final`], and folding it into a generic
    /// "dropped" would let a real defect hide inside an ordinary race.
    pub unexpected_finals: u64,
    /// Requests refused — busy, stale, or in the wrong state.
    pub refusals: u64,
    /// The last code this machine produced, for `koru-voice-worker status`.
    pub last_error: Option<ErrorCode>,
}

/// The session that is currently running.
#[derive(Clone, Debug)]
struct Current {
    request: SessionRequest,
    awaiting: Awaiting,
    /// The answer to [`crate::Requested::Auto`], once the engine has one.
    detected: Option<VoiceLanguage>,
}

/// **One dictation at a time, as a value.**
///
/// Construct one per host — a worker has exactly one, because there is one
/// microphone — and drive it with the inputs below. It is `Send`, holds no
/// borrow and allocates only the outbound queue.
#[derive(Debug, Default)]
pub struct Session {
    state: State,
    current: Option<Current>,
    /// The highest generation this machine has ever run. Monotonic across
    /// sessions, which is what makes a replayed request refusable.
    high_water: Option<u64>,
    out: Vec<Event>,
    stats: SessionStats,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    /// Which of the four states this is in.
    pub fn state(&self) -> State {
        self.state
    }

    /// Which reply is outstanding, when one is. The sub-step inside `Loading`
    /// and `Finalising` that deliberately is not a state of its own.
    pub fn awaiting(&self) -> Option<Awaiting> {
        self.current.as_ref().map(|current| current.awaiting)
    }

    /// The live session's id and generation, when there is one.
    pub fn current(&self) -> Option<(SessionId, u64)> {
        self.current
            .as_ref()
            .map(|current| (current.request.id, current.request.generation))
    }

    /// What the live session asked for.
    pub fn request(&self) -> Option<&SessionRequest> {
        self.current.as_ref().map(|current| &current.request)
    }

    /// The language the engine reported, when it has reported one.
    pub fn detected_language(&self) -> Option<VoiceLanguage> {
        self.current.as_ref().and_then(|current| current.detected)
    }

    /// Counters. See [`SessionStats`].
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// Take everything the session has decided to say. The host sends these,
    /// or hands them to a shim; the session keeps no copy.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.out)
    }

    // ------------------------------------------------------------- the inputs

    /// Start one dictation.
    ///
    /// Refused with [`ErrorCode::Busy`] when one is already running — **per
    /// installation, not per machine**; see that code's note. Refused with
    /// [`ErrorCode::StaleGeneration`] when the generation is not strictly
    /// greater than every generation this machine has run, which is what makes
    /// a reconnecting client's replayed request harmless.
    pub fn begin(&mut self, request: SessionRequest) -> Result<Vec<Effect>, VoiceError> {
        if self.state != State::Idle {
            return Err(self.refuse(
                ErrorCode::Busy,
                format!("a session is already running in state {:?}", self.state),
            ));
        }
        if let Some(high_water) = self.high_water {
            if request.generation <= high_water {
                return Err(self.refuse(
                    ErrorCode::StaleGeneration,
                    format!(
                        "generation {} is not ahead of {high_water}",
                        request.generation
                    ),
                ));
            }
        }
        let model = request.model.clone();
        self.high_water = Some(request.generation);
        self.stats.sessions += 1;
        self.current = Some(Current {
            request,
            awaiting: Awaiting::ModelLoad,
            detected: None,
        });
        self.enter(State::Loading);
        Ok(vec![Effect::LoadModel(model)])
    }

    /// **Ask whether this audio is wanted**, before feeding it to the engine.
    ///
    /// The session does not consume audio — it holds no engine and no buffer —
    /// so this authorises rather than accepts, and the host's inference thread
    /// reads:
    ///
    /// ```text
    /// session.audio(&frame)?;
    /// engine.accept_audio(&frame)?;
    /// ```
    ///
    /// It is deliberately *not* called `accept_audio`: two methods on two types
    /// with one name and two meanings is the hazard `Leaf`'s `Editing`/`View`
    /// pair already demonstrates, where choosing wrong is silent.
    ///
    /// The slice is passed rather than a length because that is the call shape
    /// that makes the refusal useful — a host cannot ask about one buffer and
    /// feed another without noticing.
    pub fn audio(&mut self, samples: &[f32]) -> Result<(), VoiceError> {
        match self.state {
            State::Listening => {
                self.stats.samples += samples.len() as u64;
                Ok(())
            }
            State::Idle => Err(self.refuse(
                ErrorCode::NoSession,
                "audio arrived with no session running",
            )),
            state => Err(self.refuse(
                ErrorCode::WrongState,
                format!("audio arrived in state {state:?}"),
            )),
        }
    }

    /// A coarse input level, for a meter.
    ///
    /// Queued and stamped like everything else, and **dropped when no session
    /// is listening** — a level with nothing to be about is noise on the wire.
    /// It produces no effect: a meter never changes what a session does.
    pub fn level(&mut self, bucket: LevelBucket) {
        if self.state != State::Listening {
            return;
        }
        let Some((session, generation)) = self.current() else {
            return;
        };
        self.out.push(Event::Level {
            session,
            generation,
            bucket,
        });
    }

    /// Hand over something the engine decided.
    ///
    /// `generation` is the generation the **engine run** was begun with — the
    /// inference thread captured it when it called
    /// [`SpeechEngine::begin`](crate::SpeechEngine::begin) — not the session's
    /// current one. That is the whole mechanism: a poll that drains a decoder
    /// belonging to a cancelled session passes the old number and the event is
    /// dropped here rather than committed there.
    pub fn engine_event(&mut self, generation: u64, event: EngineEvent) -> Vec<Effect> {
        let Some((session, current_generation)) = self.current() else {
            self.stats.dropped_engine_events += 1;
            return Vec::new();
        };
        if generation != current_generation {
            self.stats.dropped_engine_events += 1;
            return Vec::new();
        }
        match event {
            EngineEvent::Partial(text) => {
                if !matches!(self.state, State::Listening | State::Finalising) {
                    self.stats.dropped_engine_events += 1;
                    return Vec::new();
                }
                self.stats.partials += 1;
                self.out.push(Event::Partial {
                    session,
                    generation,
                    text,
                });
                Vec::new()
            }
            EngineEvent::Final(text) => {
                if self.state != State::Finalising {
                    // Finalisation is requested, never volunteered. Counted
                    // separately so a misbehaving adapter is visible instead of
                    // hiding inside the ordinary post-cancel race.
                    self.stats.unexpected_finals += 1;
                    return Vec::new();
                }
                self.stats.finals += 1;
                self.out.push(Event::Final {
                    session,
                    generation,
                    text,
                });
                self.end(EndReason::Completed);
                vec![Effect::Release]
            }
            EngineEvent::LanguageDetected(language) => {
                if let Some(current) = self.current.as_mut() {
                    current.detected = Some(language);
                }
                self.out.push(Event::Language {
                    session,
                    generation,
                    language,
                });
                Vec::new()
            }
        }
    }

    /// Hand over what the host did about an effect.
    ///
    /// A reply for a generation that is not current, or one that arrives in a
    /// state that was not waiting for it, is dropped and counted. It returns
    /// `Vec<Effect>` and never an error for the same reason: nobody asked, so
    /// there is nobody to tell.
    pub fn host_reply(&mut self, reply: HostReply) -> Vec<Effect> {
        let Some((_, generation)) = self.current() else {
            self.stats.dropped_replies += 1;
            return Vec::new();
        };
        if reply.generation() != generation {
            self.stats.dropped_replies += 1;
            return Vec::new();
        }
        let awaiting = self.awaiting();
        match (reply, awaiting) {
            (HostReply::ModelLoaded { .. }, Some(Awaiting::ModelLoad)) => {
                self.set_awaiting(Awaiting::CaptureStart);
                vec![Effect::StartCapture]
            }
            (HostReply::CaptureStarted { .. }, Some(Awaiting::CaptureStart)) => {
                self.set_awaiting(Awaiting::FinalResult);
                self.enter(State::Listening);
                Vec::new()
            }
            (HostReply::CaptureStopped { .. }, Some(Awaiting::CaptureStop)) => {
                self.set_awaiting(Awaiting::FinalResult);
                vec![Effect::Finish]
            }
            (HostReply::ModelLoadFailed { code, .. }, _)
            | (HostReply::CaptureFailed { code, .. }, _)
            | (HostReply::FinishFailed { code, .. }, _) => {
                let message = format!("the host reported {code}");
                self.stats.last_error = Some(code);
                self.end(EndReason::Failed { code, message });
                vec![Effect::Release]
            }
            _ => {
                self.stats.dropped_replies += 1;
                Vec::new()
            }
        }
    }

    /// Stop capturing and produce the final result.
    ///
    /// From `Listening` this is the ordinary path: capture stops, then the
    /// engine is asked to finalise, and the session stays `Finalising` until
    /// the matching [`EngineEvent::Final`] arrives. A key going up is not a
    /// reason to commit the last partial.
    ///
    /// From `Loading` there is nothing to finalise — somebody released the key
    /// before the model was ready — and **the session is discarded**, which is
    /// the answer every other interruption in this product already gets.
    pub fn finish(
        &mut self,
        session: SessionId,
        generation: u64,
    ) -> Result<Vec<Effect>, VoiceError> {
        self.check_addressed(session, generation)?;
        match self.state {
            State::Listening => {
                self.set_awaiting(Awaiting::CaptureStop);
                self.enter(State::Finalising);
                Ok(vec![Effect::StopCapture])
            }
            State::Loading => {
                let mut effects = self.stop_capture_if_live();
                self.end(EndReason::Cancelled);
                effects.push(Effect::Release);
                Ok(effects)
            }
            state => Err(self.refuse(
                ErrorCode::WrongState,
                format!("finish arrived in state {state:?}"),
            )),
        }
    }

    /// Throw the session away. No final, no text, no trace.
    pub fn cancel(
        &mut self,
        session: SessionId,
        generation: u64,
    ) -> Result<Vec<Effect>, VoiceError> {
        self.check_addressed(session, generation)?;
        let mut effects = self.stop_capture_if_live();
        self.end(EndReason::Cancelled);
        effects.push(Effect::Release);
        Ok(effects)
    }

    // ------------------------------------------------------------- internals

    /// Whether a request naming this session and generation may act.
    ///
    /// `NoSession` when nothing is running; `StaleGeneration` when something
    /// else is — which is the case that matters, because a late `cancel` for a
    /// finished dictation must not cancel the one that followed it.
    fn check_addressed(&mut self, session: SessionId, generation: u64) -> Result<(), VoiceError> {
        match self.current() {
            None => Err(self.refuse(ErrorCode::NoSession, "no session is running")),
            Some((id, current)) if id == session && current == generation => Ok(()),
            Some((_, current)) => Err(self.refuse(
                ErrorCode::StaleGeneration,
                format!("generation {generation} is not the running {current}"),
            )),
        }
    }

    /// `StopCapture`, but only when capture could actually be running. Asking a
    /// host to stop a device it never opened is harmless and is also a line in
    /// its log every time somebody releases a key early.
    fn stop_capture_if_live(&self) -> Vec<Effect> {
        let live = match (self.state, self.awaiting()) {
            (State::Loading, Some(Awaiting::CaptureStart)) => true,
            (State::Listening, _) => true,
            (State::Finalising, Some(Awaiting::CaptureStop)) => true,
            _ => false,
        };
        if live {
            vec![Effect::StopCapture]
        } else {
            Vec::new()
        }
    }

    fn set_awaiting(&mut self, awaiting: Awaiting) {
        if let Some(current) = self.current.as_mut() {
            current.awaiting = awaiting;
        }
    }

    /// Move to a state that is not `Idle`, and say so.
    ///
    /// `Idle` is never announced through [`Event::State`]; the return to it is
    /// [`Event::Ended`], which carries the reason a bare state cannot.
    /// `a_state_event_never_reports_idle` is what holds that.
    fn enter(&mut self, state: State) {
        debug_assert_ne!(state, State::Idle, "Idle is announced as Ended");
        self.state = state;
        if let Some((session, generation)) = self.current() {
            self.out.push(Event::State {
                session,
                generation,
                state,
            });
        }
    }

    fn end(&mut self, reason: EndReason) {
        if let Some((session, generation)) = self.current() {
            self.out.push(Event::Ended {
                session,
                generation,
                reason,
            });
        }
        self.state = State::Idle;
        self.current = None;
    }

    fn refuse(&mut self, code: ErrorCode, message: impl Into<String>) -> VoiceError {
        self.stats.refusals += 1;
        self.stats.last_error = Some(code);
        code.with(message)
    }
}

#[cfg(test)]
mod tests;
