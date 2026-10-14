// SPDX-License-Identifier: MPL-2.0

use std::{cell::RefCell, rc::Rc};

use koru_draw::text::TextEngine;
use koru_session_protocol::{
    AgentEvent, AgentTurnId, ApprovalDecision, ApprovalId, ErrorCode, SequencedAgentEvent,
    SessionId, StreamSequence, ToolCallId,
};
use koru_session_view::{
    ApprovalFocus, MANAGED_ITEM_LIMIT, MANAGED_MESSAGE_BLOCK_BYTE_LIMIT, ManagedApprovalState,
    ManagedIntent, ManagedItemContent, ManagedItemKind, ManagedKey, ManagedSessionCopy,
    ManagedSessionView, SessionSurfaceIntent, SessionSurfaceInteraction,
};
use vello::{
    Scene,
    kurbo::{Affine, Rect},
};

#[derive(Clone, Default)]
struct Recorder(Rc<RefCell<Vec<ManagedIntent>>>);

impl SessionSurfaceInteraction for Recorder {
    fn dispatch(&mut self, intent: SessionSurfaceIntent) {
        let SessionSurfaceIntent::Managed(intent) = intent else {
            panic!("managed view emitted a different surface intent");
        };
        self.0.borrow_mut().push(intent);
    }
}

fn sequence(value: u64) -> StreamSequence {
    StreamSequence::new(value).unwrap()
}

fn event(session_id: SessionId, value: u64, event: AgentEvent) -> SequencedAgentEvent {
    SequencedAgentEvent {
        session_id,
        sequence: sequence(value),
        event,
    }
}

fn draw(view: &mut ManagedSessionView<Recorder>) -> koru_session_view::ManagedDrawOutcome {
    let mut engine = TextEngine::new();
    let mut scene = Scene::new();
    view.draw(
        &mut engine,
        &mut scene,
        Affine::IDENTITY,
        Rect::new(0.0, 0.0, 800.0, 600.0),
        1.0,
    )
}

#[test]
fn ten_thousand_token_deltas_coalesce_before_any_layout() {
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let events = (1..=10_000).map(|index| {
        event(
            session_id,
            index,
            AgentEvent::MessageDelta {
                turn_id,
                text: "x".into(),
            },
        )
    });
    let mut view = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    view.set_visible(false);
    let applied = view.apply_batch(events).unwrap();
    assert_eq!(applied.events, 10_000);
    assert_eq!(applied.inserted_items, 1);
    assert_eq!(applied.coalesced_deltas, 9_999);
    assert_eq!(applied.attention_transitions, 1);
    assert!(!applied.redraw);
    assert_eq!(view.transcript().len(), 1);
    assert_eq!(draw(&mut view).work.items_shaped, 0);

    view.set_visible(true);
    let first = draw(&mut view);
    assert_eq!(first.work.items_shaped, 1);
    let stable = draw(&mut view);
    assert_eq!(stable.work.items_shaped, 0);
    assert_eq!(stable.work.cache_hits, 1);
}

#[test]
fn transcript_retention_keeps_the_newest_items_and_one_truncation_notice() {
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let total = MANAGED_ITEM_LIMIT as u64 + 40;
    let events = (1..=total).map(|index| {
        event(
            session_id,
            index,
            AgentEvent::ToolStarted {
                turn_id,
                tool_call_id: ToolCallId::mint(),
                label: format!("tool-{index}"),
                input: None,
            },
        )
    });
    let mut view = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    view.apply_batch(events).unwrap();
    let items = view.transcript().collect::<Vec<_>>();
    assert_eq!(items.len(), MANAGED_ITEM_LIMIT);
    assert_eq!(items[0].content.kind(), ManagedItemKind::Truncated);
    assert!(matches!(
        &items[items.len() - 1].content,
        ManagedItemContent::Tool { label, .. } if label == &format!("tool-{total}")
    ));
}

#[test]
fn approval_is_keyboard_safe_exactly_once_and_stale_state_is_inert() {
    let recorder = Recorder::default();
    let observed = recorder.0.clone();
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let approval_id = ApprovalId::mint();
    let mut view = ManagedSessionView::new(recorder, ManagedSessionCopy::default());
    view.apply_batch([event(
        session_id,
        1,
        AgentEvent::ApprovalRequired {
            turn_id,
            approval_id,
            summary: "Run a bounded command".into(),
            choices: Vec::new(),
        },
    )])
    .unwrap();
    assert_eq!(view.approval_focus(), ApprovalFocus::Reject);
    assert!(view.handle_key(ManagedKey::Tab { reverse: false }).redraw);
    assert_eq!(view.approval_focus(), ApprovalFocus::Approve);
    assert!(view.handle_key(ManagedKey::Enter).redraw);
    assert!(!view.handle_key(ManagedKey::Enter).redraw);
    assert_eq!(
        *observed.borrow(),
        vec![ManagedIntent::AnswerApproval {
            approval_id,
            decision: ApprovalDecision::Approve,
            choice: None,
        }]
    );
    assert!(view.approval_failed(approval_id, ErrorCode::ApprovalStale));
    assert!(!view.handle_key(ManagedKey::Enter).redraw);
    let state = view.transcript().find_map(|item| match item.content {
        ManagedItemContent::Approval { state, .. } => Some(state),
        _ => None,
    });
    assert_eq!(state, Some(ManagedApprovalState::Expired));
}

#[test]
fn dismissed_approval_can_be_reopened_from_its_transcript_row() {
    let session_id = SessionId::mint();
    let approval_id = ApprovalId::mint();
    let mut view = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    view.apply_batch([event(
        session_id,
        1,
        AgentEvent::ApprovalRequired {
            turn_id: AgentTurnId::mint(),
            approval_id,
            summary: "Review me later".into(),
            choices: Vec::new(),
        },
    )])
    .unwrap();
    assert!(view.handle_key(ManagedKey::Escape).redraw);
    draw(&mut view);
    assert!(view.pointer_press(20.0, 20.0).redraw);
    assert_eq!(view.approval_focus(), ApprovalFocus::Reject);
    assert!(view.handle_key(ManagedKey::Enter).redraw);
    assert!(matches!(
        view.transcript().next().map(|item| &item.content),
        Some(ManagedItemContent::Approval {
            state: ManagedApprovalState::Submitting(ApprovalDecision::Reject),
            ..
        })
    ));
}

#[test]
fn resolved_approval_stops_owning_keyboard_focus() {
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let approval_id = ApprovalId::mint();
    let mut view = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    view.apply_batch([
        event(
            session_id,
            1,
            AgentEvent::ApprovalRequired {
                turn_id,
                approval_id,
                summary: "No longer pending".into(),
                choices: Vec::new(),
            },
        ),
        event(
            session_id,
            2,
            AgentEvent::ApprovalResolved {
                turn_id,
                approval_id,
                decision: ApprovalDecision::Reject,
                choice: None,
            },
        ),
    ])
    .unwrap();
    assert!(!view.handle_key(ManagedKey::Enter).handled);
}

#[test]
fn cancel_turn_dispatches_once_while_the_request_is_in_flight() {
    let recorder = Recorder::default();
    let observed = recorder.0.clone();
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let mut view = ManagedSessionView::new(recorder, ManagedSessionCopy::default());
    view.apply_batch([event(session_id, 1, AgentEvent::TurnStarted { turn_id })])
        .unwrap();
    assert!(view.handle_key(ManagedKey::CancelTurn).redraw);
    assert!(!view.handle_key(ManagedKey::CancelTurn).redraw);
    assert_eq!(
        *observed.borrow(),
        vec![ManagedIntent::CancelTurn { turn_id }]
    );
}

#[test]
fn oversized_approval_cannot_be_approved() {
    let recorder = Recorder::default();
    let observed = recorder.0.clone();
    let session_id = SessionId::mint();
    let approval_id = ApprovalId::mint();
    let mut view = ManagedSessionView::new(recorder, ManagedSessionCopy::default());
    view.apply_batch([event(
        session_id,
        1,
        AgentEvent::ApprovalRequired {
            turn_id: AgentTurnId::mint(),
            approval_id,
            summary: "x".repeat(MANAGED_MESSAGE_BLOCK_BYTE_LIMIT + 1),
            choices: Vec::new(),
        },
    )])
    .unwrap();
    view.handle_key(ManagedKey::Tab { reverse: false });
    let outcome = view.handle_key(ManagedKey::Enter);
    assert!(outcome.handled);
    assert!(!outcome.redraw);
    assert!(observed.borrow().is_empty());
}

#[test]
fn drawing_does_not_mark_an_approval_read_but_confirmed_presentation_does_once() {
    let recorder = Recorder::default();
    let observed = recorder.0.clone();
    let session_id = SessionId::mint();
    let mut view = ManagedSessionView::new(recorder, ManagedSessionCopy::default());
    view.set_visible(false);
    let applied = view
        .apply_batch([event(
            session_id,
            1,
            AgentEvent::ApprovalRequired {
                turn_id: AgentTurnId::mint(),
                approval_id: ApprovalId::mint(),
                summary: "Review me".into(),
                choices: Vec::new(),
            },
        )])
        .unwrap();
    assert_eq!(applied.attention_transitions, 1);
    assert!(observed.borrow().is_empty());
    view.set_visible(true);
    let frame = draw(&mut view);
    let token = frame.presentation.expect("approval reached the frame");
    assert!(observed.borrow().is_empty());
    assert!(view.presented(token));
    assert!(!view.presented(token));
    assert_eq!(
        *observed.borrow(),
        vec![ManagedIntent::AttentionPresented {
            through: sequence(1),
        }]
    );
}

#[test]
fn visible_layout_work_is_bounded_by_the_viewport() {
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let events = (1..=1_000).map(|index| {
        event(
            session_id,
            index,
            AgentEvent::ToolStarted {
                turn_id,
                tool_call_id: ToolCallId::mint(),
                label: format!("tool-{index}"),
                input: None,
            },
        )
    });
    let mut view = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    view.apply_batch(events).unwrap();
    let frame = draw(&mut view);
    assert!(frame.work.items_considered < 20, "{:#?}", frame.work);
    assert_eq!(frame.work.items_shaped, frame.work.items_considered);
}

#[test]
fn a_different_session_or_sequence_gap_is_rejected_until_reset() {
    let first = SessionId::mint();
    let second = SessionId::mint();
    let turn = AgentTurnId::mint();
    let mut view = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    view.apply_batch([event(first, 1, AgentEvent::TurnStarted { turn_id: turn })])
        .unwrap();
    assert_eq!(
        view.apply_batch([event(
            first,
            3,
            AgentEvent::MessageDelta {
                turn_id: turn,
                text: "gap".into(),
            },
        )])
        .unwrap_err(),
        ErrorCode::StreamGap
    );
    assert_eq!(
        view.apply_batch([event(
            second,
            2,
            AgentEvent::MessageDelta {
                turn_id: turn,
                text: "wrong".into(),
            },
        )])
        .unwrap_err(),
        ErrorCode::StaleGeneration
    );
    view.reset();
    assert!(
        view.apply_batch([event(
            second,
            8,
            AgentEvent::MessageDelta {
                turn_id: turn,
                text: "new surface".into(),
            },
        )])
        .is_ok()
    );
}

#[test]
fn an_invalid_batch_is_rejected_atomically() {
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let mut view = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    view.apply_batch([event(session_id, 1, AgentEvent::TurnStarted { turn_id })])
        .unwrap();
    let before = view.transcript().collect::<Vec<_>>().len();
    assert_eq!(
        view.apply_batch([
            event(
                session_id,
                2,
                AgentEvent::MessageDelta {
                    turn_id,
                    text: "must not leak".into(),
                },
            ),
            event(session_id, 4, AgentEvent::TurnCompleted { turn_id }),
        ])
        .unwrap_err(),
        ErrorCode::StreamGap
    );
    assert_eq!(view.transcript().collect::<Vec<_>>().len(), before);
    assert!(
        view.apply_batch([event(
            session_id,
            2,
            AgentEvent::MessageDelta {
                turn_id,
                text: "accepted after rejection".into(),
            },
        )])
        .is_ok()
    );
}

#[test]
fn equivalent_normalized_provider_fixtures_produce_identical_state() {
    fn fixture(session_id: SessionId, turn_id: AgentTurnId) -> Vec<SequencedAgentEvent> {
        vec![
            event(session_id, 1, AgentEvent::TurnStarted { turn_id }),
            event(
                session_id,
                2,
                AgentEvent::MessageDelta {
                    turn_id,
                    text: "provider-neutral response".into(),
                },
            ),
            event(session_id, 3, AgentEvent::TurnCompleted { turn_id }),
        ]
    }
    let session = SessionId::mint();
    let turn = AgentTurnId::mint();
    let mut codex = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    let mut second = ManagedSessionView::new(Recorder::default(), ManagedSessionCopy::default());
    codex.apply_batch(fixture(session, turn)).unwrap();
    second.apply_batch(fixture(session, turn)).unwrap();
    assert_eq!(
        codex.transcript().collect::<Vec<_>>(),
        second.transcript().collect::<Vec<_>>()
    );
}

/// **A transcript this connection may not act on lays out no control at
/// all** (ADR-354: absent, not disabled), in each of the three states a
/// watched delegated run passes through — working, waiting for an approval,
/// and ended.
///
/// The observer verbs need no controller lease, so a person can read a
/// session another connection is driving; every control in the transcript
/// needs one. A dimmed *Approve* on a session this window cannot answer for
/// would be a promise it could not keep, so read-only draws neither the
/// approval panel nor the cancel, and neither a press nor a key produces an
/// intent to be refused.
#[test]
fn a_read_only_transcript_offers_nothing_a_lease_would_be_needed_for() {
    let recorder = Recorder::default();
    let observed = recorder.0.clone();
    let session_id = SessionId::mint();
    let turn_id = AgentTurnId::mint();
    let approval_id = ApprovalId::mint();
    let mut view = ManagedSessionView::new(recorder, ManagedSessionCopy::default());
    assert!(view.set_read_only(true));
    assert!(view.read_only());

    // Working: a turn is running, so an editable transcript would show
    // *Cancel turn* at the foot of it.
    view.apply_batch([event(session_id, 1, AgentEvent::TurnStarted { turn_id })])
        .unwrap();
    draw(&mut view);
    assert!(
        view.hit_test(780.0, 580.0).is_none(),
        "no cancel control on a session this connection does not hold"
    );

    // Waiting: an approval arrives. No panel opens and no answer can be
    // given — by pointer or by key.
    view.apply_batch([event(
        session_id,
        2,
        AgentEvent::ApprovalRequired {
            turn_id,
            approval_id,
            summary: "Run a bounded command".into(),
            choices: Vec::new(),
        },
    )])
    .unwrap();
    draw(&mut view);
    assert!(!view.pointer_press(20.0, 20.0).handled);
    assert!(!view.handle_key(ManagedKey::Enter).handled);
    assert!(!view.handle_key(ManagedKey::Tab { reverse: false }).handled);
    assert!(!view.handle_key(ManagedKey::CancelTurn).handled);
    assert!(
        observed.borrow().is_empty(),
        "read-only sends nothing: {:?}",
        observed.borrow()
    );

    // Ended: the transcript is still readable, and still offers nothing.
    view.apply_batch([event(session_id, 3, AgentEvent::TurnCompleted { turn_id })])
        .unwrap();
    draw(&mut view);
    assert!(view.hit_test(780.0, 580.0).is_none());
    assert!(view.transcript().len() >= 2, "and it is all still there");

    // Handed the session, the same view becomes the ordinary one: the
    // pending approval is offered again rather than staying lost.
    assert!(view.set_read_only(false));
    assert_eq!(view.approval_focus(), ApprovalFocus::Reject);
    draw(&mut view);
    assert!(view.handle_key(ManagedKey::Enter).handled);
    assert_eq!(
        *observed.borrow(),
        vec![ManagedIntent::AnswerApproval {
            approval_id,
            decision: ApprovalDecision::Reject,
            choice: None,
        }]
    );
}
