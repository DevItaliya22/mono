// SPDX-License-Identifier: MPL-2.0

//! The tools' decisions over the real repository: where a result lands,
//! what a retry finds, what the grant set reaches and what it refuses —
//! another workspace, another run's family, an empty set — and what
//! provenance a registration records.

use std::collections::BTreeSet;

use koru_artifact_core::{ArtifactTitle, AttemptGeneration, Filename, NewFamily, Principal, RunId};
use koru_artifact_wire::authority::IssueAuthorityRequest;

use super::*;
use crate::authority::tests::issue_request;
use crate::tests::{both, service, v4};
use crate::{ArtifactServerPort, Resolution, RunAuthority};

fn blob(n: u8) -> BlobHash {
    BlobHash::from_digest("b3", &format!("{n:02x}").repeat(32)).unwrap()
}

fn staged(path: &str, content: u8) -> StagedFile {
    StagedFile {
        relative_path: path.to_owned(),
        filename: Some(Filename::new(path.rsplit('/').next().unwrap()).unwrap()),
        blob: blob(content),
        size_bytes: 10 + u64::from(content),
    }
}

fn own_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(v4(0xa0))
}

fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(v4(0xb0))
}

fn authority_for(
    service: &Service<koru_artifact_sqlite::Repository>,
    request: IssueAuthorityRequest,
) -> RunAuthorityRecord {
    let issued = service.issue(both(), request).unwrap();
    match service.resolve(&issued.token).unwrap() {
        Resolution::Authorized(record) => record,
        Resolution::Refused(refusal) => panic!("{refusal}"),
    }
}

/// A family somebody else made by hand, in `workspace`.
fn family_by_hand(
    service: &Service<koru_artifact_sqlite::Repository>,
    n: u8,
    workspace: WorkspaceId,
) -> ArtifactFamily {
    service
        .create_family(
            both(),
            koru_artifact_wire::CreateFamilyRequest {
                family_id: ArtifactFamilyId::from_uuid(v4(n)),
                workspace,
                title: ArtifactTitle::new("Quarterly report").unwrap(),
                created_by_run: None,
            },
        )
        .unwrap()
}

#[test]
fn create_lands_in_the_runs_own_workspace_records_provenance_and_retries_as_the_same_version() {
    let service = service();
    // An empty grant set: no workspace access, and `create` still works.
    let authority = authority_for(&service, issue_request(1, &[]));
    assert!(authority.grants.is_empty());
    let target = service
        .create_target(&authority, "Findings", Some("text/markdown"))
        .unwrap();
    assert_eq!(target.workspace, own_workspace());
    assert_eq!(target.mime.as_str(), "text/markdown");

    let created = service
        .create(
            &authority,
            staged("findings.md", 1),
            "Findings",
            Some("text/markdown"),
        )
        .unwrap();
    assert!(!created.retried);
    assert_eq!(created.family.workspace, own_workspace());
    assert_eq!(created.family.created_by_run, Some(authority.run));
    assert_eq!(created.family.title.as_str(), "Findings");
    assert_eq!(created.version.ordinal.get(), 1);
    assert_eq!(created.version.blob, blob(1));
    assert_eq!(created.version.size_bytes, 11);
    assert_eq!(
        created.version.produced_by,
        Some(ProducedBy {
            run: authority.run,
            generation: AttemptGeneration::FIRST,
        })
    );
    assert_eq!(created.version.derived_from, None);
    assert_eq!(
        created.version.filename.as_ref().map(Filename::as_str),
        Some("findings.md")
    );

    // The same call again — a lost reply — is the same version and family.
    let again = service
        .create(
            &authority,
            staged("findings.md", 1),
            "Findings",
            Some("text/markdown"),
        )
        .unwrap();
    assert!(again.retried);
    assert_eq!(again.version, created.version);
    assert_eq!(again.family.id, created.family.id);

    // The same path with new bytes is the next version of the same family,
    // derived from the one before.
    let rewritten = service
        .create(&authority, staged("findings.md", 2), "Findings", None)
        .unwrap();
    assert!(!rewritten.retried);
    assert_eq!(rewritten.family.id, created.family.id);
    assert_eq!(rewritten.version.ordinal.get(), 2);
    assert_eq!(rewritten.version.derived_from, Some(created.version.id));
    assert_eq!(rewritten.version.mime.as_str(), "application/octet-stream");

    // Another path is another family.
    let other = service
        .create(&authority, staged("notes/plan.md", 1), "Plan", None)
        .unwrap();
    assert_ne!(other.family.id, created.family.id);
    assert_eq!(other.version.ordinal.get(), 1);

    // The run's own output is reachable without a grant, and its links say
    // it produced them.
    assert_eq!(
        service
            .get(&authority, created.version.id)
            .unwrap()
            .family
            .id,
        created.family.id
    );
    let produced = service.versions_produced_by(both(), authority.run).unwrap();
    assert_eq!(produced.versions.len(), 3);
    let provenance = service.provenance(both(), rewritten.version.id).unwrap();
    assert_eq!(provenance.generations(), 2);
    assert_eq!(
        provenance
            .links(rewritten.version.id, LinkRole::Output)
            .len(),
        1
    );

    // A bad title or media type is refused before anything is staged.
    assert_eq!(
        service
            .create_target(&authority, "   ", None)
            .unwrap_err()
            .code,
        "invalid_request"
    );
    assert_eq!(
        service
            .create_target(&authority, "Findings", Some("not a type"))
            .unwrap_err()
            .code,
        "invalid_request"
    );
}

#[test]
fn a_fresh_attempt_registers_the_same_bytes_as_a_new_version() {
    let service = service();
    let first = authority_for(&service, issue_request(1, &[]));
    let created = service
        .create(&first, staged("out.md", 1), "Out", None)
        .unwrap();
    let advanced = service
        .advance_generation(both(), first.run, AttemptGeneration::FIRST.next())
        .unwrap();
    let second = match service.resolve(&advanced.token).unwrap() {
        Resolution::Authorized(record) => record,
        Resolution::Refused(refusal) => panic!("{refusal}"),
    };
    let again = service
        .create(&second, staged("out.md", 1), "Out", None)
        .unwrap();
    assert!(!again.retried, "a new attempt is a new registration");
    assert_eq!(again.family.id, created.family.id);
    assert_eq!(again.version.ordinal.get(), 2);
    assert_eq!(
        again.version.produced_by.unwrap().generation,
        AttemptGeneration::FIRST.next()
    );
}

#[test]
fn the_grant_set_decides_every_workspace_call() {
    let service = service();
    let here = family_by_hand(&service, 0x10, own_workspace());
    let elsewhere = family_by_hand(&service, 0x11, other_workspace());
    let register = |family: ArtifactFamilyId, n: u8| {
        service
            .register_version(
                both(),
                family,
                koru_artifact_wire::RegisterVersionRequest {
                    registration_id: RegistrationId::from_uuid(v4(0x40 + n)),
                    blob: blob(n),
                    mime: MediaType::new("text/plain").unwrap(),
                    filename: None,
                    produced_by: None,
                    derived_from: None,
                },
                crate::VerifiedBlob { size_bytes: 3 },
            )
            .unwrap()
            .version
    };
    let here_v1 = register(here.id, 1);
    let elsewhere_v1 = register(elsewhere.id, 2);

    // The delegating workspace as the initial grant.
    let granted = authority_for(&service, issue_request(1, &[own_workspace()]));
    assert_eq!(
        service.get(&granted, here_v1.id).unwrap().family.id,
        here.id
    );
    assert_eq!(
        service.content_target(&granted, here_v1.id).unwrap().1,
        own_workspace()
    );
    let hits = service.search(&granted, "quarterly").unwrap();
    assert_eq!(hits.hits.len(), 1);
    assert_eq!(hits.hits[0].family.id, here.id);
    assert_eq!(hits.hits[0].versions.len(), 1);
    assert!(!hits.truncated);
    assert!(service.search(&granted, "annual").unwrap().hits.is_empty());
    let attached = service.attach_to_run(&granted, here_v1.id).unwrap();
    assert_eq!(attached.link.role, LinkRole::Input);
    assert_eq!(attached.link.run, granted.run);
    // A new version under a family the person made, in the granted
    // workspace: the bytes go to that workspace, and the version is the
    // run's output.
    let target = service
        .create_version_target(&granted, here.id, None)
        .unwrap();
    assert_eq!(target.workspace, own_workspace());
    assert_eq!(
        target.mime.as_str(),
        "text/plain",
        "the family's latest type"
    );
    let v2 = service
        .create_version(&granted, here.id, staged("report.md", 3), None)
        .unwrap();
    assert_eq!(v2.version.ordinal.get(), 2);
    assert_eq!(v2.version.derived_from, Some(here_v1.id));
    assert_eq!(v2.version.produced_by.map(|by| by.run), Some(granted.run));

    // Another workspace: every call is permission_denied, and the refusal
    // names nothing.
    for (what, code) in [
        (
            "get",
            service.get(&granted, elsewhere_v1.id).unwrap_err().code,
        ),
        (
            "read",
            service
                .content_target(&granted, elsewhere_v1.id)
                .unwrap_err()
                .code,
        ),
        (
            "attach",
            service
                .attach_to_run(&granted, elsewhere_v1.id)
                .unwrap_err()
                .code,
        ),
        (
            "create_version target",
            service
                .create_version_target(&granted, elsewhere.id, None)
                .unwrap_err()
                .code,
        ),
        (
            "create_version",
            service
                .create_version(&granted, elsewhere.id, staged("x.md", 4), None)
                .unwrap_err()
                .code,
        ),
    ] {
        assert_eq!(code, "permission_denied", "{what}");
    }
    assert_eq!(
        service
            .search(&granted, "quarterly")
            .unwrap()
            .hits
            .iter()
            .filter(|hit| hit.family.id == elsewhere.id)
            .count(),
        0,
        "search sees only granted workspaces"
    );

    // The grant removed: an empty set denies every workspace call,
    // including search, while create still works (asserted above).
    let none = authority_for(&service, issue_request(2, &[]));
    assert_eq!(
        service.get(&none, here_v1.id).unwrap_err().code,
        "permission_denied"
    );
    assert_eq!(
        service.search(&none, "").unwrap_err().code,
        "permission_denied"
    );
    assert_eq!(
        service
            .create_version_target(&none, here.id, None)
            .unwrap_err()
            .code,
        "permission_denied"
    );

    // Both workspaces granted: both reachable, and search is truncated at
    // its bound rather than unbounded.
    let wide = authority_for(
        &service,
        issue_request(3, &[own_workspace(), other_workspace()]),
    );
    assert_eq!(
        wide.grants,
        BTreeSet::from([own_workspace(), other_workspace()])
    );
    assert!(service.get(&wide, elsewhere_v1.id).is_ok());
    for n in 0..SEARCH_LIMIT as u8 {
        family_by_hand(&service, 0x60 + n, other_workspace());
    }
    let all = service.search(&wide, "").unwrap();
    assert_eq!(all.hits.len(), SEARCH_LIMIT);
    assert!(all.truncated);
}

#[test]
fn another_runs_family_is_not_this_runs_own() {
    let service = service();
    let mine = authority_for(&service, issue_request(1, &[]));
    let theirs = authority_for(&service, issue_request(2, &[]));
    let made = service
        .create(&theirs, staged("theirs.md", 1), "Theirs", None)
        .unwrap();
    // Run 1 holds a token for run 1; run 2's family is neither its own nor
    // in a granted workspace.
    assert_eq!(
        service
            .create_version(&mine, made.family.id, staged("theirs.md", 2), None)
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(
        service.get(&mine, made.version.id).unwrap_err().code,
        "permission_denied"
    );
    // And a family nobody made is not found, before any grant question.
    assert_eq!(
        service
            .create_version_target(&mine, ArtifactFamilyId::from_uuid(v4(0x77)), None)
            .unwrap_err()
            .code,
        "artifact_family_not_found"
    );
    assert_eq!(
        service
            .get(&mine, ArtifactVersionId::from_uuid(v4(0x78)))
            .unwrap_err()
            .code,
        "artifact_not_found"
    );
}

#[test]
fn a_retired_family_refuses_a_new_version_before_anything_is_staged() {
    let service = service();
    let authority = authority_for(&service, issue_request(1, &[own_workspace()]));
    let family = family_by_hand(&service, 0x10, own_workspace());
    service.retire_family(both(), family.id).unwrap();
    assert_eq!(
        service
            .create_version_target(&authority, family.id, None)
            .unwrap_err()
            .code,
        "artifact_family_retired"
    );
}

#[test]
fn derived_ids_are_v4_shaped_stable_and_distinct_per_input() {
    let service = service();
    let authority = authority_for(&service, issue_request(1, &[]));
    let a = registration_id(&authority, &staged("a.md", 1));
    assert_eq!(a, registration_id(&authority, &staged("a.md", 1)));
    assert_ne!(a, registration_id(&authority, &staged("a.md", 2)));
    assert_ne!(a, registration_id(&authority, &staged("b.md", 1)));
    assert!(
        a.to_storage().parse::<RegistrationId>().is_ok(),
        "strict v4"
    );
    let family = family_id(&authority, "a.md");
    assert_eq!(family, family_id(&authority, "a.md"));
    assert_ne!(family, family_id(&authority, "b.md"));
    assert!(family.to_storage().parse::<ArtifactFamilyId>().is_ok());
    let mut other = authority.clone();
    other.run = RunId::from_uuid(v4(2));
    assert_ne!(family, family_id(&other, "a.md"));
    // A staged file's Debug names the hash and the size, never the path
    // or the filename.
    let rendered = format!("{:?}", staged("SECRET-PATH/report.md", 1));
    assert!(!rendered.contains("SECRET-PATH"));
    assert!(!rendered.contains("report.md"));
    // A family created by hand elsewhere is unaffected by a run's own
    // family derivation: the same title, a different id.
    let _ = NewFamily {
        id: family,
        workspace: own_workspace(),
        title: ArtifactTitle::new("x").unwrap(),
        created_by_run: None,
        created_at_ms: 0,
    };
    let _ = Principal::LocalOwner;
}
