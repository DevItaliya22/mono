// SPDX-License-Identifier: MPL-2.0

//! **Stable codes, and untranslated developer text beside them.**
//!
//! `{ code, message }`, which is the convention
//! [PRINCIPLES](../../../guide/contributing/PRINCIPLES.md) already sets for every API in this
//! product: `code` is what a caller branches on and what a log line carries,
//! `message` is developer text nobody may show a person. A surface that has to
//! say something to a person looks the *code* up in `koru-i18n`; there is
//! deliberately no translated string anywhere in this crate, because a session
//! also runs inside a headless bench and inside a phone's audio thread.
//!
//! ## Why the code is a closed enum and not a `String`
//!
//! `koru_agent_wire` carries `code: String`, for a good reason there: an agent
//! and a shell can be different builds, so a code from a newer peer has to
//! survive being read by an older one. That is not this crate's situation. The
//! worker, the client and the window are three of the six binaries in one zip
//! ([ADR-112](../../../docs/decisions/ADR-101-125.md)); they are installed
//! together, updated together and never mixed. The only boundary where two
//! versions can meet is the handshake, and [`crate::wire`] refuses there rather
//! than guessing.
//!
//! So the enum is closed, which buys three things a string does not: an
//! exhaustive `match` in the client, a test that every code is unique, and the
//! impossibility of an adapter inventing `"MODEL_LOAD_FAILED"` because it did
//! not know `model_load_failed` existed. An engine adapter with something more
//! specific to say puts it in `message`.

use serde::{Deserialize, Serialize};

/// Every way voice can refuse or fail, as one closed list.
///
/// Lowercase ASCII with underscores, and that is not a style note: these
/// strings are matched by a client, printed in a log, and — one day — used as a
/// `koru-i18n` key. Mixed case or a space in one is a rename waiting to happen.
/// `every_code_is_lowercase_ascii_and_unique` in `error/tests.rs` is what keeps
/// it true.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// A dictation is already running on this installation. **Per
    /// installation, not per machine** — stable, beta, nightly and a rooted
    /// test build each derive their own endpoint and therefore their own
    /// worker, and a lock spanning them would let a throwaway dev build block a
    /// real session. Two installations capturing at once produce an OS-level
    /// device conflict, reported as [`ErrorCode::CaptureFailed`].
    Busy,
    /// The generation on this message is not ahead of the last one this session
    /// ran, so it belongs to a session that has already ended. The reply for a
    /// *request*; the same situation arriving as an engine event or a host
    /// reply is dropped silently and counted, because nobody asked.
    StaleGeneration,
    /// There is no live session for this input to be about.
    NoSession,
    /// There is a live session and it is not in a state that accepts this — a
    /// second `finish`, audio that arrived while the model was still loading.
    WrongState,
    /// The named model is not installed on this machine. Distinct from
    /// [`ErrorCode::ModelLoadFailed`] because only one of them is fixed by
    /// downloading something.
    ModelMissing,
    /// The bytes are here and the runtime would not load them — a corrupt
    /// archive, a model built for another engine, not enough memory.
    ModelLoadFailed,
    /// The microphone would not open: no device, or a device another process
    /// holds exclusively.
    ///
    /// **Not a refused permission** — that is
    /// [`ErrorCode::PermissionDenied`], and the split is the whole point of
    /// having both. One of them is fixed in the operating system's privacy
    /// settings and the other is not, so a surface that could only say
    /// *the microphone would not open* would send somebody to look at their
    /// hardware for a switch that is in Settings.
    CaptureFailed,
    /// The operating system refused this application the microphone.
    ///
    /// A per-app privacy setting on Windows, a TCC denial on macOS. It is a
    /// state rather than a failure: nothing is broken and nothing will change
    /// until somebody allows it, which is why the sentence a surface shows
    /// names where the switch is.
    PermissionDenied,
    /// Nobody has agreed to voice input on this machine, so nothing may be
    /// downloaded and no microphone may be opened.
    ///
    /// **Not an error somebody has to act on in a toast** — it is the state
    /// the consent step exists to change, and a host that meets it opens that
    /// step rather than reporting it. It is a code so that a path which
    /// *cannot* open the step (a worker, a command line) still has a true
    /// thing to say. See `koru-voice-consent`.
    ConsentRequired,
    /// The engine refused the request or failed part-way through it. The
    /// runtime's own words go in `message`.
    EngineFailed,
    /// Finalisation failed, so there is no final transcript to commit. The
    /// composition is discarded rather than committed from the last partial.
    FinishFailed,
    /// This model cannot transcribe the language the request named, or the
    /// request asked for `Auto` and this engine cannot detect one.
    LanguageUnsupported,
    /// The two ends of a connection do not speak the same
    /// [`crate::wire::PROTOCOL_VERSION`]. Both numbers travel in the greeting,
    /// because "incompatible" alone sends somebody to read two changelogs to
    /// find out which end is behind.
    ProtocolMismatch,
    /// The voice worker is not running and could not be started. The client's
    /// code, not the worker's — a worker that answers never sends it.
    WorkerUnavailable,
}

impl ErrorCode {
    /// Every code, so a test can walk them.
    pub const ALL: [ErrorCode; 14] = [
        ErrorCode::Busy,
        ErrorCode::StaleGeneration,
        ErrorCode::NoSession,
        ErrorCode::WrongState,
        ErrorCode::ModelMissing,
        ErrorCode::ModelLoadFailed,
        ErrorCode::CaptureFailed,
        ErrorCode::PermissionDenied,
        ErrorCode::ConsentRequired,
        ErrorCode::EngineFailed,
        ErrorCode::FinishFailed,
        ErrorCode::LanguageUnsupported,
        ErrorCode::ProtocolMismatch,
        ErrorCode::WorkerUnavailable,
    ];

    /// The stable token. The same string serde writes, pinned by a test —
    /// otherwise a `rename_all` change would move the wire spelling and leave
    /// every log line and every future translation key behind.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Busy => "busy",
            ErrorCode::StaleGeneration => "stale_generation",
            ErrorCode::NoSession => "no_session",
            ErrorCode::WrongState => "wrong_state",
            ErrorCode::ModelMissing => "model_missing",
            ErrorCode::ModelLoadFailed => "model_load_failed",
            ErrorCode::CaptureFailed => "capture_failed",
            ErrorCode::PermissionDenied => "permission_denied",
            ErrorCode::ConsentRequired => "consent_required",
            ErrorCode::EngineFailed => "engine_failed",
            ErrorCode::FinishFailed => "finish_failed",
            ErrorCode::LanguageUnsupported => "language_unsupported",
            ErrorCode::ProtocolMismatch => "protocol_mismatch",
            ErrorCode::WorkerUnavailable => "worker_unavailable",
        }
    }

    /// The code a token names, for a client reading one back out of a log or a
    /// stored diagnostic. `None` rather than a catch-all: a code nobody
    /// declared is not a code, and swallowing it into an `Other` arm is how a
    /// typo survives a review.
    pub fn parse(token: &str) -> Option<ErrorCode> {
        ErrorCode::ALL.into_iter().find(|c| c.as_str() == token)
    }

    /// **The `koru-i18n` row a surface shows a person for this code.**
    ///
    /// The one exception to this module's "no translated string here" rule, and
    /// it is a *key* rather than a string: the mapping is the thing two
    /// surfaces have to agree about, not the wording. Two of them already need
    /// it — the document's refusal toast and the notification area's voice
    /// submenu — and neither can see the other's crate, so a copy in each is a
    /// pair that drifts silently. Which sentence a code deserves is a fact
    /// about the code.
    ///
    /// **`PermissionDenied` is two rows because the switch is in two places**
    /// (Q-V75). A single sentence would be wrong on one platform, which is
    /// worse than a vague one: somebody sent to the wrong Settings page
    /// concludes the feature is broken.
    ///
    /// Everything a person can do nothing about collapses into one sentence —
    /// a stale generation, a wrong state, an engine that failed part-way and a
    /// finalisation that did not finish are all *the dictation stopped* from
    /// where somebody is sitting, and the code is in the log.
    pub fn sentence_key(self) -> koru_i18n::Key {
        use koru_i18n::Key;
        match self {
            ErrorCode::CaptureFailed => Key::VoiceErrorMicrophone,
            ErrorCode::PermissionDenied => {
                if cfg!(target_os = "macos") {
                    Key::VoiceErrorPermissionMac
                } else {
                    Key::VoiceErrorPermissionWindows
                }
            }
            ErrorCode::ConsentRequired => Key::VoiceErrorConsent,
            ErrorCode::ModelMissing | ErrorCode::ModelLoadFailed => Key::VoiceErrorModelMissing,
            ErrorCode::Busy => Key::VoiceErrorBusy,
            ErrorCode::WorkerUnavailable => Key::VoiceErrorUnavailable,
            ErrorCode::ProtocolMismatch => Key::VoiceErrorOutdated,
            ErrorCode::StaleGeneration
            | ErrorCode::NoSession
            | ErrorCode::WrongState
            | ErrorCode::EngineFailed
            | ErrorCode::FinishFailed
            | ErrorCode::LanguageUnsupported => Key::VoiceErrorGeneric,
        }
    }

    /// Build the error with developer text attached.
    pub fn with(self, message: impl Into<String>) -> VoiceError {
        VoiceError {
            code: self,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refusal or a failure: a code to branch on, and developer text that is
/// **never shown to a person**.
///
/// One struct rather than one per layer. A session refusing `finish` after a
/// cancel and an engine failing to load a model are the same *shape*, and
/// giving each its own struct is how a product ends up with two error
/// vocabularies that mean the same things under different names.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct VoiceError {
    pub code: ErrorCode,
    /// Untranslated, English, for a developer and a log. It may name an id, a
    /// state or a count; it may never carry a transcript, a device's name or
    /// any audio — see `docs/contributing/operations/LOGGING.md`.
    pub message: String,
}

/// What [`crate::SpeechEngine`]'s methods answer with.
///
/// A type alias rather than a second struct, so the trait reads the way the
/// roadmap writes it while there is still only one error shape to learn.
pub type EngineError = VoiceError;

impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for VoiceError {}

#[cfg(test)]
mod tests;
