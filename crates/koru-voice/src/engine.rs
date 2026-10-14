// SPDX-License-Identifier: MPL-2.0

//! **What a speech engine is asked, and what it answers.**
//!
//! One trait, and the identities and descriptions around it. Nothing here
//! implements it: `koru-voice-sherpa` and `koru-voice-whisper` do, behind
//! features that are off by default so an ordinary `cargo test --workspace`
//! never compiles an inference runtime — nor lets one's build script reach the
//! network (Plan 0.3).
//!
//! ## The thread contract, which has exactly one normative statement
//!
//! **The real-time obligation belongs to the ring-buffer write, not to the
//! engine.** A capture callback converts into preallocated ring capacity,
//! advances counters and returns: it allocates nothing and blocks on nothing,
//! and that is where an allocation test is pointed.
//!
//! The **inference thread** owns the [`Session`](crate::Session) and the
//! [`SpeechEngine`] exclusively, and calls [`SpeechEngine::accept_audio`],
//! **which may decode and may allocate**. Rev 1 of the roadmap asserted the
//! real-time property in two places, which is how a contract ends up meaning
//! neither thing; this is the one that holds. A recogniser that decodes a
//! 160 ms window is doing hundreds of microseconds of arithmetic and there is
//! nothing to be gained by pretending it must not allocate — what matters is
//! that the *device callback* never waits for it.
//!
//! `Send` and no `Sync`: the engine moves onto the inference thread once and is
//! never shared. That is the bound the ownership rule above deserves, and it is
//! the reason an adapter over a `!Sync` native handle needs no lock.

use crate::error::EngineError;
use crate::language::{Requested, VoiceLanguage};
use crate::transcript::Transcript;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// **16 kHz, mono, `f32` in `-1.0..=1.0`.**
///
/// Declared once, here, because four components resample into it — the desktop
/// worker's `cpal` capture, the phone's `AudioRecord`/`AVAudioEngine` shim, the
/// benchmark's file reader and every engine adapter — and a second opinion
/// about the rate is a silent bug: an engine handed 48 kHz audio it believes is
/// 16 kHz transcribes fluent nonsense at a third of the speed rather than
/// failing.
///
/// Nothing outside this crate may declare it. That is an acceptance criterion
/// of Plan 0.1, not a preference.
pub const SAMPLE_RATE: u32 = 16_000;

/// One dictation, from the gesture that started it to the text that ended it.
///
/// A client-minted v4 UUID, never derived from a window, a device or a
/// profile — the identity rule the whole product follows. It is not enough on
/// its own to correlate a message: see [`SessionRequest::generation`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Mint one.
    ///
    /// Not `Default`, because a session id is an event rather than a value with
    /// a sensible blank — `SessionId::default()` appearing in a struct literal
    /// would silently invent a dictation.
    #[allow(clippy::new_without_default)]
    pub fn new() -> SessionId {
        SessionId(Uuid::new_v4())
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which model to run — `nemotron-3.5-int8`, `parakeet-tdt-v3`.
///
/// **A path component, so its charset is checked rather than assumed.** Plan
/// 1.2 stores installed bytes under `<id>/<version>/<digest>/`, and
/// [PRINCIPLES](../../../guide/contributing/PRINCIPLES.md) is explicit that arbitrary Unicode
/// is fine in a filename and never fine in a path component. Validating here —
/// in the only crate that both declares the type and is depended on by the
/// model store — is what stops a catalog entry named `../../etc` from ever
/// becoming a directory.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModelId(String);

/// Which runtime a model is for — `sherpa`, `whisper`.
///
/// Same validation as [`ModelId`], and for a weaker but real version of the
/// same reason: an engine id ends up in a directory name, a log line and a
/// `status` table, and one containing a newline is a log injection.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EngineId(String);

/// The charset both ids share: lowercase ASCII letters, digits, `-`, `.` and
/// `_`; non-empty; at most 64 characters; never beginning with a `.`.
///
/// Lowercase rather than case-insensitive because Windows and macOS have
/// case-insensitive filesystems and Linux does not: two ids differing only in
/// case would be one directory on two platforms and two on the third, which is
/// a corruption that only reproduces on somebody else's machine. The leading
/// `.` is refused because it makes `.` and `..` unreachable in one rule instead
/// of two.
fn validate_ident(what: &str, value: &str) -> Result<(), EngineError> {
    use crate::error::ErrorCode;
    let bad = |why: &str| Err(ErrorCode::EngineFailed.with(format!("{what} {why}")));
    if value.is_empty() {
        return bad("is empty");
    }
    if value.chars().count() > 64 {
        return bad("is longer than 64 characters");
    }
    if value.starts_with('.') {
        return bad("begins with a dot");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_'))
    {
        return bad("is not lowercase ASCII letters, digits, '-', '.' or '_'");
    }
    Ok(())
}

macro_rules! checked_ident {
    ($type:ty, $what:literal) => {
        impl $type {
            /// Check and wrap. The only constructor: there is deliberately no
            /// `From<String>`, because an unchecked one would be the shortest
            /// path and would therefore be the one used.
            pub fn parse(value: impl Into<String>) -> Result<$type, EngineError> {
                let value = value.into();
                validate_ident($what, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $type {
            type Error = EngineError;

            fn try_from(value: String) -> Result<$type, EngineError> {
                <$type>::parse(value)
            }
        }

        impl From<$type> for String {
            fn from(value: $type) -> String {
                value.0
            }
        }

        impl std::fmt::Display for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

checked_ident!(ModelId, "a model id");
checked_ident!(EngineId, "an engine id");

/// What an engine says about itself, for `koru-voice-worker status`, for the
/// benchmark's table, and for the one decision a caller makes from it.
///
/// The principle this serves is the standing one: **a component nobody can
/// inspect is a component nobody can trust**, and by the time somebody wants to
/// look it is too late to add the seam.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EngineInfo {
    pub engine: EngineId,
    /// The runtime's own version string. Printed and logged, **never parsed and
    /// never compared** — it is whatever the native library says about itself,
    /// and giving it a grammar would be inventing one.
    pub version: String,
    /// The model currently loaded, if any. `None` is the ordinary state of an
    /// idle worker: Plan 1.3 unloads after `KORU_VOICE_IDLE_UNLOAD`, because
    /// several hundred megabytes are only genuinely released by letting go.
    pub model: Option<ModelId>,
    /// What this engine, with this model, can transcribe.
    pub languages: Vec<VoiceLanguage>,
    /// Whether it revises its own output while audio is still arriving.
    ///
    /// An *online* recogniser answers `true`. An *offline* one answers `false`
    /// unless a declared chunk-and-redecode policy makes it true — which is a
    /// policy with its own latency and CPU cost, stated per candidate in
    /// `docs/features/voice/plans/ENGINE_BENCHMARK.md` rather than assumed.
    pub streaming: bool,
    /// Whether [`Requested::Auto`] is answerable. An engine that says `false`
    /// and is asked anyway refuses in [`SpeechEngine::begin`] with
    /// [`crate::ErrorCode::LanguageUnsupported`], rather than quietly
    /// transcribing as English.
    pub detects_language: bool,
}

/// What an engine is asked for.
///
/// The model is already resolved: choosing one from a language and a quality
/// tier is the model store's job (Plan 1.1), and a session that had to choose
/// would need a catalog, which would put a downloader under a state machine
/// that is meant to run on a phone's audio thread.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SessionRequest {
    pub id: SessionId,
    /// **Monotonic, and the reason correlation works at all.**
    ///
    /// A [`SessionId`] alone is not enough. The hard case is not two sessions
    /// with different ids — it is a `Final` that was in flight when its session
    /// was cancelled, arriving while the *next* one is open. Comparing ids
    /// would work only if every producer remembered to carry one; comparing a
    /// number that never goes backwards means a late message is dropped by
    /// arithmetic rather than by vigilance.
    ///
    /// Minted by whoever begins a session, and [`Session`](crate::Session)
    /// refuses one that is not strictly greater than the last it ran — so a
    /// client that reconnects and replays its last request cannot restart a
    /// dictation the person has already finished.
    pub generation: u64,
    pub model: ModelId,
    pub language: Requested,
}

/// What an engine has decided since it was last polled.
///
/// It carries no session id and no generation on purpose: an engine does not
/// know what a session is. The host hands each event to
/// [`Session::engine_event`](crate::Session::engine_event) together with the
/// generation the *engine run* was begun with, and the session stamps or drops
/// it. That is the whole of the correlation, in one place.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EngineEvent {
    /// The current best guess, which the engine will revise. Never enters Loro:
    /// a recogniser rewrites itself several times a second, and writing each
    /// revision would generate undo history, sync traffic and visible
    /// collaborative churn for text nobody has finished saying.
    Partial(Transcript),
    /// The result of [`SpeechEngine::finish`], and **only** of that.
    ///
    /// A `Final` volunteered mid-utterance is dropped and counted, because
    /// finalisation in this product is requested rather than offered: a person
    /// holding a key has not finished speaking merely because a recogniser
    /// found a pause. An adapter over an endpointing recogniser reports its
    /// segment results as [`EngineEvent::Partial`].
    Final(Transcript),
    /// The answer to [`Requested::Auto`], once the engine has one. It may
    /// arrive at any point in a session and may arrive more than once; the last
    /// one wins.
    LanguageDetected(VoiceLanguage),
}

/// **A speech recogniser, from the outside.**
///
/// Six methods and no lifecycle beyond them: the host constructs an
/// implementation with whatever a runtime needs, moves it to the inference
/// thread, and drops it when [`crate::Effect::Release`] says the session is
/// over.
///
/// Every method that can fail answers [`EngineError`], which is
/// `{ code, message }`: the code is branched on, the runtime's own words go in
/// the message, and nothing here is ever shown to a person.
pub trait SpeechEngine: Send {
    /// What this is, what it has loaded, and what it can do.
    fn describe(&self) -> EngineInfo;

    /// Prepare for one session. Called on the inference thread, after the
    /// model is loaded and before any audio.
    ///
    /// Refuses with [`crate::ErrorCode::LanguageUnsupported`] when the request
    /// names a language this model cannot transcribe, or asks for
    /// [`Requested::Auto`] and [`EngineInfo::detects_language`] is `false`.
    fn begin(&mut self, request: &SessionRequest) -> Result<(), EngineError>;

    /// Feed audio: mono `f32` in `-1.0..=1.0` at [`SAMPLE_RATE`].
    ///
    /// **Runs on the inference thread. May decode; may allocate.** See the
    /// module note — the real-time obligation is the ring-buffer write's, not
    /// this call's, and holding this method to a no-allocation rule would buy
    /// nothing and rule out most real runtimes.
    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), EngineError>;

    /// Drain whatever the engine has decided since the last call. **Never
    /// blocks** — a poll that waited would turn the inference thread into a
    /// thread that waits on the world, which is the one thing it may not be.
    /// `None` means nothing new, not "nothing more".
    fn poll(&mut self) -> Option<EngineEvent>;

    /// No more audio is coming; produce the final result.
    ///
    /// Returning `Ok` means finalisation has *started*. The result arrives
    /// through [`poll`](SpeechEngine::poll) as [`EngineEvent::Final`], possibly
    /// several polls later — a whole-utterance decoder does real work here, and
    /// a `finish` that blocked until it was done would stall the thread that is
    /// still meant to be answering.
    fn finish(&mut self) -> Result<(), EngineError>;

    /// Throw the session away. No final result, no error, nothing to report:
    /// cancellation is not a failure and the host has already stopped caring.
    /// It must be safe to call in any state, including one where `begin` was
    /// never reached.
    fn cancel(&mut self);
}

#[cfg(test)]
mod tests;
