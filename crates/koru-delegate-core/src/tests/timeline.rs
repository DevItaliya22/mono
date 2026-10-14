// SPDX-License-Identifier: MPL-2.0

//! §17.2's definitions over the timeline: elapsed excludes `Interrupted` and
//! human control; an intervention is a handover or a human turn.

use super::*;

use StepKind as K;
use TurnOriginator::{Human, Orchestrator};

#[test]
fn elapsed_is_none_until_the_run_starts() {
    assert_eq!(elapsed_ms(&[], 10_000), None);
    assert_eq!(elapsed_ms(&[entry(K::RunCreated, 1_000)], 10_000), None);
}

#[test]
fn elapsed_counts_up_to_now_while_running_and_stops_at_a_terminal_step() {
    let started = [entry(K::RunCreated, 1_000), entry(K::Started, 2_000)];
    assert_eq!(elapsed_ms(&started, 5_000), Some(3_000));

    let finished = [
        entry(K::RunCreated, 1_000),
        entry(K::Started, 2_000),
        entry(K::Completed, 6_000),
    ];
    assert_eq!(elapsed_ms(&finished, 999_000), Some(4_000));
}

#[test]
fn elapsed_excludes_interrupted_time_and_human_control() {
    let timeline = [
        entry(K::Started, 1_000),
        // 1_000..3_000 running: 2_000
        entry(
            K::Interrupted {
                prior_attempt: PriorAttempt::Ended,
            },
            3_000,
        ),
        // 3_000..10_000 interrupted: excluded
        entry(
            K::Resumed {
                via: ResumeSeam::RecordedContext,
            },
            10_000,
        ),
        // 10_000..11_000 running: 1_000
        entry(K::ControlTaken { discovered: false }, 11_000),
        // 11_000..15_000 a person typing: excluded
        entry(K::TurnStarted { originator: Human }, 12_000),
        entry(K::ControlReturned, 15_000),
        // 15_000..15_500 running: 500
        entry(K::Completed, 15_500),
    ];
    assert_eq!(elapsed_ms(&timeline, 999_000), Some(3_500));
}

#[test]
fn a_takeover_discovered_on_recovery_still_closes_the_interval() {
    let timeline = [
        entry(K::Started, 1_000),
        entry(K::ControlTaken { discovered: true }, 4_000),
    ];
    assert_eq!(elapsed_ms(&timeline, 9_000), Some(3_000));
}

#[test]
fn elapsed_never_goes_negative_on_a_clock_that_went_backwards() {
    let timeline = [entry(K::Started, 5_000), entry(K::Completed, 4_000)];
    assert_eq!(elapsed_ms(&timeline, 0), Some(0));
}

#[test]
fn interventions_are_handovers_plus_human_turns_and_nothing_else() {
    let timeline = [
        entry(K::Started, 1),
        entry(
            K::TurnStarted {
                originator: Orchestrator,
            },
            2,
        ),
        entry(K::ApprovalRequired, 3),
        entry(
            K::ApprovalResolved {
                outcome: ApprovalOutcome::Approved,
            },
            4,
        ),
        entry(K::ControlTaken { discovered: false }, 5),
        entry(K::TurnStarted { originator: Human }, 6),
        entry(K::TurnStarted { originator: Human }, 7),
        entry(K::ControlReturned, 8),
        entry(
            K::TurnStarted {
                originator: Orchestrator,
            },
            9,
        ),
        entry(
            K::ToolsCompleted {
                calls: 12,
                failed: 1,
                failures: ToolFailures::default(),
            },
            10,
        ),
        entry(K::Completed, 11),
    ];
    assert_eq!(interventions(&timeline), 3);
    let summary = EconomicsSummary::project(&Default::default(), &timeline, 0);
    assert_eq!(summary.interventions, 3);
    assert_eq!(summary.orchestrator_turns, 2);
    assert_eq!(summary.human_turns, 2);
    assert_eq!(summary.tool_calls, 12);
    assert_eq!(summary.elapsed_ms, Some(7));
    assert_eq!(summary.turns_with_usage, 0);
    assert!(summary.complete, "no facts is not an incomplete fact");
}

#[test]
fn terminal_step_kinds_are_exactly_the_three() {
    let terminal = [
        K::Completed,
        K::Failed {
            reason: None,
            stage: FailureStage::Turn,
        },
        K::Cancelled {
            reason: CancelReason::Run,
        },
    ];
    for kind in terminal {
        assert!(kind.is_terminal());
    }
    for kind in [
        K::Interrupted {
            prior_attempt: PriorAttempt::Unknown,
        },
        K::Gap { events_lost: None },
        K::Rated {
            rating: Rating::NotGood,
        },
        K::TurnCancelled {
            originator: Orchestrator,
            reason: CancelReason::TakeoverBoundary,
        },
    ] {
        assert!(!kind.is_terminal(), "{kind:?}");
    }
}
