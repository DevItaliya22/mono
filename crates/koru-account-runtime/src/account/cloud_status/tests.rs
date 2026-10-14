// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_agent_wire::AccountProfileCloud;

#[test]
fn cloud_progress_is_account_scoped_and_never_reports_coverage() {
    let account = "10000000-0000-4000-8000-000000000001".parse().unwrap();
    let mut state = AccountState {
        active_scope: 7,
        active_account_id: Some(format!("{account}")),
        cloud_profiles: vec![AccountProfileCloud {
            profile_id: "20000000-0000-4000-8000-000000000001".into(),
            stage: CloudConnectionStage::Connecting,
        }],
        ..Default::default()
    };
    let mut snapshot = Snapshot {
        owner_scope: Some(8),
        account_id: Some(account),
        failure_code: Some("cloud_unavailable"),
        ..Default::default()
    };
    project(&mut state, &snapshot);
    assert_eq!(
        state.cloud_profiles[0].stage,
        CloudConnectionStage::Connecting
    );
    snapshot.owner_scope = Some(7);
    project(&mut state, &snapshot);
    assert_eq!(
        state.cloud_profiles[0].stage,
        CloudConnectionStage::Retrying
    );
    snapshot.failure_code = None;
    snapshot.stage = Stage::Settled;
    project(&mut state, &snapshot);
    // A completed account pass without this Profile is not download completion.
    assert_eq!(
        state.cloud_profiles[0].stage,
        CloudConnectionStage::Connecting
    );
}

#[test]
fn one_profiles_refusal_does_not_replace_another_profiles_progress() {
    use crate::cloud::StatusRecord;
    use koru_device::cloud::{CloudId, CloudReplicaPrincipalV1};
    let account = "10000000-0000-4000-8000-000000000001".parse().unwrap();
    let profile =
        koru_account_core::ProfileId::parse("20000000-0000-4000-8000-000000000001").unwrap();
    let other =
        koru_account_core::ProfileId::parse("20000000-0000-4000-8000-000000000002").unwrap();
    let mut state = AccountState {
        active_scope: 7,
        active_account_id: Some(format!("{account}")),
        cloud_profiles: [profile, other]
            .into_iter()
            .map(|id| AccountProfileCloud {
                profile_id: id.to_string(),
                stage: CloudConnectionStage::Connecting,
            })
            .collect(),
        ..Default::default()
    };
    let snapshot = Snapshot {
        owner_scope: Some(7),
        account_id: Some(account),
        failure_code: Some("cloud_unavailable"),
        records: [profile, other]
            .into_iter()
            .map(|id| StatusRecord {
                permit_id: uuid::Uuid::nil(),
                profile_id: id,
                account_id: account,
                account_generation: 1,
                dataset: CloudReplicaPrincipalV1::Profile {
                    profile_id: CloudId::from_bytes([1; 16]),
                    lineage_id: CloudId::from_bytes([2; 16]),
                    logical_replica_id: CloudId::from_bytes([3; 16]),
                },
                stage: if id == profile {
                    Stage::Settled
                } else {
                    Stage::Unavailable
                },
                error_code: (id == other).then_some("cloud_unavailable"),
                pending_consent_ids: Vec::new(),
                bytes_held: 0,
                queued_jobs: 0,
            })
            .collect(),
        ..Default::default()
    };
    project(&mut state, &snapshot);
    assert_eq!(state.cloud_profiles[0].stage, CloudConnectionStage::Syncing);
    assert_eq!(
        state.cloud_profiles[1].stage,
        CloudConnectionStage::Retrying
    );
}
