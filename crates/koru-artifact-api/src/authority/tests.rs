// SPDX-License-Identifier: MPL-2.0

//! The authority verbs over the real repository: a token that resolves, a
//! token that is not a run token, an advance that makes the old one stale,
//! a revocation every token then reads, and the scope each verb takes.

use std::collections::BTreeSet;
use std::path::Path;

use koru_artifact_core::{AuthorityRefusal, AuthorityState, RunWorkspace, WorkspaceId};

use super::*;
use crate::tests::{both, context, service, v4};

fn run_workspace() -> RunWorkspace {
    let path = if cfg!(windows) {
        Path::new(r"C:\runs\one")
    } else {
        Path::new("/runs/one")
    };
    RunWorkspace::new(path).unwrap()
}

pub(crate) fn issue_request(run: u8, grants: &[WorkspaceId]) -> IssueAuthorityRequest {
    IssueAuthorityRequest {
        run: RunId::from_uuid(v4(run)),
        workspace: WorkspaceId::from_uuid(v4(0xa0)),
        generation: AttemptGeneration::FIRST,
        grants: grants.iter().copied().collect(),
        run_workspace: run_workspace(),
    }
}

fn authorized(resolution: Resolution) -> RunAuthorityRecord {
    match resolution {
        Resolution::Authorized(record) => record,
        Resolution::Refused(refusal) => panic!("refused: {refusal}"),
    }
}

fn refused(resolution: Resolution) -> AuthorityRefusal {
    match resolution {
        Resolution::Refused(refusal) => refusal,
        Resolution::Authorized(record) => panic!("authorized {:?}", record.run),
    }
}

#[test]
fn a_token_is_minted_once_hashed_for_storage_and_resolves_to_its_record() {
    let service = service();
    let grants = [WorkspaceId::from_uuid(v4(0xa0))];
    let issued = service.issue(both(), issue_request(1, &grants)).unwrap();
    assert!(issued.token.starts_with(TOKEN_PREFIX));
    assert_eq!(issued.token.len(), TOKEN_PREFIX.len() + 64);
    assert_eq!(
        issued.path,
        format!("/v1/runs/{}/mcp", RunId::from_uuid(v4(1)).to_storage())
    );
    assert!(
        !format!("{issued:?}").contains(&issued.token),
        "the bearer is shown once, on the wire"
    );
    let record = authorized(service.resolve(&issued.token).unwrap());
    assert_eq!(record.run, issued.run);
    assert_eq!(record.generation, AttemptGeneration::FIRST);
    assert_eq!(record.grants, BTreeSet::from(grants));
    assert_eq!(record.run_workspace, run_workspace());
    assert_eq!(record.state, AuthorityState::Active);
    // The record answers by run id too, without the token or the path.
    let view = service.authority(both(), issued.run).unwrap();
    assert_eq!(view.generation, AttemptGeneration::FIRST);
    assert_eq!(view.grants, BTreeSet::from(grants));
    let json = serde_json::to_value(&view).unwrap();
    assert!(json.get("token").is_none());
    assert!(json.get("run_workspace").is_none());
    // A second issue for the run is refused: advance instead.
    assert_eq!(
        service
            .issue(both(), issue_request(1, &[]))
            .unwrap_err()
            .code,
        "authority_exists"
    );
}

#[test]
fn a_bearer_that_is_not_a_run_token_is_unknown_and_so_is_a_forged_one() {
    let service = service();
    service.issue(both(), issue_request(1, &[])).unwrap();
    assert_eq!(
        refused(service.resolve("koru_api_0123456789abcdef").unwrap()),
        AuthorityRefusal::Unknown
    );
    assert_eq!(
        refused(
            service
                .resolve(&format!("{TOKEN_PREFIX}{}", "0".repeat(64)))
                .unwrap()
        ),
        AuthorityRefusal::Unknown
    );
    assert_eq!(
        refused(service.resolve("").unwrap()),
        AuthorityRefusal::Unknown
    );
}

#[test]
fn advancing_the_generation_retires_the_old_token_as_stale_and_keeps_the_grants() {
    let service = service();
    let grants = [WorkspaceId::from_uuid(v4(0xa0))];
    let first = service.issue(both(), issue_request(1, &grants)).unwrap();
    let second = service
        .advance_generation(both(), first.run, AttemptGeneration::FIRST.next())
        .unwrap();
    assert_ne!(first.token, second.token);
    assert_eq!(second.path, first.path, "the endpoint is stable per run");
    assert_eq!(second.generation, AttemptGeneration::FIRST.next());
    assert_eq!(
        refused(service.resolve(&first.token).unwrap()),
        AuthorityRefusal::StaleGeneration
    );
    let record = authorized(service.resolve(&second.token).unwrap());
    assert_eq!(record.generation, AttemptGeneration::FIRST.next());
    assert_eq!(record.grants, BTreeSet::from(grants), "never widened");
    // Not newer: the fence's own code.
    assert_eq!(
        service
            .advance_generation(both(), first.run, AttemptGeneration::FIRST)
            .unwrap_err()
            .code,
        "stale_generation"
    );
    assert_eq!(
        service
            .advance_generation(
                both(),
                RunId::from_uuid(v4(2)),
                AttemptGeneration::FIRST.next()
            )
            .unwrap_err()
            .code,
        "authority_not_found"
    );
}

#[test]
fn revocation_is_read_by_every_token_and_is_idempotent() {
    let service = service();
    let issued = service.issue(both(), issue_request(1, &[])).unwrap();
    let revoked = service.revoke(both(), issued.run).unwrap();
    assert_eq!(revoked.state, AuthorityState::Revoked);
    assert_eq!(revoked.revoked_at_ms, Some(4_242));
    assert_eq!(
        refused(service.resolve(&issued.token).unwrap()),
        AuthorityRefusal::Revoked
    );
    assert_eq!(
        service.revoke(both(), issued.run).unwrap().revoked_at_ms,
        Some(4_242)
    );
    assert_eq!(
        service
            .advance_generation(both(), issued.run, AttemptGeneration::FIRST.next())
            .unwrap_err()
            .code,
        "authority_revoked"
    );
}

#[test]
fn the_verbs_take_the_write_scope_and_the_reads_take_the_read_scope() {
    let service = service();
    let read_only = context(&[ArtifactScope::Read]);
    let write_only = context(&[ArtifactScope::Write]);
    assert_eq!(
        service
            .issue(read_only.clone(), issue_request(1, &[]))
            .unwrap_err()
            .code,
        "forbidden"
    );
    let issued = service
        .issue(write_only.clone(), issue_request(1, &[]))
        .unwrap();
    assert_eq!(
        service
            .advance_generation(
                read_only.clone(),
                issued.run,
                AttemptGeneration::FIRST.next()
            )
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service
            .revoke(read_only.clone(), issued.run)
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service
            .authority(write_only.clone(), issued.run)
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service
            .versions_produced_by(write_only, issued.run)
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert!(service.authority(read_only.clone(), issued.run).is_ok());
    assert!(
        service
            .versions_produced_by(read_only, issued.run)
            .unwrap()
            .versions
            .is_empty()
    );
    assert_eq!(
        service
            .authority(both(), RunId::from_uuid(v4(9)))
            .unwrap_err()
            .code,
        "authority_not_found"
    );
}

#[test]
fn the_token_hash_is_the_stores_key_and_the_derivation_is_stable() {
    let hash = hash_token("koru_run_example");
    assert_eq!(hash, hash_token("koru_run_example"));
    assert_ne!(hash, hash_token("koru_run_examplE"));
    assert_eq!(hash.as_str().len(), 64);
}
