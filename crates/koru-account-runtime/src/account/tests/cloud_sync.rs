// SPDX-License-Identifier: MPL-2.0

use super::*;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::account::cloud_sync::{
    CLOUD_SYNC_UNAVAILABLE, CloudSyncPort, DeviceCloudSync, STATE_VERSION,
};
use koru_account_core::WorkspaceId;
use koru_account_wire::{
    BeginWorkspaceCloudActivationRequestV1, CloudWorkspaceCandidateKindV1,
    CloudWorkspaceCandidateV1, CloudWorkspacePageV1, CompleteWorkspaceCloudActivationRequestV1,
    MutateWorkspaceCloudRequestV1, WorkspaceCloudActivationChallengeV1,
    WorkspaceCloudLifecycleResponseV1,
};
use koru_agent_wire::CloudSyncProfile;
use serde::Serialize;

/// The incarnation a Workspace got when some *other* device activated it.
const ELSEWHERE_INCARNATION: u128 = 0xe15e;

// ----- the host port, in memory -----------------------------------------

#[derive(Default)]
struct FakePort {
    workspaces: Mutex<Vec<(ProfileId, WorkspaceId)>>,
    state: Mutex<DeviceCloudSync>,
    stores: AtomicUsize,
}

impl FakePort {
    fn with(workspaces: Vec<(ProfileId, WorkspaceId)>) -> Arc<Self> {
        Arc::new(Self {
            workspaces: Mutex::new(workspaces),
            state: Mutex::new(DeviceCloudSync::default()),
            stores: AtomicUsize::new(0),
        })
    }
    fn saved(&self) -> DeviceCloudSync {
        self.state.lock().unwrap().clone()
    }
    /// A Workspace created after the switch was already answered.
    fn created(&self, profile: ProfileId, workspace: WorkspaceId) {
        self.workspaces.lock().unwrap().push((profile, workspace));
    }
}

impl CloudSyncPort for FakePort {
    fn workspaces(&self) -> Result<Vec<(ProfileId, WorkspaceId)>, ServiceError> {
        Ok(self.workspaces.lock().unwrap().clone())
    }
    fn load(&self) -> Result<DeviceCloudSync, ServiceError> {
        Ok(self.state.lock().unwrap().clone())
    }
    fn store(&self, state: &DeviceCloudSync) -> Result<(), ServiceError> {
        self.stores.fetch_add(1, Ordering::AcqRel);
        *self.state.lock().unwrap() = state.clone();
        Ok(())
    }
}

// ----- the hosted account, in memory ------------------------------------

/// A hosted Workspace Cloud row, obeying the parts of
/// `workspace-cloud-lifecycle-v1` this handler depends on: a version that only
/// moves forward, an incarnation minted by the first activation, and a refusal
/// when the caller's expected version is not the current one.
#[derive(Clone, Copy)]
struct Row {
    enabled: bool,
    version: u64,
    incarnation: uuid::Uuid,
}

#[derive(Default)]
struct HostedAccount {
    profile: Option<ProfileId>,
    rows: Mutex<BTreeMap<WorkspaceId, Row>>,
    /// Activation is refused outright for these, with a code a person's other
    /// Workspaces must survive.
    unauthorized: BTreeSet<WorkspaceId>,
    challenges: Mutex<BTreeMap<uuid::Uuid, WorkspaceId>>,
    paths: Mutex<Vec<String>>,
    minted: AtomicUsize,
}

impl HostedAccount {
    fn calls(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
    /// Hosted mutations: begin, complete and disable. Reads are not these.
    fn lifecycle_calls(&self) -> usize {
        self.calls()
            .iter()
            .filter(|path| {
                path.ends_with("/cloud/activation")
                    || path.ends_with("/cloud/activation/complete")
                    || path.ends_with("/cloud/disable")
            })
            .count()
    }
    /// Reads of one Workspace's lifecycle row.
    fn reads(&self) -> usize {
        self.calls()
            .iter()
            .filter(|path| {
                path.starts_with("/v1/account/workspaces/") && path.ends_with("/cloud/state")
            })
            .count()
    }
    /// Another device turned this Workspace off: the row stays, its version
    /// moved, and the account's enabled list no longer names it.
    fn disabled_elsewhere(&self, workspace: WorkspaceId) {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.get_mut(&workspace).expect("a row to disable");
        row.enabled = false;
        row.version += 1;
    }
    fn row(&self, workspace: WorkspaceId) -> Option<Row> {
        self.rows.lock().unwrap().get(&workspace).copied()
    }
    /// Another device turned this Workspace on, so the local device's guess of
    /// version 0 is stale and the account already lists it as enabled.
    fn enabled_elsewhere(&self, workspace: WorkspaceId) {
        self.rows.lock().unwrap().insert(
            workspace,
            Row {
                enabled: true,
                version: 4,
                incarnation: uuid::Uuid::from_u128(ELSEWHERE_INCARNATION),
            },
        );
    }
    fn mint(&self) -> uuid::Uuid {
        uuid::Uuid::from_u128(1 + self.minted.fetch_add(1, Ordering::AcqRel) as u128)
    }
    fn workspace_of(path: &str) -> WorkspaceId {
        let id = path
            .trim_start_matches("/v1/account/workspaces/")
            .split('/')
            .next()
            .unwrap();
        WorkspaceId::parse(id).unwrap()
    }
    fn json<T: Serialize>(value: &T) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: 200,
            body: serde_json::to_vec(value).unwrap(),
        })
    }
    fn refuse(status: u16, code: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status,
            body: serde_json::to_vec(&serde_json::json!({"code": code, "message": "refused"}))
                .unwrap(),
        })
    }
    fn answer(&self, workspace: WorkspaceId, row: Row) -> Result<HttpResponse, TransportError> {
        Self::json(&WorkspaceCloudLifecycleResponseV1 {
            version: Version1,
            status: koru_account_core::workspace_lifecycle::WorkspaceLifecycleStatus::Committed,
            workspace_id: workspace,
            state: if row.enabled {
                koru_account_core::workspace_lifecycle::WorkspaceCloudState::Enabled
            } else {
                koru_account_core::workspace_lifecycle::WorkspaceCloudState::Disabled
            },
            incarnation_id: row.incarnation,
            membership_epoch: row.version,
            cloud_access_epoch: row.version,
            authorization_version: row.version,
        })
    }
}

impl Transport for HostedAccount {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let path = request.path_and_query.clone();
        self.paths.lock().unwrap().push(path.clone());
        if path == "/v1/account/cloud/workspaces" {
            let rows = self.rows.lock().unwrap();
            return Self::json(&CloudWorkspacePageV1 {
                version: Version1,
                workspaces: rows
                    .iter()
                    .filter(|(_, row)| row.enabled)
                    .map(|(workspace, row)| CloudWorkspaceCandidateV1 {
                        kind: CloudWorkspaceCandidateKindV1::Cloud,
                        profile_id: self.profile.unwrap(),
                        workspace_id: *workspace,
                        workspace_incarnation_id: row.incarnation,
                        authorization_version: row.version as i64,
                        membership_epoch: row.version as i64,
                        cloud_access_epoch: row.version as i64,
                    })
                    .collect(),
                next: None,
            });
        }
        if path.starts_with("/v1/account/workspaces/") && path.ends_with("/cloud/state") {
            use koru_account_core::workspace_lifecycle::{
                WorkspaceCloudState, WorkspaceLifecycleStatus,
            };
            let workspace = Self::workspace_of(&path);
            return match self.row(workspace) {
                Some(row) => Self::json(&WorkspaceCloudLifecycleResponseV1 {
                    version: Version1,
                    status: if row.enabled {
                        WorkspaceLifecycleStatus::AlreadyEnabled
                    } else {
                        WorkspaceLifecycleStatus::AlreadyDisabled
                    },
                    workspace_id: workspace,
                    state: if row.enabled {
                        WorkspaceCloudState::Enabled
                    } else {
                        WorkspaceCloudState::Disabled
                    },
                    incarnation_id: row.incarnation,
                    membership_epoch: row.version,
                    cloud_access_epoch: row.version,
                    authorization_version: row.version,
                }),
                None => Self::json(&WorkspaceCloudLifecycleResponseV1 {
                    version: Version1,
                    status: WorkspaceLifecycleStatus::AlreadyAbsent,
                    workspace_id: workspace,
                    state: WorkspaceCloudState::Absent,
                    incarnation_id: uuid::Uuid::nil(),
                    membership_epoch: 0,
                    cloud_access_epoch: 0,
                    authorization_version: 0,
                }),
            };
        }
        if path.ends_with("/cloud/activation") {
            let workspace = Self::workspace_of(&path);
            let body: BeginWorkspaceCloudActivationRequestV1 =
                serde_json::from_slice(&request.body).unwrap();
            if self.unauthorized.contains(&workspace) {
                return Self::refuse(403, "workspace_not_authorized");
            }
            let current = self.row(workspace).map_or(0, |row| row.version);
            if body.expected_authorization_version != current {
                return Self::refuse(409, "workspace_version_mismatch");
            }
            let challenge_id = self.mint();
            self.challenges
                .lock()
                .unwrap()
                .insert(challenge_id, workspace);
            return Self::json(&WorkspaceCloudActivationChallengeV1 {
                version: Version1,
                challenge_id,
                workspace_id: workspace,
                signing_bytes: workspace.as_uuid().as_bytes().to_vec(),
                expires_at_millis: 9_000_000,
            });
        }
        if path.ends_with("/cloud/activation/complete") {
            let body: CompleteWorkspaceCloudActivationRequestV1 =
                serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body.signature.len(), 64, "the device signature is ed25519");
            let workspace = self.challenges.lock().unwrap()[&body.challenge_id];
            let mut rows = self.rows.lock().unwrap();
            let previous = rows.get(&workspace).copied();
            let row = Row {
                enabled: true,
                version: previous.map_or(1, |row| row.version + 1),
                incarnation: previous.map_or_else(|| self.mint(), |row| row.incarnation),
            };
            rows.insert(workspace, row);
            drop(rows);
            return self.answer(workspace, row);
        }
        if path.ends_with("/cloud/disable") {
            let workspace = Self::workspace_of(&path);
            let body: MutateWorkspaceCloudRequestV1 =
                serde_json::from_slice(&request.body).unwrap();
            let mut rows = self.rows.lock().unwrap();
            let Some(previous) = rows.get(&workspace).copied() else {
                return Self::refuse(409, "workspace_version_mismatch");
            };
            if body.expected_authorization_version != previous.version
                || body.expected_incarnation_id != previous.incarnation
            {
                return Self::refuse(409, "workspace_version_mismatch");
            }
            let row = Row {
                enabled: false,
                version: previous.version + 1,
                ..previous
            };
            rows.insert(workspace, row);
            drop(rows);
            return self.answer(workspace, row);
        }
        panic!("unexpected Cloud sync route {path}");
    }
}

// ----- fixture ----------------------------------------------------------

struct Fixture {
    service: ProductionAccountService,
    hosted: Arc<HostedAccount>,
    port: Arc<FakePort>,
    profile: ProfileId,
}

/// A restored session. Only its presence matters here — the Cloud sync
/// commands read the access token, never the vault secret.
fn restored() -> SessionCredential {
    SessionCredential::new(
        "20000000-0000-4000-8000-000000000002".into(),
        "refresh-vault-only".into(),
    )
    .unwrap()
}

fn fixture(hosted: HostedAccount, workspaces: Vec<(ProfileId, WorkspaceId)>) -> Fixture {
    let profile = hosted.profile.expect("the fixture account owns a Profile");
    let account = KoruIdentityId::mint();
    let hosted = Arc::new(hosted);
    let port = FakePort::with(workspaces);
    let inventory = koru_store_wire::ProfileLifecycleInventory {
        profiles: vec![koru_store_wire::ProfileLifecycleInventoryItem {
            profile_id: profile,
            account_binding: format!("koru:{account}"),
            removed_at_millis: None,
        }],
    };
    let mut service = ProductionAccountService::with_client(
        AccountClient::with_transport(hosted.clone()),
        signed_descriptor(),
        "test".into(),
    )
    .with_local_profile_inventory(Arc::new(StaticInventory(inventory)))
    .with_cloud_sync_port(port.clone());
    service.access = Some(AccessToken("memory-only".into()));
    service.access_expires_at_millis = Some(2_000_000_000_000);
    service.durable_owner = Some(crate::identity::AccountOwner {
        account,
        auth_identity: "30000000-0000-4000-8000-000000000001".parse().unwrap(),
    });
    Fixture {
        service,
        hosted,
        port,
        profile,
    }
}

impl Fixture {
    /// `Err` is the stable code of a `Failed` answer: a refusal of the switch
    /// is answered as one, never published as a change of account stage.
    fn run(&mut self, command: AccountCommand) -> Result<Vec<CloudSyncProfile>, String> {
        match self.service.execute(
            7,
            command,
            Some(&restored()),
            &Cancel(Arc::new(AtomicBool::new(false))),
            &mut |_| {},
        ) {
            Ok(outcome) => {
                assert_eq!(outcome.state.stage, AccountStage::SignedIn);
                match outcome.result {
                    AccountOperationResult::CloudSync { profiles, .. } => Ok(profiles),
                    AccountOperationResult::Failed { code } => Err(code),
                    other => panic!("expected CloudSync or Failed, got {other:?}"),
                }
            }
            Err(error) => {
                panic!("a Cloud sync command must answer, not fail the stage: {error:?}")
            }
        }
    }
    fn set(&mut self, enabled: bool) -> Result<Vec<CloudSyncProfile>, String> {
        let profile_id = self.profile.to_storage();
        self.run(AccountCommand::SetProfileCloudSync {
            request_id: 11,
            profile_id,
            enabled,
        })
    }
    fn inspect(&mut self) -> Result<Vec<CloudSyncProfile>, String> {
        self.run(AccountCommand::InspectCloudSync { request_id: 12 })
    }
}

fn workspaces(profile: ProfileId, count: usize) -> Vec<(ProfileId, WorkspaceId)> {
    (0..count)
        .map(|index| {
            (
                profile,
                WorkspaceId::from_uuid(uuid::Uuid::from_u128(0xf000 + index as u128)),
            )
        })
        .collect()
}

// ----- the tests --------------------------------------------------------

#[test]
fn local_cloud_choices_are_account_scoped_before_session_refresh() {
    let profile = ProfileId::new();
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            ..Default::default()
        },
        Vec::new(),
    );
    let account = fixture.service.durable_owner.unwrap().account;
    fixture
        .port
        .state
        .lock()
        .unwrap()
        .accounts
        .entry(account.to_string())
        .or_default()
        .profiles
        .entry(profile)
        .or_default()
        .enabled = true;
    fixture.service.access = None;
    fixture.service.durable_owner = None;
    let choices = fixture.service.cloud_profiles(account);
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].profile_id, profile.to_string());
    assert!(
        fixture
            .service
            .cloud_profiles(KoruIdentityId::mint())
            .is_empty()
    );
    assert!(
        fixture.hosted.calls().is_empty(),
        "local policy must precede HTTP and credentials"
    );
}

#[test]
fn a_host_without_the_port_refuses_rather_than_reporting_an_unapplied_choice() {
    let mut service = ProductionAccountService::with_client(
        AccountClient::with_transport(Arc::new(HostedAccount::default())),
        signed_descriptor(),
        "test".into(),
    );
    service.access = Some(AccessToken("memory-only".into()));
    service.access_expires_at_millis = Some(2_000_000_000_000);
    for command in [
        AccountCommand::InspectCloudSync { request_id: 1 },
        AccountCommand::SetProfileCloudSync {
            request_id: 2,
            profile_id: ProfileId::new().to_storage(),
            enabled: true,
        },
    ] {
        let outcome = service
            .execute(
                1,
                command.clone(),
                Some(&restored()),
                &Cancel(Arc::new(AtomicBool::new(false))),
                &mut |_| {},
            )
            .expect("a refusal of the switch is an answer, not a stage change");
        // Nothing is known, so the answer is a visible failure rather than an
        // empty Profile list that would read as "every Profile is off" - and
        // the person stays signed in.
        assert_eq!(outcome.state.stage, AccountStage::SignedIn);
        assert_eq!(
            outcome.result,
            AccountOperationResult::Failed {
                code: CLOUD_SYNC_UNAVAILABLE.into()
            }
        );
        assert!(matches!(
            command_failure(&command, CLOUD_SYNC_UNAVAILABLE),
            AccountOperationResult::Failed { .. }
        ));
    }
}

#[test]
fn turning_a_profile_on_then_off_carries_every_workspace_with_it() {
    let profile = ProfileId::new();
    let local = workspaces(profile, 3);
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            ..Default::default()
        },
        local.clone(),
    );

    let rows = fixture.set(true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].enabled,
            rows[0].workspaces,
            rows[0].synced_workspaces
        ),
        (true, 3, 3)
    );
    assert_eq!(rows[0].error, None);
    for (_, workspace) in &local {
        assert!(fixture.hosted.row(*workspace).unwrap().enabled);
    }
    let saved = fixture.port.saved();
    assert_eq!(saved.version, STATE_VERSION);
    let remembered = &saved.accounts.values().next().unwrap().profiles[&profile];
    assert!(remembered.enabled);
    assert_eq!(remembered.workspaces.len(), 3);
    assert!(
        remembered
            .workspaces
            .values()
            .all(|entry| entry.enabled && entry.incarnation_id.is_some() && entry.error.is_none())
    );

    let rows = fixture.set(false).unwrap();
    assert_eq!(
        (
            rows[0].enabled,
            rows[0].synced_workspaces,
            rows[0].error.clone()
        ),
        (false, 0, None)
    );
    for (_, workspace) in &local {
        let row = fixture.hosted.row(*workspace).unwrap();
        assert!(!row.enabled);
        // Disable bumps the version, and the device records the new one.
        assert_eq!(row.version, 2);
    }
    let saved = fixture.port.saved();
    let remembered = &saved.accounts.values().next().unwrap().profiles[&profile];
    assert!(!remembered.enabled);
    assert!(
        remembered
            .workspaces
            .values()
            .all(|entry| !entry.enabled && entry.authorization_version == 2)
    );
}

#[test]
fn a_version_mismatch_is_resolved_from_the_accounts_own_cloud_list() {
    let profile = ProfileId::new();
    let local = workspaces(profile, 2);
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            ..Default::default()
        },
        local.clone(),
    );
    // The second Workspace was turned on by another device, so this one's
    // expected version of 0 is stale.
    fixture.hosted.enabled_elsewhere(local[1].1);

    let rows = fixture.set(true).unwrap();
    assert_eq!(
        (rows[0].synced_workspaces, rows[0].error.clone()),
        (2, None)
    );
    let saved = fixture.port.saved();
    let remembered = &saved.accounts.values().next().unwrap().profiles[&profile];
    let learned = &remembered.workspaces[&local[1].1];
    assert!(learned.enabled);
    // The version and incarnation came from the list, not from an activation
    // this device performed — so a later disable can quote them.
    assert_eq!(learned.authorization_version, 4);
    assert_eq!(
        learned.incarnation_id,
        Some(uuid::Uuid::from_u128(ELSEWHERE_INCARNATION))
    );
    assert!(
        fixture
            .hosted
            .calls()
            .contains(&"/v1/account/cloud/workspaces".to_string())
    );

    // And turning it off disables both, including the one this device never
    // activated itself.
    let rows = fixture.set(false).unwrap();
    assert_eq!(
        (rows[0].synced_workspaces, rows[0].error.clone()),
        (0, None)
    );
    assert!(
        local
            .iter()
            .all(|(_, workspace)| { !fixture.hosted.row(*workspace).unwrap().enabled })
    );
}

#[test]
fn one_refused_workspace_does_not_stop_the_others() {
    let profile = ProfileId::new();
    let local = workspaces(profile, 3);
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            unauthorized: BTreeSet::from([local[1].1]),
            ..Default::default()
        },
        local.clone(),
    );

    let rows = fixture.set(true).unwrap();
    assert_eq!(
        (rows[0].workspaces, rows[0].synced_workspaces),
        (3, 2),
        "the refusal is one Workspace's, not the Profile's"
    );
    assert_eq!(rows[0].error.as_deref(), Some("workspace_not_authorized"));
    assert!(fixture.hosted.row(local[0].1).unwrap().enabled);
    assert!(fixture.hosted.row(local[2].1).unwrap().enabled);
    assert!(fixture.hosted.row(local[1].1).is_none());
    // The choice survives the refusal: it is what the reconcile pass replays.
    let saved = fixture.port.saved();
    let remembered = &saved.accounts.values().next().unwrap().profiles[&profile];
    assert!(remembered.enabled);
    assert_eq!(
        remembered.workspaces[&local[1].1].error.as_deref(),
        Some("workspace_not_authorized")
    );
}

#[test]
fn inspect_activates_a_workspace_created_after_the_switch_and_is_free_otherwise() {
    let profile = ProfileId::new();
    let local = workspaces(profile, 1);
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            ..Default::default()
        },
        local.clone(),
    );
    assert_eq!(fixture.set(true).unwrap()[0].synced_workspaces, 1);

    // Nothing changed: the reconcile pass reads the account's enabled list
    // (that is how another device's switch is seen), mutates nothing, reads
    // no single row and rewrites no remembered state.
    let before = fixture.hosted.lifecycle_calls();
    let reads = fixture.hosted.reads();
    let writes = fixture.port.stores.load(Ordering::Acquire);
    let rows = fixture.inspect().unwrap();
    assert_eq!((rows[0].enabled, rows[0].synced_workspaces), (true, 1));
    assert_eq!(fixture.hosted.lifecycle_calls(), before);
    assert_eq!(fixture.hosted.reads(), reads);
    assert_eq!(fixture.port.stores.load(Ordering::Acquire), writes);

    // A Workspace created afterwards reaches the Cloud with nobody touching
    // the switch again.
    let fresh = WorkspaceId::new();
    fixture.port.created(profile, fresh);
    let rows = fixture.inspect().unwrap();
    assert_eq!((rows[0].workspaces, rows[0].synced_workspaces), (2, 2));
    assert!(fixture.hosted.row(fresh).unwrap().enabled);
}

#[test]
fn a_profile_outside_the_signed_in_account_is_refused_before_any_hosted_call() {
    let profile = ProfileId::new();
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            ..Default::default()
        },
        workspaces(profile, 1),
    );
    let code = fixture
        .run(AccountCommand::SetProfileCloudSync {
            request_id: 3,
            profile_id: ProfileId::new().to_storage(),
            enabled: true,
        })
        .unwrap_err();
    assert_eq!(code, "profile_not_authorized");
    assert!(fixture.hosted.calls().is_empty());
    assert_eq!(fixture.port.stores.load(Ordering::Acquire), 0);
}

#[test]
fn a_state_file_from_an_unknown_version_is_discarded_rather_than_half_read() {
    let state = DeviceCloudSync {
        version: STATE_VERSION + 7,
        ..Default::default()
    };
    assert_eq!(state.accepted(), DeviceCloudSync::default());
    let current = DeviceCloudSync::default();
    assert_eq!(current.clone().accepted(), current);
}

#[test]
fn the_reconcile_pass_mirrors_what_another_device_turned_on_and_never_turns_it_off() {
    let profile = ProfileId::new();
    let local = workspaces(profile, 1);
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            ..Default::default()
        },
        local.clone(),
    );
    // This device has no remembered choice; another device switched the
    // Profile on. Before the fix, "no choice" read as "off" and the timer
    // disabled the other device's work within one heartbeat.
    fixture.hosted.enabled_elsewhere(local[0].1);

    let rows = fixture.inspect().unwrap();
    assert_eq!(
        (
            rows[0].enabled,
            rows[0].synced_workspaces,
            rows[0].error.clone()
        ),
        (true, 1, None),
        "the switch shows the hosted truth"
    );
    assert!(fixture.hosted.row(local[0].1).unwrap().enabled);
    assert_eq!(
        fixture.hosted.lifecycle_calls(),
        0,
        "the reconcile pass mutates nothing"
    );
    let saved = fixture.port.saved();
    let remembered = &saved.accounts.values().next().unwrap().profiles[&profile];
    assert!(remembered.enabled);
    assert_eq!(remembered.workspaces[&local[0].1].authorization_version, 4);
}

#[test]
fn a_workspace_another_device_turned_off_shows_off_and_can_be_turned_on_again() {
    let profile = ProfileId::new();
    let local = workspaces(profile, 1);
    let mut fixture = fixture(
        HostedAccount {
            profile: Some(profile),
            ..Default::default()
        },
        local.clone(),
    );
    assert_eq!(fixture.set(true).unwrap()[0].synced_workspaces, 1);
    fixture.hosted.disabled_elsewhere(local[0].1);

    // The reconcile pass notices (the list no longer names the Workspace, so
    // its row is read), mirrors "off", and does not fight the other device by
    // re-activating.
    let mutations = fixture.hosted.lifecycle_calls();
    let reads = fixture.hosted.reads();
    let rows = fixture.inspect().unwrap();
    assert_eq!(
        (
            rows[0].enabled,
            rows[0].synced_workspaces,
            rows[0].error.clone()
        ),
        (false, 0, None)
    );
    assert_eq!(fixture.hosted.lifecycle_calls(), mutations);
    assert_eq!(fixture.hosted.reads(), reads + 1);
    assert!(!fixture.hosted.row(local[0].1).unwrap().enabled);

    // A person switching it on here quotes the version the row now holds,
    // which only the read could have told this device.
    let rows = fixture.set(true).unwrap();
    assert_eq!(
        (
            rows[0].enabled,
            rows[0].synced_workspaces,
            rows[0].error.clone()
        ),
        (true, 1, None)
    );
    let row = fixture.hosted.row(local[0].1).unwrap();
    assert!(row.enabled);
    assert_eq!(row.version, 3);
}
