// SPDX-License-Identifier: MPL-2.0

//! Restart classification: §7.6's table as a pure function.
//!
//! On restart the worker finds an attempt in this ledger, asks the session
//! host's **non-creating lookup** about the key it bound, and classifies.
//! Three facts make the answer provable: the key was reserved before the
//! process was spawned, retiring an attempt fences a late create, and
//! `HostHello.instance_id` separates the host that might still hold the
//! session from a host that restarted. The function takes the row, the
//! answer and the current incarnation, and returns what to do.
//!
//! **No path yields a failure.** A restart interrupts (D10), and what it
//! cannot prove it flags for a person; there is no `Failed` arm to return,
//! and the test beside this module enumerates every input to show the
//! function is total.
//!
//! [`LookupAnswer`] mirrors §7.6's four rows. P1.1 mints the wire answer;
//! P1.6 adapts that answer to this enum, so this crate depends on the table
//! and not on the wire.

use koru_delegate_core::{HostInstanceId, PriorAttempt};

use crate::{AttemptPhase, AttemptRow};

/// What the host's non-creating lookup said about the attempt's key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LookupAnswer {
    /// A live session carries the key: it started and is running.
    Known,
    /// The key is reserved and insertion has not completed: a process may
    /// exist right now.
    Creating,
    /// No live session, but a replay entry survives: it started and has
    /// finished.
    KnownEnded,
    /// Neither a session nor a reservation nor a replay entry.
    NoEvidence,
}

impl LookupAnswer {
    pub const ALL: [Self; 4] = [
        Self::Known,
        Self::Creating,
        Self::KnownEnded,
        Self::NoEvidence,
    ];
}

/// Whether an interrupted attempt may be resumed without a person.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ResumePolicy {
    /// Nothing survived; a fresh attempt is safe.
    MayAutoResume,
    /// Earlier work may exist and nothing proves it stopped. A person
    /// decides, told so (§7.6's last row).
    PersonDecides,
}

/// What a restart does with an attempt.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RestartVerdict {
    /// Reattach; replay the timeline; never start a replacement.
    Reattach,
    /// Treat as running. Wait for the reservation to resolve, then re-ask.
    WaitThenReask,
    /// Reconcile from the retained response and the timeline; do not
    /// respawn.
    ReconcileEnded,
    /// The attempt is over (D10: never failed). `prior` is what the Run
    /// document records; `resume` is whether the worker may start a fresh
    /// attempt on its own.
    Interrupted {
        prior: PriorAttempt,
        resume: ResumePolicy,
    },
}

impl RestartVerdict {
    /// The closed vocabulary a verdict is reported under.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Reattach => "reattach",
            Self::WaitThenReask => "wait_then_reask",
            Self::ReconcileEnded => "reconcile_ended",
            Self::Interrupted {
                prior: PriorAttempt::Ended,
                ..
            } => "interrupted",
            Self::Interrupted {
                prior: PriorAttempt::Unknown,
                ..
            } => "interrupted_prior_unknown",
        }
    }

    /// The phase the ledger records for the attempt, if the verdict ends
    /// it: `Ended` on reconciliation, `Interrupted` or `Unknown` on an
    /// interruption. `None` when the attempt continues.
    pub fn records_as(&self) -> Option<AttemptPhase> {
        match self {
            Self::Reattach | Self::WaitThenReask => None,
            Self::ReconcileEnded => Some(AttemptPhase::Ended),
            Self::Interrupted {
                prior: PriorAttempt::Ended,
                ..
            } => Some(AttemptPhase::Interrupted),
            Self::Interrupted {
                prior: PriorAttempt::Unknown,
                ..
            } => Some(AttemptPhase::Unknown),
        }
    }
}

/// §7.6's table. `host_now` is the incarnation the host answered with.
///
/// | Lookup | Verdict |
/// | --- | --- |
/// | `Known` | `Reattach` |
/// | `Creating` | `WaitThenReask` |
/// | `KnownEnded` | `ReconcileEnded` |
/// | `NoEvidence`, no host was ever asked | `Interrupted`, prior ended, may auto-resume |
/// | `NoEvidence`, a different incarnation | `Interrupted`, prior ended, may auto-resume |
/// | `NoEvidence`, the same incarnation | `Interrupted`, prior unknown, a person decides |
///
/// An attempt that never bound a session never asked a host for anything
/// (`bind_session` precedes the create), so nothing can be running for it
/// whichever incarnation answers.
pub fn classify_at_restart(
    attempt: &AttemptRow,
    lookup: LookupAnswer,
    host_now: HostInstanceId,
) -> RestartVerdict {
    match lookup {
        LookupAnswer::Known => RestartVerdict::Reattach,
        LookupAnswer::Creating => RestartVerdict::WaitThenReask,
        LookupAnswer::KnownEnded => RestartVerdict::ReconcileEnded,
        LookupAnswer::NoEvidence => match (attempt.provider_session, attempt.host_instance) {
            (Some(_), Some(asked)) if asked == host_now => RestartVerdict::Interrupted {
                prior: PriorAttempt::Unknown,
                resume: ResumePolicy::PersonDecides,
            },
            _ => RestartVerdict::Interrupted {
                prior: PriorAttempt::Ended,
                resume: ResumePolicy::MayAutoResume,
            },
        },
    }
}

#[cfg(test)]
mod tests;
