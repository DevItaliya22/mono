// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn approval_mutation_identity_distinguishes_choices_and_true_retry_sends_once() {
    let harness = harness();
    let handle = harness.owner.handle();
    let mut state = handle.shared.lock();
    let key = IdempotencyKey::mint();
    let approval_id = ApprovalId::mint();
    let first = koru_session_protocol::ApprovalChoiceId::mint();
    let second = koru_session_protocol::ApprovalChoiceId::mint();
    let mutation = |choice| SessionMutation::Approval {
        approval_id,
        decision: ApprovalDecision::Approve,
        choice: Some(choice),
    };
    let mut sends = 0;
    if !begin_mutation(&mut state, key, mutation(first)).unwrap() {
        sends += 1;
    }
    assert_eq!(
        begin_mutation(&mut state, key, mutation(second)),
        Err(ErrorCode::IdempotencyConflict)
    );
    finish_mutation(&mut state, key, true);
    if !begin_mutation(&mut state, key, mutation(first)).unwrap() {
        sends += 1;
    }
    assert_eq!(sends, 1);
    assert_eq!(
        begin_mutation(&mut state, key, mutation(second)),
        Err(ErrorCode::IdempotencyConflict)
    );
}
