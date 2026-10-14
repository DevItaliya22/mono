// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_session_protocol::{
    AgentEvent, AgentTurnId, ApprovalChoice, ApprovalChoiceId, ApprovalIntent, SessionId,
};
use model::ManagedModel;

fn option(intent: ApprovalIntent) -> ApprovalChoice {
    ApprovalChoice {
        id: ApprovalChoiceId::mint(),
        intent,
        label: "sanitized offered label".into(),
    }
}

fn offer(
    model: &mut ManagedModel,
    session: SessionId,
    sequence: u64,
    id: ApprovalId,
    summary: String,
    choices: Vec<ApprovalChoice>,
) {
    model
        .apply_batch(
            [SequencedAgentEvent {
                session_id: session,
                sequence: StreamSequence::new(sequence).unwrap(),
                event: AgentEvent::ApprovalRequired {
                    turn_id: AgentTurnId::mint(),
                    approval_id: id,
                    summary,
                    choices,
                },
            }],
            true,
        )
        .unwrap();
}

#[test]
fn every_grant_is_refused_on_truncated_content_but_rejections_work() {
    for intent in [
        ApprovalIntent::Allow,
        ApprovalIntent::AllowRemembered,
        ApprovalIntent::Reject,
        ApprovalIntent::RejectRemembered,
    ] {
        let mut model = ManagedModel::default();
        let id = ApprovalId::mint();
        let choice = option(intent);
        offer(
            &mut model,
            SessionId::mint(),
            1,
            id,
            "x".repeat(MANAGED_MESSAGE_BLOCK_BYTE_LIMIT + 1),
            vec![choice.clone()],
        );
        // Contradictory caller decision cannot evade the intent-wide guard.
        assert_eq!(
            model.mark_approval_submitting(id, ApprovalDecision::Reject, Some(choice.id)),
            !intent.grants()
        );
    }
}

#[test]
fn total_labels_and_option_count_are_bounded_and_disable_grants() {
    for choices in [
        vec![ApprovalChoice {
            label: "x".repeat(MANAGED_MESSAGE_BLOCK_BYTE_LIMIT),
            ..option(ApprovalIntent::AllowRemembered)
        }],
        (0..=MANAGED_ITEM_LIMIT)
            .map(|_| option(ApprovalIntent::Allow))
            .collect(),
    ] {
        let mut model = ManagedModel::default();
        let id = ApprovalId::mint();
        let choice = choices[0].id;
        offer(
            &mut model,
            SessionId::mint(),
            1,
            id,
            "details".into(),
            choices,
        );
        assert!(!model.mark_approval_submitting(id, ApprovalDecision::Reject, Some(choice)));
    }
}

#[test]
fn stale_choice_and_old_approval_do_not_change_new_pending_approval() {
    let session = SessionId::mint();
    let mut model = ManagedModel::default();
    let a = ApprovalId::mint();
    let b = ApprovalId::mint();
    let old = option(ApprovalIntent::Allow);
    let current = option(ApprovalIntent::RejectRemembered);
    offer(&mut model, session, 1, a, "a".into(), vec![old.clone()]);
    offer(&mut model, session, 2, b, "b".into(), vec![current.clone()]);
    assert!(!model.mark_approval_submitting(b, ApprovalDecision::Approve, Some(old.id)));
    model
        .apply_batch(
            [SequencedAgentEvent {
                session_id: session,
                sequence: StreamSequence::new(3).unwrap(),
                event: AgentEvent::ApprovalExpired {
                    turn_id: AgentTurnId::mint(),
                    approval_id: a,
                },
            }],
            true,
        )
        .unwrap();
    assert!(!model.mark_approval_submitting(a, ApprovalDecision::Approve, Some(old.id)));
    assert!(model.mark_approval_submitting(b, ApprovalDecision::Approve, Some(current.id)));
    let ManagedItemContent::Approval { state, choice, .. } = &model.approval(b).unwrap().content
    else {
        panic!()
    };
    assert_eq!(
        *state,
        ManagedApprovalState::Submitting(ApprovalDecision::Reject)
    );
    assert_eq!(*choice, Some(current.id));
}

#[test]
fn duplicate_intents_select_exact_identity_and_preserve_resolution() {
    for selected_index in [0, 1] {
        let mut model = ManagedModel::default();
        let session = SessionId::mint();
        let id = ApprovalId::mint();
        let choices = vec![option(ApprovalIntent::Allow), option(ApprovalIntent::Allow)];
        let selected = choices[selected_index].id;
        offer(&mut model, session, 1, id, "details".into(), choices);
        assert!(!model.mark_approval_submitting(id, ApprovalDecision::Approve, None));
        assert!(model.mark_approval_submitting(id, ApprovalDecision::Reject, Some(selected)));
        assert!(!model.mark_approval_submitting(id, ApprovalDecision::Approve, Some(selected)));
        model
            .apply_batch(
                [SequencedAgentEvent {
                    session_id: session,
                    sequence: StreamSequence::new(2).unwrap(),
                    event: AgentEvent::ApprovalResolved {
                        turn_id: AgentTurnId::mint(),
                        approval_id: id,
                        decision: ApprovalDecision::Approve,
                        choice: Some(selected),
                    },
                }],
                true,
            )
            .unwrap();
        let ManagedItemContent::Approval { state, choice, .. } =
            &model.approval(id).unwrap().content
        else {
            panic!()
        };
        assert_eq!(
            *state,
            ManagedApprovalState::Resolved(ApprovalDecision::Approve)
        );
        assert_eq!(*choice, Some(selected));
    }
}

#[test]
fn dynamic_hit_testing_names_the_offered_choice_and_owns_the_panel() {
    use vello::kurbo::Rect;
    let id = ApprovalChoiceId::mint();
    let layout = ManagedLayout {
        approval: Some(ManagedApprovalLayout {
            panel: Rect::new(0.0, 0.0, 100.0, 100.0),
            reject: Rect::ZERO,
            approve: Rect::ZERO,
            choices: vec![(id, Rect::new(10.0, 20.0, 90.0, 50.0))],
        }),
        ..ManagedLayout::default()
    };
    assert_eq!(layout.hit(30.0, 30.0), Some(ManagedHit::ApprovalChoice(id)));
    assert_eq!(layout.hit(30.0, 80.0), None);
}

#[test]
fn keyboard_visits_each_choice_in_both_directions_and_activates_exact_id() {
    use crate::{SessionSurfaceIntent, SessionSurfaceInteraction};
    use std::{cell::RefCell, rc::Rc};
    struct Recorder(Rc<RefCell<Vec<ManagedIntent>>>);
    impl SessionSurfaceInteraction for Recorder {
        fn dispatch(&mut self, intent: SessionSurfaceIntent) {
            if let SessionSurfaceIntent::Managed(intent) = intent {
                self.0.borrow_mut().push(intent);
            }
        }
    }
    let recorded = Rc::new(RefCell::new(Vec::new()));
    let mut view =
        ManagedSessionView::new(Recorder(recorded.clone()), ManagedSessionCopy::default());
    let id = ApprovalId::mint();
    let choices = vec![
        option(ApprovalIntent::Allow),
        option(ApprovalIntent::AllowRemembered),
        option(ApprovalIntent::Reject),
    ];
    let selected = choices[1].id;
    view.apply_batch([SequencedAgentEvent {
        session_id: SessionId::mint(),
        sequence: StreamSequence::new(1).unwrap(),
        event: AgentEvent::ApprovalRequired {
            turn_id: AgentTurnId::mint(),
            approval_id: id,
            summary: "details".into(),
            choices,
        },
    }])
    .unwrap();
    // Default is rejection; Shift+Tab selects the remembered option exactly.
    assert!(view.handle_key(ManagedKey::Tab { reverse: true }).redraw);
    assert_eq!(view.approval_focus(), ApprovalFocus::Choice(selected));
    assert!(view.handle_key(ManagedKey::Enter).redraw);
    assert_eq!(
        *recorded.borrow(),
        vec![ManagedIntent::AnswerApproval {
            approval_id: id,
            decision: ApprovalDecision::Approve,
            choice: Some(selected)
        }]
    );
    assert!(!view.handle_key(ManagedKey::Space).redraw);
}

#[test]
fn real_dynamic_render_clips_short_viewports_scrolls_long_labels_and_reuses_layouts() {
    use vello::{
        Scene,
        kurbo::{Affine, Rect},
    };
    let mut view = ManagedSessionView::new((), ManagedSessionCopy::default());
    let choices = vec![
        ApprovalChoice {
            label: "long provider label ".repeat(80),
            ..option(ApprovalIntent::Allow)
        },
        option(ApprovalIntent::Reject),
    ];
    let reject = choices[1].id;
    view.apply_batch([SequencedAgentEvent {
        session_id: SessionId::mint(),
        sequence: StreamSequence::new(1).unwrap(),
        event: AgentEvent::ApprovalRequired {
            turn_id: AgentTurnId::mint(),
            approval_id: ApprovalId::mint(),
            summary: "provider details".into(),
            choices,
        },
    }])
    .unwrap();
    let mut engine = TextEngine::new();
    let mut canvas = Scene::new();
    for bounds in [
        Rect::new(0.0, 0.0, 20.0, 10.0),
        Rect::new(0.0, 0.0, 150.0, 60.0),
        Rect::new(0.0, 0.0, 280.0, 180.0),
    ] {
        canvas.reset();
        let first = view.draw(&mut engine, &mut canvas, Affine::IDENTITY, bounds, 1.0);
        assert_eq!(first.work.approval_layouts_shaped, 4);
        let panel = view.last_layout.approval.as_ref().unwrap();
        assert!(panel.panel.x0 >= bounds.x0 && panel.panel.x1 <= bounds.x1);
        assert!(panel.panel.y0 >= bounds.y0 && panel.panel.y1 <= bounds.y1);
        for (_, rect) in &panel.choices {
            assert!(rect.x0 >= panel.panel.x0 && rect.x1 <= panel.panel.x1);
            assert!(rect.y0 >= panel.panel.y0 && rect.y1 <= panel.panel.y1);
        }
        canvas.reset();
        assert_eq!(
            view.draw(&mut engine, &mut canvas, Affine::IDENTITY, bounds, 1.0)
                .work
                .approval_layouts_shaped,
            0
        );
    }
    view.set_scroll_offset(f32::MAX);
    let bounds = Rect::new(0.0, 0.0, 280.0, 180.0);
    assert_eq!(
        view.draw(&mut engine, &mut canvas, Affine::IDENTITY, bounds, 1.0)
            .work
            .approval_layouts_shaped,
        0
    );
    let (_, rect) = view
        .last_layout
        .approval
        .as_ref()
        .unwrap()
        .choices
        .iter()
        .find(|(id, _)| *id == reject)
        .unwrap();
    assert_eq!(
        view.last_layout
            .hit(rect.center().x as f32, rect.center().y as f32),
        Some(ManagedHit::ApprovalChoice(reject))
    );
}

#[test]
fn approval_content_is_absent_from_event_and_view_diagnostics() {
    let private = "private provider words";
    let event = AgentEvent::ApprovalRequired {
        turn_id: AgentTurnId::mint(),
        approval_id: ApprovalId::mint(),
        summary: private.into(),
        choices: vec![ApprovalChoice {
            label: private.into(),
            ..option(ApprovalIntent::AllowRemembered)
        }],
    };
    assert!(!format!("{event:?}").contains(private));
    let mut model = ManagedModel::default();
    model
        .apply_batch(
            [SequencedAgentEvent {
                session_id: SessionId::mint(),
                sequence: StreamSequence::new(1).unwrap(),
                event,
            }],
            true,
        )
        .unwrap();
    assert!(!format!("{:?}", model.items()).contains(private));
}

#[test]
fn only_known_content_free_approval_failure_marker_selects_actionable_copy() {
    for (code, message, expected) in [
        (
            ErrorCode::ProviderFailed,
            koru_session_protocol::APPROVAL_UNREPRESENTABLE_MESSAGE,
            true,
        ),
        (ErrorCode::ProviderFailed, "private provider error", false),
        (
            ErrorCode::ProviderUnavailable,
            koru_session_protocol::APPROVAL_UNREPRESENTABLE_MESSAGE,
            false,
        ),
    ] {
        let mut model = ManagedModel::default();
        model
            .apply_batch(
                [SequencedAgentEvent {
                    session_id: SessionId::mint(),
                    sequence: StreamSequence::new(1).unwrap(),
                    event: AgentEvent::TurnFailed {
                        turn_id: AgentTurnId::mint(),
                        error: koru_session_protocol::SessionFailure {
                            code,
                            message: message.to_owned(),
                        },
                    },
                }],
                true,
            )
            .unwrap();
        let ManagedItemContent::TurnState {
            approval_unrepresentable,
            ..
        } = &model.items()[0].content
        else {
            panic!()
        };
        assert_eq!(*approval_unrepresentable, expected);
        assert!(!format!("{:?}", model.items()).contains(message));
    }
    assert!(
        ManagedSessionCopy::default()
            .approval_unrepresentable
            .contains("Update Koru or the agent")
    );
}
