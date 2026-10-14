// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_session_protocol::{
    APPROVAL_UNREPRESENTABLE_MESSAGE, ApprovalChoice, ApprovalChoiceId, ApprovalIntent,
};

fn offered(
    options: Vec<acp::PermissionOption>,
) -> (Harness, AgentTurnId, ApprovalId, Vec<ApprovalChoice>) {
    let mut h = harness(
        FakeAcpConfig {
            permission_options: Some(options),
            ..Default::default()
        },
        None,
        None,
    );
    let caps = start(&mut h.control);
    assert!(caps.capabilities.supports(capability::DYNAMIC_APPROVALS));
    let turn = AgentTurnId::mint();
    h.control.submit(input(turn)).unwrap();
    let events = drain(&mut h.control);
    let (id, choices) = events
        .into_iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired {
                approval_id,
                choices,
                ..
            } => Some((approval_id, choices)),
            _ => None,
        })
        .unwrap();
    (h, turn, id, choices)
}

fn answers(h: &Harness, id: &str) -> Vec<Value> {
    h.state
        .lock()
        .unwrap()
        .sent()
        .iter()
        .filter(|frame| frame["id"] == id)
        .cloned()
        .collect()
}

#[test]
fn every_asymmetric_and_remembered_choice_roundtrips_and_normalizes_history() {
    let shapes = [
        vec![
            acp::PermissionOptionKind::AllowOnce,
            acp::PermissionOptionKind::RejectAlways,
        ],
        vec![
            acp::PermissionOptionKind::AllowAlways,
            acp::PermissionOptionKind::RejectOnce,
        ],
        vec![
            acp::PermissionOptionKind::AllowAlways,
            acp::PermissionOptionKind::RejectAlways,
        ],
    ];
    for kinds in shapes {
        for selected in 0..kinds.len() {
            let options: Vec<_> = kinds
                .iter()
                .enumerate()
                .map(|(i, kind)| {
                    acp::PermissionOption::new(
                        format!("<option-{i}>"),
                        format!("<label-{i}>"),
                        *kind,
                    )
                })
                .collect();
            let (mut h, turn, id, choices) = offered(options);
            assert_eq!(choices.len(), kinds.len());
            for (i, choice) in choices.iter().enumerate() {
                assert_eq!(choice.label, format!("<label-{i}>"));
            }
            let chosen = &choices[selected];
            let expected = chosen.intent.decision();
            let contradiction = if expected == ApprovalDecision::Approve {
                ApprovalDecision::Reject
            } else {
                ApprovalDecision::Approve
            };
            h.control
                .answer_approval(id, contradiction, Some(chosen.id))
                .unwrap();
            assert_eq!(
                answers(&h, PERMISSION_ID)[0]["result"]["outcome"],
                json!({"outcome":"selected","optionId":format!("<option-{selected}>")})
            );
            assert!(
                matches!(drain(&mut h.control).as_slice(), [AgentEvent::ApprovalResolved { turn_id, approval_id, decision, choice }] if *turn_id==turn && *approval_id==id && *decision==expected && *choice==Some(chosen.id))
            );
            let writes = h.state.lock().unwrap().sent().len();
            assert_eq!(
                h.control
                    .answer_approval(id, expected, Some(chosen.id))
                    .unwrap_err()
                    .code,
                ErrorCode::ApprovalStale
            );
            assert_eq!(h.state.lock().unwrap().sent().len(), writes);
            assert!(drain(&mut h.control).is_empty());
        }
    }
}

#[test]
fn duplicate_intents_are_distinct_options_and_each_new_client_choice_selects_correctly() {
    for intent in [
        acp::PermissionOptionKind::AllowOnce,
        acp::PermissionOptionKind::RejectOnce,
        acp::PermissionOptionKind::AllowAlways,
        acp::PermissionOptionKind::RejectAlways,
    ] {
        for selected in [0, 1] {
            let (mut h, _, id, choices) =
                offered(vec![option("<first>", intent), option("<second>", intent)]);
            assert_ne!(choices[0].id, choices[1].id);
            h.control
                .answer_approval(id, ApprovalDecision::Approve, Some(choices[selected].id))
                .unwrap();
            assert_eq!(
                answers(&h, PERMISSION_ID)[0]["result"]["outcome"]["optionId"],
                if selected == 0 { "<first>" } else { "<second>" }
            );
        }
    }
}

#[test]
fn legacy_ambiguous_or_remembered_only_offer_selects_nothing_and_expires_before_failed_turn() {
    for (options, decision) in [
        (
            vec![
                option("<a>", acp::PermissionOptionKind::AllowOnce),
                option("<b>", acp::PermissionOptionKind::AllowOnce),
            ],
            ApprovalDecision::Approve,
        ),
        (
            vec![
                option("<a>", acp::PermissionOptionKind::RejectOnce),
                option("<b>", acp::PermissionOptionKind::RejectOnce),
            ],
            ApprovalDecision::Reject,
        ),
        (
            vec![
                option("<a>", acp::PermissionOptionKind::AllowAlways),
                option("<b>", acp::PermissionOptionKind::RejectAlways),
            ],
            ApprovalDecision::Approve,
        ),
        (
            vec![
                option("<a>", acp::PermissionOptionKind::AllowAlways),
                option("<b>", acp::PermissionOptionKind::RejectAlways),
            ],
            ApprovalDecision::Reject,
        ),
    ] {
        let (mut h, turn, id, _) = offered(options);
        h.control.answer_approval(id, decision, None).unwrap();
        assert_eq!(
            answers(&h, PERMISSION_ID)[0]["result"]["outcome"],
            json!({"outcome":"cancelled"})
        );
        let events = drain(&mut h.control);
        assert!(
            matches!(events.as_slice(), [AgentEvent::ApprovalExpired { approval_id, .. }, AgentEvent::TurnFailed { turn_id, error }] if *approval_id==id && *turn_id==turn && error.code==ErrorCode::ProviderFailed && error.message==APPROVAL_UNREPRESENTABLE_MESSAGE)
        );
        assert_eq!(h.state.lock().unwrap().cancels, 1);
        assert!(h.control.approvals.is_empty());
    }
}

#[test]
fn legacy_unique_one_time_matches_even_beside_remembered_option() {
    for decision in [ApprovalDecision::Approve, ApprovalDecision::Reject] {
        let (mut h, _, id, _) = offered(vec![
            option("<allow>", acp::PermissionOptionKind::AllowOnce),
            option("<reject>", acp::PermissionOptionKind::RejectOnce),
            option("<remember>", acp::PermissionOptionKind::AllowAlways),
        ]);
        h.control.answer_approval(id, decision, None).unwrap();
        assert_eq!(
            answers(&h, PERMISSION_ID)[0]["result"]["outcome"]["optionId"],
            if decision == ApprovalDecision::Approve {
                "<allow>"
            } else {
                "<reject>"
            }
        );
        assert_eq!(h.state.lock().unwrap().cancels, 0);
    }
}

fn inject_permission(h: &Harness, request: &str, options: Value) {
    h.injected.lock().unwrap().push_back(json!({"jsonrpc":"2.0","id":request,"method":"session/request_permission","params":{
        "sessionId":SESSION,"toolCall":{"toolCallId":"<tool>","title":"<summary>"},"options":options,
    }}));
}

#[test]
fn stale_approval_or_unoffered_choice_never_touches_another_approval_or_later_turn() {
    let (mut h, first_turn, a, choices_a) = offered(permission_options());
    inject_permission(
        &h,
        "<request-b>",
        json!([{"optionId":"<b>","kind":"reject_once","name":"<b label>"}]),
    );
    let b = approval(&drain(&mut h.control));
    let b_choice = h.control.approvals[&b].choices[0].id;
    let writes = h.state.lock().unwrap().sent().len();
    for choice in [choices_a[0].id, ApprovalChoiceId::mint()] {
        assert_eq!(
            h.control
                .answer_approval(b, ApprovalDecision::Approve, Some(choice))
                .unwrap_err()
                .code,
            ErrorCode::ApprovalStale
        );
    }
    assert_eq!(h.state.lock().unwrap().sent().len(), writes);
    assert_eq!(h.control.active, Some(first_turn));
    assert_eq!(h.control.approvals.len(), 2);
    h.control
        .answer_approval(a, ApprovalDecision::Reject, Some(choices_a[0].id))
        .unwrap();
    drain(&mut h.control);
    let writes = h.state.lock().unwrap().sent().len();
    assert_eq!(
        h.control
            .answer_approval(a, ApprovalDecision::Reject, Some(choices_a[0].id))
            .unwrap_err()
            .code,
        ErrorCode::ApprovalStale
    );
    assert_eq!(h.state.lock().unwrap().sent().len(), writes);
    assert!(h.control.approvals.contains_key(&b));
    assert!(drain(&mut h.control).is_empty());
    h.control
        .answer_approval(b, ApprovalDecision::Approve, Some(b_choice))
        .unwrap();
    drain(&mut h.control);
    h.injected.lock().unwrap().push_back(response(
        h.control.turns[&first_turn].request_id,
        json!({"stopReason":"end_turn"}),
    ));
    drain(&mut h.control);
    let later = AgentTurnId::mint();
    h.control.submit(input(later)).unwrap();
    let later_id = approval(&drain(&mut h.control));
    let writes = h.state.lock().unwrap().sent().len();
    assert!(
        h.control
            .answer_approval(a, ApprovalDecision::Approve, None)
            .is_err()
    );
    assert_eq!(h.state.lock().unwrap().sent().len(), writes);
    assert_eq!(h.control.active, Some(later));
    assert!(h.control.approvals.contains_key(&later_id));
    assert!(h.control.turns[&later].cancelling.is_none());
}

#[test]
fn empty_or_future_only_offer_expires_without_fabricating_a_visible_offer() {
    for options in [
        json!([]),
        json!([{"optionId":"<future>","name":"<future label>","kind":"future_permission"}]),
    ] {
        let mut h = harness(FakeAcpConfig::default(), None, None);
        start(&mut h.control);
        let turn = AgentTurnId::mint();
        h.control.submit(input(turn)).unwrap();
        drain(&mut h.control);
        inject_permission(&h, "<unsupported>", options);
        let events = drain(&mut h.control);
        assert_eq!(
            answers(&h, "<unsupported>")[0]["result"]["outcome"],
            json!({"outcome":"cancelled"})
        );
        assert!(
            matches!(events.as_slice(),[AgentEvent::ApprovalExpired {turn_id,..}, AgentEvent::TurnFailed{error,..}] if *turn_id==turn && error.message==APPROVAL_UNREPRESENTABLE_MESSAGE)
        );
        assert!(h.control.approvals.is_empty());
    }
}

#[test]
fn future_option_beside_representable_choice_keeps_only_the_actual_supported_choice() {
    let mut h = harness(FakeAcpConfig::default(), None, None);
    start(&mut h.control);
    h.control.submit(input(AgentTurnId::mint())).unwrap();
    drain(&mut h.control);
    inject_permission(
        &h,
        "<mixed>",
        json!([
            {"optionId":"<future>","name":"<future label>","kind":"future_permission"},
            {"optionId":"<known>","name":"<known label>","kind":"reject_always"}
        ]),
    );
    let events = drain(&mut h.control);
    let id = approval(&events);
    let choices = &h.control.approvals[&id].choices;
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].intent, ApprovalIntent::RejectRemembered);
    let choice = choices[0].id;
    h.control
        .answer_approval(id, ApprovalDecision::Approve, Some(choice))
        .unwrap();
    assert_eq!(
        answers(&h, "<mixed>")[0]["result"]["outcome"]["optionId"],
        "<known>"
    );
}

#[test]
fn unrepresentable_permission_ignored_cancel_deadline_forces_one_failure_and_cleanup() {
    let mut h = harness(
        FakeAcpConfig {
            permission_options: Some(vec![option(
                "<remember>",
                acp::PermissionOptionKind::AllowAlways,
            )]),
            ignore_cancel: true,
            flood_after_cancel: true,
            ..Default::default()
        },
        None,
        None,
    );
    start(&mut h.control);
    let turn = AgentTurnId::mint();
    h.control.submit(input(turn)).unwrap();
    let id = approval(&drain(&mut h.control));
    h.control
        .answer_approval(id, ApprovalDecision::Approve, None)
        .unwrap();
    h.control
        .turns
        .get_mut(&turn)
        .unwrap()
        .cancelling
        .as_mut()
        .unwrap()
        .deadline = Instant::now();
    let events = drain(&mut h.control);
    assert!(
        matches!(events.as_slice(),[AgentEvent::ApprovalExpired {..},AgentEvent::TurnFailed {error,..}] if error.message==APPROVAL_UNREPRESENTABLE_MESSAGE)
    );
    let writes = h.state.lock().unwrap().sent().len();
    assert!(h.control.submit(input(AgentTurnId::mint())).is_err());
    assert!(
        h.control
            .answer_approval(id, ApprovalDecision::Approve, None)
            .is_err()
    );
    assert_eq!(h.state.lock().unwrap().sent().len(), writes);
    assert!(h.control.next_event(Duration::MAX).is_err());
    h.control.shutdown().unwrap();
    assert_eq!(h.state.lock().unwrap().shutdowns, 1);
}

#[test]
fn malformed_option_structure_is_a_protocol_failure_without_a_fabricated_offer() {
    for options in [
        json!([{"optionId":"","name":"<label>","kind":"allow_once"}]),
        json!([{"optionId":17,"name":"<label>","kind":"future_permission"}]),
        json!([{"optionId":"<id>","name":false,"kind":"future_permission"}]),
        json!([{"optionId":"<id>","name":"<label>","kind":false}]),
        json!([
            {"optionId":"<same>","name":"<label-a>","kind":"allow_once"},
            {"optionId":"<same>","name":"<label-b>","kind":"future_permission"}
        ]),
    ] {
        let mut h = harness(FakeAcpConfig::default(), None, None);
        start(&mut h.control);
        h.control.submit(input(AgentTurnId::mint())).unwrap();
        drain(&mut h.control);
        inject_permission(&h, "<malformed>", options);
        let events = drain(&mut h.control);
        assert!(
            matches!(events.as_slice(), [AgentEvent::TurnFailed { error, .. }]
            if error.message != APPROVAL_UNREPRESENTABLE_MESSAGE)
        );
        assert_eq!(
            h.control.unusable.as_ref().unwrap().diagnostic,
            ProviderDiagnostic::MalformedFrame
        );
        assert!(answers(&h, "<malformed>").is_empty());
        assert_eq!(h.state.lock().unwrap().cancels, 0);
        assert!(h.control.approvals.is_empty());
        assert!(h.control.submit(input(AgentTurnId::mint())).is_err());
    }
}
