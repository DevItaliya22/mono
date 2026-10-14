// SPDX-License-Identifier: MPL-2.0

//! **A run in a few words** — for a surface too small for the run views.
//!
//! A run pill sits inside somebody's paragraph and a canvas node sits on a
//! canvas; neither has room for Overview, but both have to say where a run
//! is, what it is doing and how long it has been at it — and they have to
//! say it *the same way*, or a person looking at both sees two runs. So the
//! words live here, once, beside the run views' own ([`crate::words`]), and
//! every small surface asks [`summary`].
//!
//! **Everything here is a closed vocabulary.** A status, a step kind and a
//! duration are tokens and numbers (D45); nothing a person or a provider
//! wrote reaches these strings, so they may be drawn, announced and logged.

use koru_delegate_core::{RunFailureReason, RunStatus, StepKind};
use koru_i18n::{Key, Lang, t};

use crate::words;

/// A run, in the words a small surface shows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Summary {
    /// Where the run is, in one word or two: *Starting…*, *Working…*,
    /// *Needs you*, *Finished*. The run pill's own vocabulary.
    pub status: String,
    /// The newest step as a sentence — *Ran 3 tools*, *Waiting for
    /// approval* — when one was given. The same sentence Activity shows for
    /// that step.
    pub activity: Option<String>,
    /// How long the run has worked (§17.2: excluding time interrupted or in
    /// a person's hands), when that is known.
    pub elapsed: Option<String>,
}

/// **The words for one run**, from what its header and newest step say.
///
/// `latest_step` is the newest *step* of the run's timeline (usage facts are
/// not steps); `elapsed_ms` is the header's §17.2 figure. Either may be
/// absent, and then so is its phrase — a missing figure is never a zero.
pub fn summary(
    lang: Lang,
    status: &RunStatus,
    latest_step: Option<StepKind>,
    elapsed_ms: Option<u64>,
) -> Summary {
    Summary {
        status: t(lang, status_key(status)).to_owned(),
        activity: latest_step.map(|kind| words::step(lang, kind)),
        elapsed: elapsed_ms.map(|ms| words::duration(lang, Some(ms))),
    }
}

/// **Why a failed run failed**, in the sentence Overview shows: the
/// retained, content-free cause when the run has one, and otherwise the
/// stage it failed at. `None` for a run that has not failed.
pub fn failed_because(
    lang: Lang,
    status: &RunStatus,
    reason: Option<RunFailureReason>,
) -> Option<String> {
    match status {
        RunStatus::Failed { .. } => Some(match reason {
            Some(reason) => words::failure_reason(lang, reason),
            None => words::status(lang, status),
        }),
        _ => None,
    }
}

/// The word for one status.
///
/// **Every marker is one word.** A run wanting an approval, waiting for input,
/// holding a child session, or having finished a turn with nothing registered
/// (D29) all read *needs you*: which of them it is takes a sentence, and a
/// sentence does not belong inside somebody's paragraph. The run's views say
/// which.
fn status_key(status: &RunStatus) -> Key {
    match status {
        RunStatus::Created | RunStatus::Starting => Key::DelegateRunStarting,
        RunStatus::Running { attention, .. } if !attention.is_empty() => Key::DelegateRunNeedsYou,
        RunStatus::Running { .. } => Key::DelegateRunWorking,
        RunStatus::Interrupted { .. } => Key::DelegateRunInterrupted,
        RunStatus::Completed => Key::DelegateRunFinished,
        RunStatus::Failed { .. } => Key::DelegateRunFailed,
        RunStatus::Cancelled => Key::DelegateRunStopped,
        RunStatus::AwaitingReview => Key::DelegateRunAwaitingReview,
        RunStatus::WaitingForParticipant => Key::DelegateRunWaitingForParticipant,
    }
}

#[cfg(test)]
mod tests;
