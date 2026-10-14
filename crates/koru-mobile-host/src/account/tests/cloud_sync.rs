// SPDX-License-Identifier: MPL-2.0

//! The phone edge of the per-Profile Cloud sync switch: what the app's two
//! commands become, and what the handler's answer looks like coming back.
//!
//! The switch itself is `koru-account-runtime`'s, and its rules are tested
//! there against a fake host. What is only true here is the translation: the
//! generated request has no `request_id`, the phone correlates by
//! requester/generation/sequence instead, and the answer has to arrive as a
//! content-free projection rather than the runtime's own type.

use super::*;

use koru_agent_wire::CloudSyncProfile;

/// Records the command it was handed and answers it with a fixed report, so
/// the test asserts the edge rather than the handler.
struct RecordingService(Arc<std::sync::Mutex<Vec<AccountCommand>>>);

impl AccountService for RecordingService {
    fn execute(
        &mut self,
        _: u64,
        command: AccountCommand,
        _: Option<&SessionCredential>,
        _: &Cancel,
        _: &mut dyn FnMut(ServiceProgress),
    ) -> Result<ServiceOutcome, ServiceError> {
        let cloud_sync = matches!(
            command,
            AccountCommand::SetProfileCloudSync { .. } | AccountCommand::InspectCloudSync { .. }
        );
        self.0.lock().unwrap().push(command);
        Ok(ServiceOutcome {
            state: AccountState {
                capability: true,
                stage: AccountStage::SignedIn,
                ..Default::default()
            },
            credential: CredentialChange::Keep,
            result: if cloud_sync {
                AccountOperationResult::CloudSync {
                    request_id: 7,
                    profiles: vec![
                        CloudSyncProfile {
                            profile_id: "22222222-2222-4222-8222-222222222222".into(),
                            enabled: true,
                            workspaces: 5,
                            synced_workspaces: 2,
                            error: Some("workspace_version_mismatch".into()),
                        },
                        CloudSyncProfile {
                            profile_id: "33333333-3333-4333-8333-333333333333".into(),
                            enabled: false,
                            workspaces: 1,
                            synced_workspaces: 0,
                            error: None,
                        },
                    ],
                }
            } else {
                AccountOperationResult::SignedIn
            },
            follow_up: None,
        })
    }
}

const PROFILE: &str = "22222222-2222-4222-8222-222222222222";

/// A born-backgrounded worker has not restored yet, and its owner scope moves
/// while it does; a command quoting the scope before then is refused with
/// `account_scope_changed`. Settle the restore first, exactly as the app does.
fn settled(account: &MobileAccount) -> u64 {
    account.control.set_foreground(true);
    let load = wait_for(|| account.poll(7, 0, true).vault);
    account
        .bridge
        .vault_reply(wire::AccountVaultReply {
            request_id: load.request_id,
            success: true,
            credential: None,
        })
        .unwrap();
    wait_for(|| (account.control.state().stage == AccountStage::SignedOut).then_some(()));
    account.control.state().active_scope
}

fn request(action: wire::AccountAction, scope: u64) -> wire::AccountCommandRequest {
    wire::AccountCommandRequest {
        expected_owner_scope: Some(scope),
        version: 1,
        requester: 7,
        action: action as i32,
        ..Default::default()
    }
}

#[test]
fn the_switch_reaches_the_service_as_the_command_the_desktop_already_sends() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let bridge = PlatformBridge::new("trusted-web.apps.googleusercontent.com".into()).unwrap();
    let account = MobileAccount::start(RecordingService(seen.clone()), bridge, entry());
    let scope = settled(&account);

    let mut set = request(wire::AccountAction::SetProfileCloudSync, scope);
    set.profile_id = PROFILE.into();
    set.cloud_sync_enabled = Some(true);
    account.command(&set).unwrap();
    wait_for(|| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|command| matches!(command, AccountCommand::SetProfileCloudSync { .. }))
            .then_some(())
    });

    let recorded = seen.lock().unwrap().clone();
    let switch = recorded
        .iter()
        .find_map(|command| match command {
            AccountCommand::SetProfileCloudSync {
                request_id,
                profile_id,
                enabled,
            } => Some((*request_id, profile_id.clone(), *enabled)),
            _ => None,
        })
        .expect("the set command reached the service");
    assert_eq!(switch.1, PROFILE);
    assert!(switch.2, "the switch's position is carried, not defaulted");
    // The generated request carries no request id; this edge correlates by
    // requester, so that is what the echoed field must be.
    assert_eq!(switch.0, 7);
}

#[test]
fn reading_every_profiles_state_is_its_own_command_with_no_target() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let bridge = PlatformBridge::new("trusted-web.apps.googleusercontent.com".into()).unwrap();
    let account = MobileAccount::start(RecordingService(seen.clone()), bridge, entry());
    let scope = settled(&account);

    let inspect = request(wire::AccountAction::InspectCloudSync, scope);
    account.command(&inspect).unwrap();
    wait_for(|| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|command| matches!(command, AccountCommand::InspectCloudSync { .. }))
            .then_some(())
    });
}

#[test]
fn the_answer_projects_ids_counts_and_a_stable_code_and_nothing_else() {
    let result = operation_result(
        3,
        7,
        11,
        AccountOperationResult::CloudSync {
            request_id: 7,
            profiles: vec![CloudSyncProfile {
                profile_id: PROFILE.into(),
                enabled: true,
                workspaces: 5,
                synced_workspaces: 2,
                error: Some("workspace_version_mismatch".into()),
            }],
        },
    );
    assert_eq!(result.kind, "cloud_sync");
    assert_eq!(result.cloud_sync.len(), 1);
    let row = &result.cloud_sync[0];
    assert_eq!(row.profile_id, PROFILE);
    assert!(row.enabled);
    assert_eq!((row.workspaces, row.synced_workspaces), (5, 2));
    assert_eq!(
        row.error_code.as_deref(),
        Some("workspace_version_mismatch")
    );
    // A partly applied choice must stay visible rather than being rounded to
    // "on": the counts are the only thing that says so.
    assert!(row.synced_workspaces < row.workspaces);
    // Nothing else on the result is filled in by this projection.
    assert!(result.profiles.is_empty() && result.enrollments.is_empty());
    assert!(result.error_code.is_none() && result.account.is_none());
}

#[test]
fn a_refused_switch_is_a_failure_result_the_app_can_show_verbatim() {
    let result = operation_result(
        4,
        7,
        12,
        AccountOperationResult::Failed {
            code: koru_account_runtime::account::cloud_sync::CLOUD_SYNC_UNAVAILABLE.into(),
        },
    );
    assert_eq!(result.kind, "failed");
    assert_eq!(result.error_code.as_deref(), Some("cloud_sync_unavailable"));
    assert!(result.cloud_sync.is_empty());
}
