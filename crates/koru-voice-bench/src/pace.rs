// SPDX-License-Identifier: MPL-2.0

//! **The two passes: one paced to a wall clock, one as fast as the engine takes
//! it.**
//!
//! Neither substitutes for the other, and running one and deriving the other's
//! number from it is the mistake this module exists to make impossible.
//!
//! **Paced.** One 20 ms frame at each monotonic deadline, exactly as a
//! microphone would. That is the only condition in which a latency has a meaning
//! a person would recognise: the clock they experience is wall-clock, and the
//! recogniser is being asked to keep up with a speaker rather than with a disk.
//! A paced pass **cannot** measure RTF, because pacing makes wall time equal
//! audio time by construction and every utterance would score 1.0.
//!
//! **Unpaced.** The whole utterance, pushed as fast as the engine accepts it.
//! That gives real-time factor and peak resident memory, and it contains no
//! clock that corresponds to anything a person waits for, so it cannot measure
//! latency.
//!
//! ## Deadlines are absolute, never cumulative sleeps
//!
//! `next += FRAME` from a fixed `t0`, and a sleep to that instant. Sleeping
//! `FRAME` between pushes instead would accumulate every wake-up's overshoot: on
//! Windows a 20 ms sleep is routinely 21–35 ms, so a five-second utterance would
//! be paced over seven and every latency in it would be measured against a clock
//! that had drifted a second and a half.
//!
//! **A deadline already in the past is not slept on and is counted.**
//! `overruns` is the number of frames the engine's own `accept_audio` pushed
//! past their deadline, and it is a fact about the candidate: an engine whose
//! synchronous decode costs more than 20 ms per frame *cannot be paced*, and its
//! latency figures are then measuring a queue rather than a recogniser. The
//! report prints it rather than silently producing a number.
//!
//! ## Events are drained after every push, and the instant is the poll's
//!
//! `SpeechEngine::poll` never blocks and answers one event at a time, so the
//! driver drains it after each push and stamps each event with the instant it
//! was *observed*. That is later than the instant it was produced, by however
//! long the rest of the push took — which is exactly right: a partial the host
//! cannot see yet has not appeared on anybody's screen.

use std::time::{Duration, Instant};

use koru_voice::{
    EngineError, EngineEvent, SessionRequest, SpeechEngine, Transcript, VoiceLanguage,
};

/// The frame the paced pass pushes, and the one a capture callback would
/// deliver.
pub const FRAME: Duration = Duration::from_millis(20);

/// One observed event.
#[derive(Clone, PartialEq, Debug)]
pub struct Observed {
    pub at: Instant,
    pub event: EngineEvent,
}

/// What one session produced.
#[derive(Clone, PartialEq, Debug)]
pub struct Session {
    /// `t0` — the instant the first frame was pushed after trimming.
    pub t0: Instant,
    /// The instant the last frame was pushed and `finish` was called.
    pub last_push: Instant,
    /// Every partial, in order, with the instant it was observed.
    pub partials: Vec<(Instant, Transcript)>,
    /// The final transcript and when it was observed. `None` when the engine
    /// never produced one — which is a failure of the run rather than a
    /// censored observation, and the caller must not treat the two alike.
    pub final_text: Option<(Instant, Transcript)>,
    /// The language the engine detected, if it said.
    pub detected: Option<VoiceLanguage>,
    /// Frames pushed after their deadline. See the module note.
    pub overruns: usize,
    /// The audio's trimmed duration, which is what RTF divides by.
    pub audio: Duration,
}

impl Session {
    /// Wall time from the first push to the `Final`.
    pub fn wall(&self) -> Option<Duration> {
        self.final_text.as_ref().map(|(at, _)| *at - self.t0)
    }

    /// `RTF = wall / trimmed audio duration`. Meaningful **only** from the
    /// unpaced pass; from a paced one it is 1.0 by construction.
    pub fn real_time_factor(&self) -> Option<f64> {
        let wall = self.wall()?;
        if self.audio.is_zero() {
            return None;
        }
        Some(wall.as_secs_f64() / self.audio.as_secs_f64())
    }

    /// `t_final − t_last_push` — the analogue of releasing the push-to-talk
    /// key, which is why no alignment is needed for it.
    pub fn final_latency(&self) -> Option<Duration> {
        self.final_text
            .as_ref()
            .map(|(at, _)| at.saturating_duration_since(self.last_push))
    }

    /// The final transcript's text, or the empty string.
    pub fn final_str(&self) -> &str {
        self.final_text
            .as_ref()
            .map(|(_, t)| t.as_str())
            .unwrap_or("")
    }
}

/// Drain everything the engine has decided, stamping each with now.
fn drain(engine: &mut dyn SpeechEngine, into: &mut Vec<Observed>) {
    while let Some(event) = engine.poll() {
        into.push(Observed {
            at: Instant::now(),
            event,
        });
    }
}

/// Run one utterance, paced or not.
///
/// One function with a flag rather than two, because the *only* difference is
/// whether the loop waits — and two functions would let the drain, the stamping
/// or the finish order drift apart between the passes, which would show up as
/// `drift` and be blamed on the engine.
pub fn run(
    engine: &mut dyn SpeechEngine,
    request: &SessionRequest,
    samples: &[f32],
    audio: Duration,
    paced: bool,
) -> Result<Session, EngineError> {
    engine.begin(request)?;

    let frame_samples = (FRAME.as_secs_f64() * f64::from(koru_voice::SAMPLE_RATE)) as usize;
    let mut observed: Vec<Observed> = Vec::new();
    let mut overruns = 0usize;

    let t0 = Instant::now();

    if paced {
        for (index, chunk) in samples.chunks(frame_samples).enumerate() {
            // Absolute, from t0. See the module note — a cumulative sleep would
            // accumulate every wake-up's overshoot.
            let deadline = t0 + FRAME * (index as u32);
            let now = Instant::now();
            if now < deadline {
                std::thread::sleep(deadline - now);
            } else if index > 0 {
                overruns += 1;
            }
            engine.accept_audio(chunk)?;
            drain(engine, &mut observed);
        }
    } else {
        // The whole utterance in one call. Not chunked: chunking unpaced would
        // charge the engine for per-call overhead the paced pass also pays, and
        // then the two passes' RTFs would differ for a reason that is ours.
        if !samples.is_empty() {
            engine.accept_audio(samples)?;
            drain(engine, &mut observed);
        }
    }

    let last_push = Instant::now();
    engine.finish()?;

    // Finalisation has *started*; the result arrives through `poll`, possibly
    // several polls later. A whole-utterance decoder does real work here.
    //
    // The bound is generous and is a bound rather than a wait: a `Final` that
    // has not arrived after this long is an engine that has stopped answering,
    // and hanging would turn one bad utterance into a run nobody finishes.
    let ceiling = last_push + FINALISE_TIMEOUT;
    let mut final_at: Option<(Instant, Transcript)> = None;
    while final_at.is_none() && Instant::now() < ceiling {
        drain(engine, &mut observed);
        for entry in &observed {
            if let EngineEvent::Final(text) = &entry.event {
                final_at = Some((entry.at, text.clone()));
            }
        }
        if final_at.is_none() {
            // A yield rather than a spin: `poll` is documented never to block,
            // so a tight loop here would burn a core against the very thread
            // that is trying to finish the decode.
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    let mut partials = Vec::new();
    let mut detected = None;
    for entry in &observed {
        match &entry.event {
            EngineEvent::Partial(text) => partials.push((entry.at, text.clone())),
            EngineEvent::LanguageDetected(language) => detected = Some(*language),
            EngineEvent::Final(_) => {}
        }
    }

    Ok(Session {
        t0,
        last_push,
        partials,
        final_text: final_at,
        detected,
        overruns,
        audio,
    })
}

/// How long a `Final` is waited for after `finish`.
///
/// Not a latency budget — the measured figure is whatever it turns out to be —
/// but a bound past which the engine is not answering. Two minutes is longer
/// than any plausible decode of a 20 s utterance at any RTF a candidate could
/// have and still be in this gate.
pub const FINALISE_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the finalisation loop sleeps between polls.
///
/// One millisecond: fine enough that it costs nothing against a `t_final` in the
/// hundreds of milliseconds, coarse enough not to compete with the decode.
pub const POLL_INTERVAL: Duration = Duration::from_millis(1);

#[cfg(test)]
mod tests;
