// SPDX-License-Identifier: MPL-2.0

//! **Whether this installation offers dictation at all** — one predicate, so
//! four surfaces cannot answer it four ways.
//!
//! `KORU_VOICE_DISABLED` is a kill switch rather than a diagnostic: with it set
//! no worker starts, no dictation chord is bound, the tray row says
//! *Unavailable* and the Settings section says why. Four separate places have
//! to agree about that, in four different crates, and each of them reaching for
//! `koru_env::switch` on its own is how three of them end up right and the
//! fourth ends up binding a chord that opens a feature nothing will serve.
//!
//! ## Why it lives here
//!
//! Because everything that has to ask already links this crate. The worker
//! (`koru-voice-worker`) does, the client does, the shell does through the
//! client, and Milestone 4's phone does directly. Putting it in the worker
//! would make the shell ask a binary it does not link; putting it in the client
//! would put it out of the worker's reach. This crate is the one place all of
//! them already meet, which is the same argument that put the wire messages and
//! the error codes here.
//!
//! It does not breach the charter. The exclusions are an inference runtime, a
//! microphone, a socket and file I/O; `koru-env` is a dependency-free leaf whose
//! whole content is a table of names and one truthiness rule, and reading a
//! process's own environment is neither of the two things that would make this
//! crate unusable on a phone or inside a test that must never open a device.
//!
//! ## Ask once
//!
//! [`disabled`] reads the environment on every call, deliberately — a cached
//! answer would need a lock to be testable and the value cannot change under a
//! running process anyway. So **a caller latches it**: at startup, when the key
//! map is built, when a surface is constructed. A per-frame caller is the bug
//! this note exists to name.

/// The variable's name, so a caller that wants to *say* it in a message names
/// the same string the registry declares.
pub const DISABLE_VAR: &str = koru_env::name::KORU_VOICE_DISABLED;

/// Whether dictation is switched off for this installation.
///
/// The one truthiness rule is `koru_env::switch`'s: on for anything except
/// unset, `""`, `0`, `false`, `no`, `off`. So `KORU_VOICE_DISABLED=0` means
/// *voice is enabled*, which is the reading TECHDEBT 29 closed and the one a
/// person expects.
pub fn disabled() -> bool {
    koru_env::switch(DISABLE_VAR)
}

/// The same question the way most call sites want to read it.
pub fn available() -> bool {
    !disabled()
}

#[cfg(test)]
mod tests;
