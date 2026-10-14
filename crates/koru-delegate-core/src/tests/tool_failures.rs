// SPDX-License-Identifier: MPL-2.0

//! The breakdown a `ToolsCompleted` step carries: bounded by type, honest
//! about what it could not fit, and content-free by construction.

use super::*;

use crate::{ToolFailure as F, ToolKind as T};

fn built(pairs: &[(T, F, u32)]) -> ToolFailures {
    let mut failures = ToolFailures::default();
    for (tool, failure, calls) in pairs {
        for _ in 0..*calls {
            failures.record(*tool, *failure);
        }
    }
    failures
}

#[test]
fn the_same_pair_counts_up_and_a_new_pair_takes_a_row() {
    let failures = built(&[
        (T::CommandExecution, F::Errored, 3),
        (T::FileChange, F::Denied, 1),
        (T::CommandExecution, F::Denied, 2),
    ]);
    assert_eq!(
        failures.iter().collect::<Vec<_>>(),
        vec![
            ToolFailureTally {
                tool: T::CommandExecution,
                failure: F::Errored,
                calls: 3,
            },
            ToolFailureTally {
                tool: T::FileChange,
                failure: F::Denied,
                calls: 1,
            },
            ToolFailureTally {
                tool: T::CommandExecution,
                failure: F::Denied,
                calls: 2,
            },
        ],
        "pairs are listed in the order they first failed"
    );
    assert_eq!(failures.total(), 6);
    assert_eq!(failures.elided(), 0);
}

/// **The tail is counted, never dropped.** A step whose failures span more
/// distinct pairs than the bound still adds up to the `failed` it claims.
#[test]
fn pairs_past_the_bound_are_counted_rather_than_lost() {
    let mut failures = ToolFailures::default();
    let mut offered = 0_u32;
    for tool in T::ALL {
        for failure in F::ALL {
            failures.record(tool, failure);
            offered += 1;
        }
    }
    assert!(offered > MAX_TOOL_FAILURE_KINDS as u32);
    assert_eq!(failures.len(), MAX_TOOL_FAILURE_KINDS);
    assert_eq!(
        failures.elided(),
        offered - MAX_TOOL_FAILURE_KINDS as u32,
        "every failure past the bound is still accounted for"
    );
    assert_eq!(failures.total(), offered);
}

/// The size bound is the type's, not a convention: a turn that failed sixty
/// times costs the same as one that failed twice.
#[test]
fn the_step_is_the_same_size_however_many_calls_failed() {
    let small = built(&[(T::WebSearch, F::TimedOut, 1)]);
    let huge = {
        let mut failures = ToolFailures::default();
        for tool in T::ALL {
            for failure in F::ALL {
                for _ in 0..64 {
                    failures.record(tool, failure);
                }
            }
        }
        failures
    };
    assert_eq!(size_of_val(&small), size_of_val(&huge));
    assert_eq!(huge.total(), 64 * (T::ALL.len() * F::ALL.len()) as u32);
    // Whatever the padding turns out to be, a step stays a step: the whole
    // closed vocabulary plus its counts fits in tens of bytes, and it
    // cannot grow with the run. D45's affordability argument is a property
    // of the type, so this is where it is asserted.
    assert!(size_of::<StepKind>() <= 64, "{}", size_of::<StepKind>());
}

/// **An entry written before this build still reads.** The field is absent
/// from its JSON, which is the ordinary tolerance the document codec
/// promises, and the step is the counts alone.
#[test]
fn a_step_from_an_older_build_reads_as_the_counts_alone() {
    let older: StepKind = serde_json::from_value(
        serde_json::json!({"step": "tools_completed", "calls": 6, "failed": 2}),
    )
    .unwrap();
    let StepKind::ToolsCompleted {
        calls,
        failed,
        failures,
    } = older
    else {
        panic!("not a tool step: {older:?}");
    };
    assert_eq!((calls, failed), (6, 2));
    assert!(failures.is_empty());
    assert_eq!(failures.total(), 0);
}

/// And a step with nothing to say writes nothing: the ordinary turn, in
/// which no call failed, costs not one byte more than it used to.
#[test]
fn a_turn_with_no_failures_serializes_exactly_as_it_always_did() {
    let step = StepKind::ToolsCompleted {
        calls: 6,
        failed: 0,
        failures: ToolFailures::default(),
    };
    assert_eq!(
        serde_json::to_value(step).unwrap(),
        serde_json::json!({"step": "tools_completed", "calls": 6, "failed": 0})
    );
}

#[test]
fn a_breakdown_survives_the_round_trip_and_a_longer_list_folds_into_the_tail() {
    let failures = built(&[
        (T::CommandExecution, F::Errored, 2),
        (T::McpTool, F::Unclassified, 1),
    ]);
    let step = StepKind::ToolsCompleted {
        calls: 9,
        failed: 3,
        failures,
    };
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(serde_json::from_value::<StepKind>(json).unwrap(), step);

    // A newer writer listed more pairs than this build lists. Reading it
    // keeps the bound and moves the rest into the tail rather than
    // truncating them away.
    let long: Vec<_> = (0..MAX_TOOL_FAILURE_KINDS + 3)
        .map(|n| serde_json::json!({"tool": "mcp_tool", "failure": "denied", "calls": n + 1}))
        .collect();
    let read: ToolFailures =
        serde_json::from_value(serde_json::json!({"tallies": long, "elided": 0})).unwrap();
    assert_eq!(read.len(), MAX_TOOL_FAILURE_KINDS);
    let offered: u32 = (1..=(MAX_TOOL_FAILURE_KINDS as u32 + 3)).sum();
    assert_eq!(read.total(), offered);
}

/// **Standing requirement 7, at the type.** Every string a breakdown
/// serializes to is a token from a closed list: nothing long, nothing with
/// whitespace, nothing a provider wrote.
#[test]
fn every_string_in_a_breakdown_is_a_token() {
    let mut failures = ToolFailures::default();
    for tool in T::ALL {
        for failure in F::ALL {
            failures.record(tool, failure);
        }
    }
    let entry = TimelineEntry::new(
        StepKind::ToolsCompleted {
            calls: 99,
            failed: 35,
            failures,
        },
        TimelineRefs::default(),
        1_000,
    );
    fn walk(value: &serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                assert!(text.len() <= 36, "a string this long is content: {text}");
                assert!(!text.contains(char::is_whitespace), "{text}");
            }
            serde_json::Value::Array(items) => items.iter().for_each(walk),
            serde_json::Value::Object(fields) => fields.values().for_each(walk),
            _ => {}
        }
    }
    walk(&serde_json::to_value(&entry).unwrap());
    // And the whole entry stays small enough that D32 — keep everything,
    // forever — is still affordable at the busiest step a turn can write.
    let bytes = serde_json::to_vec(&entry).unwrap().len();
    assert!(bytes <= 1_024, "a step of {bytes} bytes is not lean");
}
