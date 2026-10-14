// SPDX-License-Identifier: MPL-2.0

//! The service's two decisions — scope, then permission — and its mapping
//! of the store's answers onto the wire, over the real SQLite repository in
//! memory.

use koru_artifact_core::{ArtifactTitle, BlobHash, MediaType, Principal, WorkspaceId};
use koru_artifact_wire::{LinkRole, RegistrationId};
use uuid::Uuid;

use super::*;

pub(crate) fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

pub(crate) fn service() -> Service<koru_artifact_sqlite::Repository> {
    Service::with_clock(
        koru_artifact_sqlite::Repository::open_in_memory().unwrap(),
        || 4_242,
    )
}

pub(crate) fn context(scopes: &[ArtifactScope]) -> RequestContext {
    RequestContext {
        principal: ArtifactPrincipal {
            label: "test".into(),
            scopes: scopes.to_vec(),
            principal: Principal::LocalOwner,
        },
        trace_id: "trace".into(),
    }
}

pub(crate) fn both() -> RequestContext {
    context(&[ArtifactScope::Read, ArtifactScope::Write])
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(v4(0xa0))
}

fn create(service: &Service<koru_artifact_sqlite::Repository>, n: u8) -> ArtifactFamily {
    service
        .create_family(
            both(),
            CreateFamilyRequest {
                family_id: ArtifactFamilyId::from_uuid(v4(n)),
                workspace: workspace(),
                title: ArtifactTitle::new("Report").unwrap(),
                created_by_run: None,
            },
        )
        .unwrap()
}

fn register(
    service: &Service<koru_artifact_sqlite::Repository>,
    family: ArtifactFamilyId,
    registration: u8,
    derived_from: Option<ArtifactVersionId>,
) -> RegisterVersionResponse {
    service
        .register_version(
            both(),
            family,
            RegisterVersionRequest {
                registration_id: RegistrationId::from_uuid(v4(registration)),
                blob: BlobHash::from_digest("b3", &"ab".repeat(32)).unwrap(),
                mime: MediaType::new("text/markdown").unwrap(),
                filename: None,
                produced_by: None,
                derived_from,
            },
            VerifiedBlob { size_bytes: 77 },
        )
        .unwrap()
}

#[test]
fn scope_tokens_are_namespaced_and_round_trip() {
    for scope in [ArtifactScope::Read, ArtifactScope::Write] {
        assert!(scope.as_str().starts_with("artifacts."));
        assert_eq!(ArtifactScope::parse(scope.as_str()), Some(scope));
    }
    assert_eq!(ArtifactScope::parse("store.read_documents"), None);
}

#[test]
fn a_call_without_its_scope_is_forbidden_before_the_store_is_asked() {
    let service = service();
    let read_only = context(&[ArtifactScope::Read]);
    let refused = service
        .create_family(
            read_only.clone(),
            CreateFamilyRequest {
                family_id: ArtifactFamilyId::from_uuid(v4(1)),
                workspace: workspace(),
                title: ArtifactTitle::new("Report").unwrap(),
                created_by_run: None,
            },
        )
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");
    let write_only = context(&[ArtifactScope::Write]);
    assert_eq!(
        service
            .list_families(
                write_only,
                ListFamiliesQuery {
                    workspace: workspace(),
                    include_retired: false
                }
            )
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert!(
        service
            .list_families(
                read_only,
                ListFamiliesQuery {
                    workspace: workspace(),
                    include_retired: false
                }
            )
            .unwrap()
            .families
            .is_empty()
    );
}

#[test]
fn the_creator_owns_the_family_and_the_clock_stamps_it() {
    let service = service();
    let family = create(&service, 1);
    assert_eq!(family.created_at_ms, 4_242);
    let listed = service
        .list_families(
            both(),
            ListFamiliesQuery {
                workspace: workspace(),
                include_retired: false,
            },
        )
        .unwrap();
    assert_eq!(listed.families, vec![family.clone()]);
    let response = service.family(both(), family.id).unwrap();
    assert_eq!(response.family, family);
    assert!(response.versions.is_empty());
}

#[test]
fn a_principal_without_a_grant_is_denied_with_the_stable_code() {
    let service = service();
    let family = create(&service, 1);
    let stranger = RequestContext {
        principal: ArtifactPrincipal {
            principal: Principal::Run {
                run: koru_artifact_core::RunId::from_uuid(v4(9)),
            },
            ..both().principal
        },
        ..both()
    };
    assert_eq!(
        service
            .family(stranger.clone(), family.id)
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(
        service
            .retire_family(stranger.clone(), family.id)
            .unwrap_err()
            .code,
        "permission_denied"
    );
    // Not listed either, rather than refused: the list is what one may read.
    assert!(
        service
            .list_families(
                stranger,
                ListFamiliesQuery {
                    workspace: workspace(),
                    include_retired: true
                }
            )
            .unwrap()
            .families
            .is_empty()
    );
}

#[test]
fn registration_uses_the_verified_size_and_a_retry_is_the_same_version() {
    let service = service();
    let family = create(&service, 1);
    let first = register(&service, family.id, 0x20, None);
    assert!(!first.retried);
    assert_eq!(first.version.size_bytes, 77);
    assert_eq!(first.version.created_at_ms, 4_242);
    let retry = register(&service, family.id, 0x20, None);
    assert!(retry.retried);
    assert_eq!(retry.version, first.version);
    assert_eq!(
        service.version(both(), first.version.id).unwrap(),
        first.version
    );
}

#[test]
fn provenance_and_links_cross_the_port() {
    let service = service();
    let family = create(&service, 1);
    let v1 = register(&service, family.id, 0x20, None).version;
    let run = koru_artifact_core::RunId::from_uuid(v4(0x30));
    let link = service
        .link_run(
            both(),
            v1.id,
            LinkRunRequest {
                run,
                role: LinkRole::Input,
            },
        )
        .unwrap();
    assert_eq!(link.linked_at_ms, 4_242);
    let v2 = register(&service, family.id, 0x21, Some(v1.id)).version;
    let provenance = service.provenance(both(), v2.id).unwrap();
    assert_eq!(provenance.generations(), 2);
    assert_eq!(provenance.links(v1.id, LinkRole::Input)[0].run, run);
    assert_eq!(
        service
            .provenance(both(), ArtifactVersionId::from_uuid(v4(0x77)))
            .unwrap_err()
            .code,
        "artifact_not_found"
    );
}

#[test]
fn a_retired_family_refuses_registration_through_the_port() {
    let service = service();
    let family = create(&service, 1);
    assert_eq!(
        service.registration_target(both(), family.id).unwrap(),
        workspace()
    );
    let retired = service.retire_family(both(), family.id).unwrap();
    assert_eq!(retired.retired_at_ms, Some(4_242));
    assert_eq!(
        service
            .registration_target(both(), family.id)
            .unwrap_err()
            .code,
        "artifact_family_retired"
    );
    assert_eq!(
        service
            .registration_target(context(&[ArtifactScope::Read]), family.id)
            .unwrap_err()
            .code,
        "forbidden"
    );
    let refused = service
        .register_version(
            both(),
            family.id,
            RegisterVersionRequest {
                registration_id: RegistrationId::from_uuid(v4(0x22)),
                blob: BlobHash::from_digest("b3", &"cd".repeat(32)).unwrap(),
                mime: MediaType::new("text/plain").unwrap(),
                filename: None,
                produced_by: None,
                derived_from: None,
            },
            VerifiedBlob { size_bytes: 1 },
        )
        .unwrap_err();
    assert_eq!(refused.code, "artifact_family_retired");
    assert!(!refused.retryable);
}

#[test]
fn a_title_over_its_bound_is_refused_at_the_boundary() {
    let service = service();
    let refused = service
        .create_family(
            both(),
            CreateFamilyRequest {
                family_id: ArtifactFamilyId::from_uuid(v4(1)),
                workspace: workspace(),
                // Deserialization accepts what arrives; the service is where
                // the bound holds, before the store sees the row.
                title: serde_json::from_str(&format!(
                    "\"{}\"",
                    "x".repeat(koru_artifact_core::MAX_ARTIFACT_TITLE_CHARS + 1)
                ))
                .unwrap(),
                created_by_run: None,
            },
        )
        .unwrap_err();
    assert_eq!(refused.code, "invalid_request");
}
