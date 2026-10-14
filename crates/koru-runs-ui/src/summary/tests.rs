// SPDX-License-Identifier: MPL-2.0

//! A run in a few words, asserted without a surface.

use koru_delegate_core::{
    Attention, AttentionSet, FailureStage, PriorAttempt, RunFailureReason, RunStatus, StepKind,
    TurnOriginator,
};
use koru_i18n::{Key, Lang, t};

use super::*;

fn every_status() -> Vec<RunStatus> {
    vec![
        RunStatus::Created,
        RunStatus::Starting,
        RunStatus::running(),
        RunStatus::Running {
            attention: AttentionSet::new([Attention::ApprovalRequired]),
            control: Default::default(),
        },
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended,
        },
        RunStatus::Completed,
        RunStatus::Failed {
            stage: FailureStage::ProviderStart,
        },
        RunStatus::Cancelled,
        RunStatus::AwaitingReview,
        RunStatus::WaitingForParticipant,
    ]
}

/// **Every status has a word in every language**, and it is the run pill's
/// word — the vocabulary a pill in a paragraph has always used.
#[test]
fn every_status_says_its_pill_word_in_every_language() {
    for status in every_status() {
        for lang in Lang::ALL {
            let said = summary(lang, &status, None, None);
            assert!(!said.status.trim().is_empty(), "{status:?} in {lang:?}");
            assert_eq!(said.activity, None);
            assert_eq!(said.elapsed, None);
        }
    }
    let en = |status: RunStatus| summary(Lang::En, &status, None, None).status;
    assert_eq!(
        en(RunStatus::running()),
        t(Lang::En, Key::DelegateRunWorking)
    );
    assert_eq!(
        en(RunStatus::Completed),
        t(Lang::En, Key::DelegateRunFinished)
    );
    assert_eq!(
        en(RunStatus::Cancelled),
        t(Lang::En, Key::DelegateRunStopped)
    );
    assert_eq!(
        en(RunStatus::Running {
            attention: AttentionSet::new([Attention::WaitingForInput]),
            control: Default::default(),
        }),
        t(Lang::En, Key::DelegateRunNeedsYou)
    );
}

/// **The newest step reads as Activity reads it**, and the elapsed time as
/// Overview's *Worked for* does — one set of words for both surfaces.
#[test]
fn the_newest_step_and_the_elapsed_time_are_the_run_views_own_words() {
    let step = StepKind::ToolsCompleted {
        calls: 3,
        failed: 0,
        failures: Default::default(),
    };
    for lang in Lang::ALL {
        let said = summary(lang, &RunStatus::running(), Some(step), Some(125_000));
        assert_eq!(said.activity, Some(crate::words::step(lang, step)));
        assert_eq!(
            said.elapsed,
            Some(crate::words::duration(lang, Some(125_000)))
        );
        assert!(!said.activity.unwrap().trim().is_empty());
    }
    let turn = summary(
        Lang::En,
        &RunStatus::running(),
        Some(StepKind::TurnStarted {
            originator: TurnOriginator::Orchestrator,
        }),
        None,
    );
    assert_eq!(
        turn.activity.as_deref(),
        Some(t(Lang::En, Key::RunStepTurnStartedAgent))
    );
}

/// A failed run says why: the retained cause when there is one, the stage
/// otherwise. Nothing else has a failure sentence.
#[test]
fn a_failed_run_says_why_and_nothing_else_does() {
    let failed = RunStatus::Failed {
        stage: FailureStage::ProviderStart,
    };
    for lang in Lang::ALL {
        let with_reason = failed_because(
            lang,
            &failed,
            Some(RunFailureReason::AuthenticationRequired),
        )
        .expect("a failed run has a reason");
        assert_eq!(
            with_reason,
            crate::words::failure_reason(lang, RunFailureReason::AuthenticationRequired)
        );
        let without = failed_because(lang, &failed, None).expect("the stage stands in");
        assert_eq!(without, crate::words::status(lang, &failed));
        assert!(!without.trim().is_empty());
    }
    for status in every_status() {
        if !matches!(status, RunStatus::Failed { .. }) {
            assert_eq!(failed_because(Lang::En, &status, None), None, "{status:?}");
        }
    }
}
