// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn connect_preserves_pending_siblings_and_uncertain_retry_keeps_the_exact_set() {
    let mut c = ProfilesController::new();
    c.set_account(AccountState {
        active_scope: 1,
        generation: 10,
        stage: AccountStage::SignedIn,
        ..Default::default()
    });
    c.take_request();
    let sibling = ProfileId::new();
    let target = ProfileId::new();
    let unauthorized = ProfileId::new();
    c.complete(AccountOperationResult::Profiles {
        profiles: [sibling, target]
            .into_iter()
            .map(|profile| AccountProfileRow {
                profile_id: profile.to_storage(),
                koru_identity_id: "10000000-0000-4000-8000-000000000001".into(),
                name: Some("Profile".into()),
                locale: None,
                auth_identities: vec![],
            })
            .collect(),
    });
    c.account.enrollments = [sibling, unauthorized]
        .into_iter()
        .map(|profile| koru_agent_wire::AccountProfileEnrollment {
            profile_id: profile.to_storage(),
            state: koru_agent_wire::EnrollmentState::Waiting,
            failure_code: None,
            retry_at_millis: None,
        })
        .collect();
    c.take_request();
    c.begin_inline_connect(target);
    let (command, Some(submission)) = c.take_request_with_submission().unwrap() else {
        panic!()
    };
    let mut expected = vec![sibling.to_storage(), target.to_storage()];
    expected.sort();
    assert_eq!(
        command,
        AccountCommand::SelectProfiles {
            profile_ids: expected
        }
    );
    c.account_command_failed(CommandFailure {
        submission,
        uncertain: true,
    });
    c.account.stage = AccountStage::Unavailable;
    // The uncertain attempt keeps its row and offers a retry of it alone.
    assert_eq!(c.inline_connect(), Some((target, true)));
    assert!(
        c.missing_account_profiles()
            .iter()
            .any(|row| row.profile_id == target.to_storage())
    );
    c.account.enrollments.clear(); // A newer projection cannot rewrite this retry.
    c.perform(Intent::Connect(target));
    let (retry, Some(next_submission)) = c.take_request_with_submission().unwrap() else {
        panic!()
    };
    assert_eq!(retry, command);
    assert_ne!(next_submission, submission);
    assert!(!c.account_command_admitted(CommandAdmission {
        submission,
        owner_scope: 1,
        generation: 11
    }));
}

#[test]
fn connect_stays_in_manager_through_both_catalog_and_admission_orderings() {
    for catalog_first in [true, false] {
        let mut c = ProfilesController::new();
        let current = ProfileId::new();
        let target = ProfileId::new();
        let summary = |id| ProfileSummary {
            id,
            name: "Profile".into(),
            email: String::new(),
            avatar: None,
            icon: None,
            color: None,
            account: None,
            folder: String::new(),
        };
        c.set_account(AccountState {
            active_scope: 1,
            generation: 10,
            stage: AccountStage::SignedIn,
            ..Default::default()
        });
        c.set_profiles(vec![summary(current)], current, &[]);
        c.complete(AccountOperationResult::Profiles {
            profiles: vec![AccountProfileRow {
                profile_id: target.to_storage(),
                koru_identity_id: "10000000-0000-4000-8000-000000000001".into(),
                name: Some("Profile".into()),
                locale: None,
                auth_identities: vec![],
            }],
        });
        c.take_request();
        c.begin_inline_connect(target);
        let (_, Some(submission)) = c.take_request_with_submission().unwrap() else {
            panic!()
        };
        if catalog_first {
            c.set_profiles(vec![summary(current), summary(target)], current, &[]);
        }
        let result = AccountOperationResult::ProfilesSelected {
            profiles: vec![koru_agent_wire::AccountProfileEnrollment {
                profile_id: target.to_storage(),
                state: koru_agent_wire::EnrollmentState::Waiting,
                failure_code: None,
                retry_at_millis: None,
            }],
        };
        // Results can arrive before the admission acknowledgement.
        assert!(c.complete_scoped(1, 11, result));
        assert!(c.account_command_admitted(CommandAdmission {
            submission,
            owner_scope: 1,
            generation: 11,
        }));
        if !catalog_first {
            c.set_profiles(vec![summary(current), summary(target)], current, &[]);
        }
        assert!(c.inline_connect.is_none());
        assert_eq!(c.selected, Some(target));
        assert_eq!(c.current, Some(current));
        assert!(c.profile_creation.is_none());
        assert!(c.take_profile_restore_request().is_none());
        assert!(c.take_profile_switch_request().is_none());
        assert_eq!(c.profile_connection_status(current), None);
        assert_eq!(
            c.profile_connection_status(target),
            Some(koru_i18n::t(
                crate::format::lang(),
                koru_i18n::Key::ProfileConnectionWaiting
            ))
        );
        c.account.enrollments[0].state = koru_agent_wire::EnrollmentState::Complete;
        assert_eq!(
            c.profile_connection_status(target),
            Some(koru_i18n::t(
                crate::format::lang(),
                koru_i18n::Key::ProfileConnectionComplete
            ))
        );
    }
}
