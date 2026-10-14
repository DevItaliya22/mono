// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn recovery_uses_bounded_approval_identity_and_refuses_incomplete_evidence() {
    for evidence in 0..7 {
        let (mut h, run) = mid_turn("approval-restart");
        let required = AgentEvent::ApprovalRequired {
            turn_id: turn(1),
            approval_id: v4(71).to_string().parse().unwrap(),
            summary: "fixture".into(),
            choices: Vec::new(),
        };
        h.agent(run, required.clone());
        let mut h = restarted(
            h,
            (
                host(1),
                CreationLookup::Known {
                    session: summary(ManagedState::WaitingForApproval, Some(turn(1))),
                },
            ),
        );
        {
            let mut script = h.sessions.lock().unwrap();
            script.observation.earlier_truncated = evidence == 1;
            script.observation.cursor =
                koru_session_protocol::StreamSequence::new(if evidence == 2 {
                    1
                } else if evidence == 6 {
                    3
                } else {
                    2
                })
                .unwrap();
            if evidence != 2 && evidence != 3 {
                script.observation.events = vec![koru_session_protocol::SequencedAgentEvent {
                    session_id: session(if evidence == 5 { 2 } else { 1 }),
                    sequence: koru_session_protocol::StreamSequence::new(if evidence == 4 {
                        3
                    } else {
                        2
                    })
                    .unwrap(),
                    event: required,
                }];
            }
        }
        h.orchestrator.restart().unwrap();
        h.session_event(
            run,
            SessionEvent::HandoverChanged {
                handover: koru_session_protocol::HandoverState::Requested,
            },
        );
        assert_eq!(
            h.calls().contains(&Call::Release(session(1))),
            evidence == 0
        );
        assert!(
            h.status(run)
                .attention()
                .unwrap()
                .contains(Attention::ApprovalRequired)
        );
        assert!(
            !h.calls()
                .iter()
                .any(|call| matches!(call, Call::Submit { .. }))
        );
        if evidence != 0 {
            h.agent(
                run,
                AgentEvent::ApprovalResolved {
                    turn_id: turn(1),
                    approval_id: v4(71).to_string().parse().unwrap(),
                    decision: koru_session_protocol::ApprovalDecision::Approve,
                    choice: None,
                },
            );
            assert!(
                h.status(run)
                    .attention()
                    .unwrap()
                    .contains(Attention::ApprovalRequired),
                "one recovered reply cannot prove no unknown approvals remain"
            );
        }
    }
}

fn taken(h: &mut Harness, run: RunId) {
    h.session_event(
        run,
        SessionEvent::HandoverChanged {
            handover: koru_session_protocol::HandoverState::Taken,
        },
    );
}

#[test]
fn already_human_recovery_does_not_record_a_second_takeover() {
    let (mut h, run) = mid_turn("already-human-recovery");
    taken(&mut h, run);
    h.sessions.lock().unwrap().attach_fails = true;
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(report.reattached.len(), 1);
    assert!(
        !h.steps(run)
            .contains(&StepKind::ControlTaken { discovered: true })
    );
    assert!(h.submissions().is_empty());
}

#[test]
fn recovered_original_turn_reattaches_without_replacing_the_prompt() {
    let (mut h, run) = mid_turn("original-return-recovery");
    taken(&mut h, run);
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    h.orchestrator.restart().unwrap();
    assert!(h.steps(run).contains(&StepKind::ControlReturned));
    assert!(h.submissions().is_empty());
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert_eq!(h.status(run), RunStatus::Completed);
}

#[test]
fn recovered_unclassified_turn_is_neither_adopted_nor_replaced() {
    let (mut h, run) = mid_turn("unclassified-recovery");
    taken(&mut h, run);
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(9))),
            },
        ),
    );
    h.orchestrator.restart().unwrap();
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(9) });
    assert!(
        !h.facts()
            .iter()
            .any(|fact| matches!(fact, RunFact::TurnStarted { turn: id, .. } if *id == turn(9)))
    );
    h.session_event(
        run,
        SessionEvent::HandoverChanged {
            handover: koru_session_protocol::HandoverState::Returned,
        },
    );
    assert!(h.submissions().is_empty());
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(9) });
    assert!(matches!(h.status(run), RunStatus::Interrupted { .. }));
    assert!(h.submissions().is_empty());
}

#[test]
fn recovered_human_turn_cannot_satisfy_the_original_objective() {
    let (mut h, run) = mid_turn("human-turn-recovery");
    taken(&mut h, run);
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(2) });
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(2))),
            },
        ),
    );
    h.orchestrator.restart().unwrap();
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(2) });
    assert!(matches!(h.status(run), RunStatus::Interrupted { .. }));
    assert!(h.submissions().is_empty());
}

#[test]
fn ownership_outside_the_bounded_window_is_not_guessed() {
    let (mut h, run) = mid_turn("old-ownership-recovery");
    for _ in 0..260 {
        h.agent(run, AgentEvent::WaitingForInput { turn_id: turn(1) });
    }
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    h.orchestrator.restart().unwrap();
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert!(matches!(h.status(run), RunStatus::Interrupted { .. }));
    assert!(h.submissions().is_empty());
}

#[test]
fn a_recovered_reply_for_another_turn_does_not_close_the_known_approval() {
    let (mut h, run) = mid_turn("wrong-turn-reply-recovery");
    let approval_id = v4(71).to_string().parse().unwrap();
    let required = AgentEvent::ApprovalRequired {
        turn_id: turn(1),
        approval_id,
        summary: "fixture".into(),
        choices: Vec::new(),
    };
    h.agent(run, required.clone());
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::WaitingForApproval, Some(turn(1))),
            },
        ),
    );
    {
        let mut script = h.sessions.lock().unwrap();
        script.observation.cursor = koru_session_protocol::StreamSequence::new(3).unwrap();
        script.observation.events = vec![
            koru_session_protocol::SequencedAgentEvent {
                session_id: session(1),
                sequence: koru_session_protocol::StreamSequence::new(2).unwrap(),
                event: required,
            },
            koru_session_protocol::SequencedAgentEvent {
                session_id: session(1),
                sequence: koru_session_protocol::StreamSequence::new(3).unwrap(),
                event: AgentEvent::ApprovalResolved {
                    turn_id: turn(9),
                    approval_id,
                    decision: koru_session_protocol::ApprovalDecision::Approve,
                    choice: None,
                },
            },
        ];
    }
    h.orchestrator.restart().unwrap();
    h.session_event(
        run,
        SessionEvent::HandoverChanged {
            handover: koru_session_protocol::HandoverState::Requested,
        },
    );
    assert!(h.calls().contains(&Call::Release(session(1))));
    assert!(
        h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
}

#[test]
fn complete_matching_replies_reconcile_attention_without_duplicate_reply_fact() {
    let (mut h, run) = mid_turn("resolved-recovery");
    let approval_id = v4(71).to_string().parse().unwrap();
    let required = AgentEvent::ApprovalRequired {
        turn_id: turn(1),
        approval_id,
        summary: "fixture".into(),
        choices: Vec::new(),
    };
    h.agent(run, required.clone());
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    {
        let mut script = h.sessions.lock().unwrap();
        script.observation.cursor = koru_session_protocol::StreamSequence::new(3).unwrap();
        script.observation.events = vec![
            koru_session_protocol::SequencedAgentEvent {
                session_id: session(1),
                sequence: koru_session_protocol::StreamSequence::new(2).unwrap(),
                event: required,
            },
            koru_session_protocol::SequencedAgentEvent {
                session_id: session(1),
                sequence: koru_session_protocol::StreamSequence::new(3).unwrap(),
                event: AgentEvent::ApprovalResolved {
                    turn_id: turn(1),
                    approval_id,
                    decision: koru_session_protocol::ApprovalDecision::Approve,
                    choice: None,
                },
            },
        ];
    }
    h.orchestrator.restart().unwrap();
    assert!(
        !h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
    assert!(
        !h.facts()
            .iter()
            .any(|fact| matches!(fact, RunFact::ApprovalResolved { .. }))
    );
    assert!(h.submissions().is_empty());
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert_eq!(h.status(run), RunStatus::Completed);
}

#[test]
fn authoritative_original_terminal_closes_truncated_approval_evidence() {
    for failed in [false, true] {
        let (mut h, run) = mid_turn("truncated-original-terminal");
        h.agent(
            run,
            AgentEvent::ApprovalRequired {
                turn_id: turn(1),
                approval_id: v4(71).to_string().parse().unwrap(),
                summary: "fixture".into(),
                choices: Vec::new(),
            },
        );
        let mut h = restarted(
            h,
            (
                host(1),
                CreationLookup::Known {
                    session: summary(ManagedState::WaitingForApproval, Some(turn(1))),
                },
            ),
        );
        h.sessions.lock().unwrap().observation.earlier_truncated = true;
        h.orchestrator.restart().unwrap();
        h.registrations.lock().unwrap().next = vec![artifact(1)];
        let event = if failed {
            AgentEvent::TurnFailed {
                turn_id: turn(1),
                error: koru_session_protocol::SessionFailure {
                    code: koru_session_protocol::ErrorCode::ProviderFailed,
                    message: "fixture".into(),
                },
            }
        } else {
            AgentEvent::TurnCompleted { turn_id: turn(1) }
        };
        h.agent(run, event);
        if failed {
            assert!(matches!(h.status(run), RunStatus::Failed { .. }));
        } else {
            assert_eq!(h.status(run), RunStatus::Completed);
        }
        assert!(h.calls().contains(&Call::Terminate(session(1))));
        assert!(h.submissions().is_empty());
    }
}

#[test]
fn complete_closed_approval_and_original_terminal_reconcile_while_offline() {
    for human_held in [false, true] {
        let (mut h, run) = mid_turn("offline-original-completed");
        let approval_id = v4(71).to_string().parse().unwrap();
        let required = AgentEvent::ApprovalRequired {
            turn_id: turn(1),
            approval_id,
            summary: "fixture".into(),
            choices: Vec::new(),
        };
        h.agent(run, required.clone());
        if human_held {
            taken(&mut h, run);
        }
        let mut h = restarted(
            h,
            (
                host(1),
                CreationLookup::Known {
                    session: summary(ManagedState::Completed, None),
                },
            ),
        );
        {
            let mut script = h.sessions.lock().unwrap();
            script.observation.cursor = koru_session_protocol::StreamSequence::new(4).unwrap();
            script.observation.events = vec![
                koru_session_protocol::SequencedAgentEvent {
                    session_id: session(1),
                    sequence: koru_session_protocol::StreamSequence::new(2).unwrap(),
                    event: required,
                },
                koru_session_protocol::SequencedAgentEvent {
                    session_id: session(1),
                    sequence: koru_session_protocol::StreamSequence::new(3).unwrap(),
                    event: AgentEvent::ApprovalResolved {
                        turn_id: turn(1),
                        approval_id,
                        decision: koru_session_protocol::ApprovalDecision::Approve,
                        choice: None,
                    },
                },
                koru_session_protocol::SequencedAgentEvent {
                    session_id: session(1),
                    sequence: koru_session_protocol::StreamSequence::new(4).unwrap(),
                    event: AgentEvent::TurnCompleted { turn_id: turn(1) },
                },
            ];
        }
        h.registrations.lock().unwrap().next = vec![artifact(1)];
        h.orchestrator.restart().unwrap();
        assert_eq!(h.status(run), RunStatus::Completed);
        assert!(h.submissions().is_empty());
        assert!(
            !h.facts()
                .iter()
                .any(|fact| matches!(fact, RunFact::ApprovalResolved { .. }))
        );
    }
}
