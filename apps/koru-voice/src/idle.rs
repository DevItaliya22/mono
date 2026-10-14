// SPDX-License-Identifier: MPL-2.0

//! **Letting go of the model, and then of the process.**
//!
//! This is the reason a sixth binary exists at all. Several hundred megabytes
//! of model memory are only *genuinely* released by ending the process that
//! holds them — a native runtime's allocator hands very little back to the
//! operating system, and a window that had loaded a recogniser once would carry
//! it for the rest of the session
//! ([ADR-249](../../../docs/decisions/ADR-226-250.md)). So the worker unloads
//! and then exits, and this module is that decision as a value so it can be
//! tested without waiting half an hour.
//!
//! ```text
//! no session for KORU_VOICE_IDLE_UNLOAD (300 s)   -> drop the engine and the lease
//! no session for KORU_VOICE_IDLE_EXIT   (1800 s)  -> the process ends
//! ```
//!
//! ## Why two numbers and not one
//!
//! Because they answer different questions. Unloading is about memory and is
//! cheap to undo — the bytes are on the disk and reloading costs seconds.
//! Exiting is about the process itself, and undoing it costs a launch through
//! Agent's supervision. A person who dictates every few minutes should keep the
//! worker and lose the model; a person who dictated once this morning should
//! keep neither.
//!
//! ## And why this is not a timer
//!
//! It is a deadline, which is a different thing. The inference thread blocks on
//! its command channel with exactly [`IdleAction::Wait`]'s duration, so an idle
//! worker wakes **once**, at the moment something is due, rather than several
//! times a second forever. That is the same rule the window's event loop
//! follows — `min(caret deadline, Session::next_due())` — and the reason
//! `about_to_wait` is not allowed to grow work.

use std::time::Duration;

/// How long the worker keeps a model, and itself, with nothing to do.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct IdlePolicy {
    /// Zero means never — a developer keeping a loaded worker alive across a
    /// morning of experiments, and the reason the value is read rather than
    /// compiled in.
    pub unload_after: Duration,
    pub exit_after: Duration,
}

/// What the worker should do about being idle right now.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum IdleAction {
    /// Nothing is due for this long. The inference thread blocks for exactly
    /// this, so an idle process wakes once rather than polling.
    Wait(Duration),
    /// Drop the engine and release the model lease. The worker keeps answering.
    UnloadModel,
    /// End the process.
    Exit,
}

impl IdlePolicy {
    /// Five minutes, and it is a judgement about *people* rather than about
    /// memory: a person who dictates a sentence, reads it, and dictates another
    /// is inside five minutes, and paying a several-hundred-megabyte load twice
    /// for that would make the second dictation feel broken.
    pub const DEFAULT_UNLOAD: Duration = Duration::from_secs(300);
    /// Half an hour. Long enough that a working session never notices it, short
    /// enough that a machine left on overnight is not holding a worker for a
    /// dictation nobody is going to start.
    pub const DEFAULT_EXIT: Duration = Duration::from_secs(1800);

    /// The defaults, with `KORU_VOICE_IDLE_UNLOAD` and `KORU_VOICE_IDLE_EXIT`
    /// over the top when they are set.
    ///
    /// A value that is not a number is **named and ignored**, not fatal: this
    /// decides when memory is released, and refusing to start a worker because
    /// somebody typed `5m` instead of `300` would be a worse failure than
    /// holding a model five minutes longer than they wanted.
    pub fn from_env() -> IdlePolicy {
        let read = |name: &'static str, fallback| {
            parse_seconds(name, std::env::var(name).ok().as_deref(), fallback)
        };
        IdlePolicy {
            unload_after: read(koru_env::name::KORU_VOICE_IDLE_UNLOAD, Self::DEFAULT_UNLOAD),
            exit_after: read(koru_env::name::KORU_VOICE_IDLE_EXIT, Self::DEFAULT_EXIT),
        }
    }

    /// What to do, having had nothing to do for `idle_for`.
    ///
    /// **Exit is checked first**, so a policy whose exit deadline is inside its
    /// unload deadline behaves as its numbers say rather than unloading a model
    /// out of a process that is about to end anyway.
    pub fn decide(&self, idle_for: Duration, model_loaded: bool) -> IdleAction {
        let exit_due = self.exit_after > Duration::ZERO && idle_for >= self.exit_after;
        if exit_due {
            return IdleAction::Exit;
        }
        let unload_due =
            model_loaded && self.unload_after > Duration::ZERO && idle_for >= self.unload_after;
        if unload_due {
            return IdleAction::UnloadModel;
        }
        let mut wait = Duration::from_secs(60);
        if self.exit_after > Duration::ZERO {
            wait = wait.min(self.exit_after.saturating_sub(idle_for));
        }
        if model_loaded && self.unload_after > Duration::ZERO {
            wait = wait.min(self.unload_after.saturating_sub(idle_for));
        }
        // Never zero: a zero wait would turn `recv_timeout` into a spin, which
        // is the failure this whole module is arranged to avoid.
        IdleAction::Wait(wait.max(Duration::from_millis(1)))
    }
}

impl Default for IdlePolicy {
    fn default() -> IdlePolicy {
        IdlePolicy {
            unload_after: IdlePolicy::DEFAULT_UNLOAD,
            exit_after: IdlePolicy::DEFAULT_EXIT,
        }
    }
}

/// One variable's value as a duration.
///
/// Takes the raw value rather than reading the process environment, so the rule
/// can be tested without mutating a global that every other suite in this
/// binary shares — `std::env::set_var` is a data race against any thread
/// reading one, which under cargo's parallel runner is all of them.
fn parse_seconds(name: &'static str, raw: Option<&str>, fallback: Duration) -> Duration {
    let Some(raw) = raw else {
        return fallback;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return fallback;
    }
    match trimmed.parse::<u64>() {
        Ok(seconds) => Duration::from_secs(seconds),
        Err(_) => {
            tracing::warn!(
                variable = name,
                value = trimmed,
                "not a whole number of seconds; using the default"
            );
            fallback
        }
    }
}

#[cfg(test)]
mod tests;
