// SPDX-License-Identifier: MPL-2.0

use super::*;
#[test]
fn choices_are_authoritative_and_legacy_mapping_is_unique_and_one_time() {
    let mut options = one_time_approval_choices();
    let allow = options[1].id;
    assert_eq!(
        resolve_approval_choice(&options, ApprovalDecision::Reject, Some(allow))
            .unwrap()
            .intent
            .decision(),
        ApprovalDecision::Approve
    );
    assert_eq!(
        resolve_approval_choice(&options, ApprovalDecision::Approve, None)
            .unwrap()
            .id,
        allow
    );
    options.push(ApprovalChoice {
        id: ApprovalChoiceId::mint(),
        intent: ApprovalIntent::Allow,
        label: "private".into(),
    });
    assert_eq!(
        resolve_approval_choice(&options, ApprovalDecision::Approve, None),
        Err(ApprovalChoiceError::Unrepresentable)
    );
    for option in options.iter().filter(|option| option.intent.grants()) {
        assert_eq!(
            resolve_approval_choice(&options, ApprovalDecision::Reject, Some(option.id))
                .unwrap()
                .id,
            option.id
        );
    }
    let remembered = ApprovalChoice {
        id: ApprovalChoiceId::mint(),
        intent: ApprovalIntent::AllowRemembered,
        label: "private remembered".into(),
    };
    assert_eq!(
        resolve_approval_choice(
            std::slice::from_ref(&remembered),
            ApprovalDecision::Approve,
            None
        ),
        Err(ApprovalChoiceError::Unrepresentable)
    );
    assert_eq!(
        resolve_approval_choice(
            std::slice::from_ref(&remembered),
            ApprovalDecision::Reject,
            Some(remembered.id)
        ),
        Ok(&remembered)
    );
    assert_eq!(
        resolve_approval_choice(&options, ApprovalDecision::Approve, Some(remembered.id)),
        Err(ApprovalChoiceError::Stale)
    );
    assert!(!format!("{remembered:?}").contains("private remembered"));
}
