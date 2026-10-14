// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_agent_wire::AccountOperationResult;

const IDENTITY: &str = "00000000-0000-4000-8000-000000000061";

fn local_only(profile: ProfileId, name: &str) -> ProfileSummary {
    ProfileSummary {
        id: profile,
        name: name.into(),
        email: String::new(),
        avatar: None,
        icon: None,
        color: None,
        account: None,
        folder: String::new(),
    }
}

fn signed_in_with(profiles: Vec<ProfileSummary>, selected: ProfileId) -> ProfilesController {
    let mut c = ProfilesController::new();
    // A new c asks for its status first; the flows under test start
    // from an idle queue, as they do once that answer is in.
    c.take_request();
    c.profiles = profiles;
    c.selected = Some(selected);
    c.account = AccountState {
        stage: AccountStage::SignedIn,
        capability: true,
        active_account_id: Some(IDENTITY.into()),
        ..AccountState::default()
    };
    c
}

/// Whether the selected Profile's account button is `control`.
fn offers(c: &ProfilesController, control: LinkControl) -> bool {
    c.account_link_control()
        .is_some_and(|(offered, _)| offered == control)
}

fn hosted_row(profile: ProfileId) -> AccountProfileRow {
    AccountProfileRow {
        koru_identity_id: IDENTITY.into(),
        profile_id: profile.to_storage(),
        name: Some("Steven".into()),
        locale: Some("en".into()),
        auth_identities: Vec::new(),
    }
}

#[test]
fn the_button_is_offered_only_to_a_local_only_profile_while_signed_in() {
    let local = ProfileId::new();
    let owned = ProfileId::new();
    let mut c = signed_in_with(
        vec![
            local_only(local, "Steven"),
            ProfileSummary {
                account: Some(format!("koru:{IDENTITY}")),
                ..local_only(owned, "Work")
            },
        ],
        local,
    );
    assert_eq!(
        c.account_link_control().map(|(control, _)| control),
        Some(LinkControl::Attach(local))
    );
    assert!(offers(&c, LinkControl::Attach(local)));
    assert!(!offers(&c, LinkControl::AddGoogle(local)));

    c.selected = Some(owned);
    assert_eq!(
        c.account_link_control().map(|(control, _)| control),
        Some(LinkControl::AddGoogle(owned))
    );
    assert!(!offers(&c, LinkControl::Attach(owned)));

    c.selected = Some(local);
    c.account.stage = AccountStage::SignedOut;
    assert_eq!(c.account_link_control(), None);
    assert!(!offers(&c, LinkControl::Attach(local)));
}

#[test]
fn nothing_is_sent_until_the_question_is_answered_with_connect() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);

    c.perform(Intent::Attach(local));
    let (_, confirm) = c.attach_prompt.as_ref().expect("the question is up");
    assert_eq!(confirm.title(), "Connect Steven to your account?");
    assert_eq!(c.take_request(), None);

    // Escape is Cancel, and Cancel writes nothing.
    assert!(c.settle_attach_prompt(ConfirmOutcome::Cancel));
    assert!(c.attach_prompt.is_none());
    assert_eq!(c.take_request(), None);
    assert!(c.pending_attach.is_none());

    c.perform(Intent::Attach(local));
    assert!(c.settle_attach_prompt(ConfirmOutcome::Confirm));
    let (command, submission) = c.take_request_with_submission().expect("sent");
    assert_eq!(
        command,
        AccountCommand::AttachProfile {
            profile_id: local.to_storage(),
            name_snapshot: Some("Steven".into()),
            locale_snapshot: Some(crate::format::lang().tag().to_owned()),
        }
    );
    assert!(submission.is_some());
    // Withheld while in flight, and the pane says so instead.
    assert_eq!(c.account_link_control(), None);
    assert!(c.attach_status().is_some());
}

#[test]
fn the_hosted_answer_binds_locally_and_connected_waits_for_the_bind() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);
    c.perform(Intent::Attach(local));
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    c.take_request();
    assert_eq!(c.take_bind_request(), None);

    c.complete(AccountOperationResult::Profile {
        profile: hosted_row(local),
    });
    // Unbound before, and the identity comes from the result, not a guess.
    assert_eq!(
        c.take_bind_request(),
        Some((local, None, IDENTITY.to_owned()))
    );
    assert!(c.pending_attach.is_some());

    assert!(c.local_profile_account_bound(local));
    assert!(c.pending_attach.is_none());
    assert_eq!(
        c.notice.as_deref(),
        Some("Steven is connected to your account.")
    );
    assert_eq!(c.take_request(), Some(AccountCommand::ListProfiles));
}

#[test]
fn a_result_for_another_profile_is_not_this_attach() {
    let local = ProfileId::new();
    let other = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);
    c.perform(Intent::Attach(local));
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    c.take_request();

    c.complete(AccountOperationResult::Profile {
        profile: hosted_row(other),
    });
    assert_eq!(c.take_bind_request(), None);
    assert!(c.pending_attach.is_some());
}

#[test]
fn a_refused_command_or_a_hosted_failure_gives_the_button_back() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);

    c.perform(Intent::Attach(local));
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    let (_, submission) = c.take_request_with_submission().unwrap();
    assert!(c.account_command_failed(CommandFailure {
        submission: submission.unwrap(),
        uncertain: true,
    }));
    assert!(c.pending_attach.is_none());
    assert!(offers(&c, LinkControl::Attach(local)));
    assert_eq!(
        c.notice.as_deref(),
        Some("Koru couldn’t connect Steven to your account. It remains available offline.")
    );

    c.perform(Intent::Attach(local));
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    c.take_request();
    c.complete(AccountOperationResult::Failed {
        code: "profile_conflict".into(),
    });
    assert!(c.pending_attach.is_none());
    assert!(offers(&c, LinkControl::Attach(local)));
}

#[test]
fn a_local_bind_failure_leaves_the_profile_local_only_and_retryable() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);
    c.perform(Intent::Attach(local));
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    c.take_request();
    c.complete(AccountOperationResult::Profile {
        profile: hosted_row(local),
    });
    c.take_bind_request();

    assert!(c.local_profile_account_bind_failed(local, "profile_account_conflict"));
    assert!(c.pending_attach.is_none());
    assert!(offers(&c, LinkControl::Attach(local)));
    assert_eq!(c.take_request(), None);
}

#[test]
fn a_second_question_is_not_raised_while_one_is_up_or_in_flight() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);
    c.perform(Intent::Attach(local));
    c.perform(Intent::Attach(local));
    assert!(c.attach_prompt.is_some());
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    c.perform(Intent::Attach(local));
    assert!(c.attach_prompt.is_none());
}

#[test]
fn a_stale_sign_in_is_refreshed_and_the_same_attach_goes_out_again() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);
    c.perform(Intent::Attach(local));
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    let (first, first_submission) = c.take_request_with_submission().unwrap();

    // The hosted side wants the person, not a session restored from the vault.
    c.complete(AccountOperationResult::Failed {
        code: "fresh_authentication_required".into(),
    });
    assert!(matches!(
        c.take_request(),
        Some(AccountCommand::StartGoogle { .. })
    ));
    // Still in flight: no button, and the pane says what it is waiting for.
    assert_eq!(c.account_link_control(), None);
    assert_eq!(
        c.attach_status().as_deref(),
        Some("Sign in with Google again to connect Steven.")
    );

    // The sign-in lands and the very same attach goes out under a new submission.
    c.complete(AccountOperationResult::SignedIn);
    let (again, again_submission) = c.take_request_with_submission().unwrap();
    assert_eq!(again, first);
    assert_ne!(again_submission, first_submission);
    assert_eq!(c.attach_status().as_deref(), Some("Connecting…"));

    c.complete(AccountOperationResult::Profile {
        profile: hosted_row(local),
    });
    assert_eq!(
        c.take_bind_request(),
        Some((local, None, IDENTITY.to_owned()))
    );
}

#[test]
fn cancelling_the_fresh_sign_in_drops_the_attach_and_gives_the_button_back() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);
    c.perform(Intent::Attach(local));
    c.settle_attach_prompt(ConfirmOutcome::Confirm);
    c.take_request();
    c.complete(AccountOperationResult::Failed {
        code: "fresh_authentication_required".into(),
    });
    c.take_request();

    c.complete(AccountOperationResult::Cancelled);
    assert!(c.pending_attach.is_none());
    assert_eq!(c.take_request(), None);
    assert!(offers(&c, LinkControl::Attach(local)));
}

#[test]
fn a_sign_in_nobody_was_waiting_for_does_not_send_an_attach() {
    let local = ProfileId::new();
    let mut c = signed_in_with(vec![local_only(local, "Steven")], local);
    c.complete(AccountOperationResult::SignedIn);
    assert!(matches!(
        c.take_request(),
        Some(AccountCommand::InspectAccountSession)
    ));
    assert!(c.pending_attach.is_none());
}
