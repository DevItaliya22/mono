// SPDX-License-Identifier: MPL-2.0

//! Privacy is structural (§13.7): a sentinel objective, preamble, message
//! delta and approval summary go through a whole run, and none of them
//! reaches a fact, a queued body, a step, a `Debug` rendering or an error.

use koru_session_protocol::{ApprovalId, ToolCallId};

use super::*;

#[test]
fn no_objective_preamble_delta_or_summary_reaches_a_fact_a_body_a_step_or_an_error() {
    let mut h = Harness::new("privacy");
    let run = h.running(1);
    let t = turn(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: t });
    h.agent(
        run,
        AgentEvent::MessageDelta {
            turn_id: t,
            text: "DELTA-SENTINEL".into(),
        },
    );
    let call = ToolCallId::mint();
    h.agent(
        run,
        AgentEvent::ToolStarted {
            turn_id: t,
            tool_call_id: call,
            label: "TOOL-LABEL-SENTINEL".into(),
            input: None,
        },
    );
    // The completion the coalesced step is built from. Its two new fields
    // are tokens an adapter chose; there is no field on it a provider could
    // have written into, and the recorded step is checked below against
    // every sentinel like everything else.
    h.agent(
        run,
        AgentEvent::ToolCompleted {
            turn_id: t,
            tool_call_id: call,
            succeeded: false,
            tool: Some(ToolKind::CommandExecution),
            failure: Some(ToolFailure::Errored),
            detail: None,
            output: None,
        },
    );
    let approval: ApprovalId = v4(0x71).hyphenated().to_string().parse().unwrap();
    h.agent(
        run,
        AgentEvent::ApprovalRequired {
            turn_id: t,
            approval_id: approval,
            summary: "APPROVAL-SUMMARY-SENTINEL".into(),
            choices: Vec::new(),
        },
    );
    h.agent(
        run,
        AgentEvent::ApprovalExpired {
            turn_id: t,
            approval_id: approval,
        },
    );
    // Ends the turn, so the coalesced tool step is actually written and is
    // among the timeline items swept below.
    h.agent(run, AgentEvent::TurnCompleted { turn_id: t });
    h.sessions.lock().unwrap().create = None;
    let error = h.orchestrator.start(run, StartOrigin::Manual).unwrap_err();
    let root = h.dir.root.to_string_lossy().into_owned();
    let sentinels = [
        OBJECTIVE_SENTINEL,
        PREAMBLE_SENTINEL,
        "DELTA-SENTINEL",
        "TOOL-LABEL-SENTINEL",
        "APPROVAL-SUMMARY-SENTINEL",
        root.as_str(),
    ];
    let mut rendered = Vec::new();
    for fact in h.facts() {
        rendered.push(format!("{fact:?}"));
        rendered.push(serde_json::to_string(&fact).unwrap());
    }
    for publication in h.orchestrator.ports().store.open_publications(100).unwrap() {
        rendered.push(format!("{publication:?}"));
    }
    for item in h
        .orchestrator
        .view(run, None, 0, usize::MAX)
        .unwrap()
        .timeline
        .items
    {
        rendered.push(format!("{item:?}"));
    }
    // The sweep is only worth anything if the step it is meant to cover is
    // in it, so say so out loud rather than trusting the fold.
    assert!(
        rendered.iter().any(|text| text.contains("ToolsCompleted")
            && text.contains("CommandExecution")
            && text.contains("Errored")),
        "the recorded tool step is not among what was swept: {rendered:?}"
    );
    rendered.push(format!("{error:?}"));
    rendered.push(error.to_string());
    rendered.push(format!("{:?}", h.orchestrator.header(run).unwrap().status));
    rendered.push(format!("{:?}", h.attempt(run)));
    for text in rendered {
        for sentinel in sentinels {
            assert!(!text.contains(sentinel), "{sentinel} leaked into {text}");
        }
    }
    // The one place the objective and the preamble go: the provider.
    let submitted = h
        .calls()
        .iter()
        .any(|call| matches!(call, Call::Submit { text, .. } if text.contains(OBJECTIVE_SENTINEL)));
    assert!(submitted);
}

#[test]
fn a_fact_carries_no_text_field() {
    // Every variant serializes to ids, numbers, booleans and closed
    // vocabulary tokens: no JSON string longer than a UUID or a token.
    let fact = RunFact::Created {
        run: run_id(1),
        workspace: workspace(),
        document: DocumentId::from_uuid(v4(2)),
        context_items: 3,
    };
    let json = serde_json::to_value(&fact).unwrap();
    fn walk(value: &serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                assert!(text.len() <= 36, "{text}");
                assert!(!text.contains(' '), "{text}");
            }
            serde_json::Value::Array(items) => items.iter().for_each(walk),
            serde_json::Value::Object(fields) => fields.values().for_each(walk),
            _ => {}
        }
    }
    walk(&json);
}
