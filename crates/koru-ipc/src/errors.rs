// SPDX-License-Identifier: MPL-2.0

//! What can go wrong reaching a local endpoint, as values rather than strings.
//!
//! ## Why these are not a caller's own error type
//!
//! Because this crate is below every product in the tree and must stay there. A
//! store's vocabulary, an agent's vocabulary and an app's vocabulary are three
//! different things; borrowing any one of them here would point the dependency
//! the wrong way. So these are a small set of their own, and each caller
//! translates at its boundary with the context and remedy it alone knows.
//!
//! They are **stable ASCII tokens** for the same reason the store's are: a
//! person searching for one has been handed it by a program, and it must not be
//! a translated sentence.

use std::fmt;

/// Why an endpoint could not be named, claimed or reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcError {
    /// The segment is not one of `koru_channel::SEGMENTS`.
    ///
    /// Checked here, where it is cheap, rather than by the filesystem three
    /// frames down: a segment reaches a pipe name and a file name.
    UnknownSegment {
        segment: String,
        /// The channels that would have been accepted, comma-separated. Carried
        /// rather than recomputed so a caller writing its own remedy sentence
        /// does not have to reach for `koru-channel` to do it.
        expected: String,
    },
    /// Nothing is listening. Carries the endpoint's platform-native name,
    /// because a wrong endpoint is only diagnosable if you can see which one.
    NotRunning(String),
    /// Something already holds this name.
    ///
    /// **Not a failure to a client** — whoever wanted a service there has one.
    /// It is only an error to the process that was trying to *become* it, and
    /// even then the right answer is usually to stand down quietly.
    ///
    /// Told apart from a permissions problem by *connecting*, never by a lock
    /// or a pid file, which is the rule the whole crate follows.
    AlreadyBound(String),
    /// The endpoint exists and this process may not open it — on Windows,
    /// another user's pipe; on Unix, a socket in a directory whose mode
    /// excludes us. Also the failure to read this process's own SID, without
    /// which there is no way to name a per-user endpoint at all.
    PermissionDenied(String),
    /// The name could not be claimed, for a reason that is not "taken".
    BindFailed(String),
    /// The transport itself failed: a name the platform will not accept, a
    /// read or write that did not complete, a frame that is not ours.
    TransportFailed(String),
}

impl IpcError {
    /// A stable ASCII token. Safe to log, safe to match on, never translated.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownSegment { .. } => "ipc_unknown_segment",
            Self::NotRunning(_) => "ipc_not_running",
            Self::AlreadyBound(_) => "ipc_already_bound",
            Self::PermissionDenied(_) => "ipc_permission_denied",
            Self::BindFailed(_) => "ipc_bind_failed",
            Self::TransportFailed(_) => "ipc_transport_failed",
        }
    }

    /// Whether retrying could plausibly succeed without anything changing.
    ///
    /// Only "nothing is listening" — everything else needs somebody to do
    /// something first, and a client that retries those spins forever.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::NotRunning(_))
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSegment { segment, expected } => write!(
                f,
                "{segment:?} is not a release channel; expected one of {expected}"
            ),
            Self::NotRunning(name) => write!(f, "nothing is listening on {name}"),
            Self::AlreadyBound(name) => write!(f, "{name} is already held"),
            Self::PermissionDenied(why) => write!(f, "the endpoint refused this process: {why}"),
            Self::BindFailed(why) => write!(f, "could not claim the endpoint: {why}"),
            Self::TransportFailed(why) => write!(f, "the endpoint did not answer: {why}"),
        }
    }
}

impl std::error::Error for IpcError {}

#[cfg(test)]
mod tests;
