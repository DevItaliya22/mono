// SPDX-License-Identifier: MPL-2.0

//! The port's properties against a real (in-memory or temporary) database:
//! immutability at the SQL layer with the stable code, a registration
//! retry that is the same version, a three-generation provenance walk, a
//! retired family refusing new versions, and two roots that share nothing.

use koru_artifact_core::{RegistrationId, authorize};
use uuid::Uuid;

use super::*;

fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(v4(0xa0))
}

fn run(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

fn blob(n: u8) -> BlobHash {
    BlobHash::from_digest("b3", &format!("{n:02x}").repeat(32)).unwrap()
}

fn new_family(n: u8) -> NewFamily {
    NewFamily {
        id: ArtifactFamilyId::from_uuid(v4(n)),
        workspace: workspace(),
        title: ArtifactTitle::new("Report").unwrap(),
        created_by_run: Some(run(0x10)),
        created_at_ms: i64::from(n),
    }
}

fn new_version(
    family: ArtifactFamilyId,
    registration: u8,
    blob_n: u8,
    derived_from: Option<ArtifactVersionId>,
    produced_by: Option<RunId>,
) -> NewVersion {
    NewVersion {
        registration_id: RegistrationId::from_uuid(v4(registration)),
        family,
        blob: blob(blob_n),
        size_bytes: 12,
        mime: MediaType::new("text/markdown").unwrap(),
        filename: Some(Filename::new("report.md").unwrap()),
        produced_by: produced_by.map(|run| ProducedBy {
            run,
            generation: AttemptGeneration::FIRST,
        }),
        derived_from,
        created_at_ms: 100 + i64::from(registration),
    }
}

fn repository_with_family() -> (Repository, ArtifactFamily) {
    let mut repository = Repository::open_in_memory().unwrap();
    let family = repository
        .create_family(new_family(1), Principal::LocalOwner)
        .unwrap();
    (repository, family)
}

#[test]
fn creating_a_family_grants_the_owner_and_refuses_a_duplicate_id() {
    let (mut repository, family) = repository_with_family();
    assert_eq!(family.id, ArtifactFamilyId::from_uuid(v4(1)));
    assert_eq!(family.title.as_str(), "Report");
    assert!(!family.is_retired());
    let permissions = repository.permissions(family.id).unwrap();
    assert_eq!(permissions.len(), 1);
    assert_eq!(permissions[0].principal, Principal::LocalOwner);
    assert_eq!(permissions[0].access, Access::Owner);
    assert_eq!(
        authorize(&permissions, Principal::LocalOwner, Access::Owner),
        Ok(())
    );
    assert_eq!(
        repository
            .create_family(new_family(1), Principal::LocalOwner)
            .unwrap_err(),
        ArtifactError::FamilyExists
    );
    assert_eq!(
        repository
            .family(ArtifactFamilyId::from_uuid(v4(9)))
            .unwrap_err(),
        ArtifactError::FamilyNotFound
    );
}

#[test]
fn listing_is_per_workspace_and_hides_retired_families_unless_asked() {
    let (mut repository, first) = repository_with_family();
    let second = repository
        .create_family(new_family(2), Principal::LocalOwner)
        .unwrap();
    let elsewhere = NewFamily {
        workspace: WorkspaceId::from_uuid(v4(0xb0)),
        ..new_family(3)
    };
    repository
        .create_family(elsewhere, Principal::LocalOwner)
        .unwrap();
    let ids =
        |families: Vec<ArtifactFamily>| families.into_iter().map(|f| f.id).collect::<Vec<_>>();
    assert_eq!(
        ids(repository.list_families(workspace(), false).unwrap()),
        [first.id, second.id]
    );
    let retired = repository.retire_family(second.id, 500).unwrap();
    assert_eq!(retired.retired_at_ms, Some(500));
    assert_eq!(
        ids(repository.list_families(workspace(), false).unwrap()),
        [first.id]
    );
    assert_eq!(
        ids(repository.list_families(workspace(), true).unwrap()),
        [first.id, second.id]
    );
    // A second retirement keeps the first mark.
    assert_eq!(
        repository
            .retire_family(second.id, 900)
            .unwrap()
            .retired_at_ms,
        Some(500)
    );
}

#[test]
fn a_version_is_immutable_at_the_sql_layer_with_the_stable_code() {
    let (mut repository, family) = repository_with_family();
    let registered = repository
        .register_version(new_version(family.id, 0x20, 1, None, None))
        .unwrap();
    assert!(!registered.retried);
    assert_eq!(registered.version.ordinal, Ordinal::FIRST);
    let rewrite = repository.connection().execute(
        "UPDATE artifact_versions SET blob_hash = ?2 WHERE version_id = ?1",
        params![registered.version.id.to_storage(), blob(2).as_str()],
    );
    let error = map_sql("rewrite a version", rewrite.unwrap_err());
    assert_eq!(error, ArtifactError::VersionImmutable);
    assert_eq!(error.code(), "artifact_version_immutable");
    // Nothing moved.
    assert_eq!(
        repository.version(registered.version.id).unwrap(),
        registered.version
    );
}

#[test]
fn a_registration_retry_is_the_same_version_and_a_conflict_is_named() {
    let (mut repository, family) = repository_with_family();
    let first = repository
        .register_version(new_version(family.id, 0x20, 1, None, None))
        .unwrap();
    let retry = repository
        .register_version(new_version(family.id, 0x20, 1, None, None))
        .unwrap();
    assert!(retry.retried);
    assert_eq!(retry.version, first.version);
    assert_eq!(repository.versions(family.id).unwrap().len(), 1);

    // The same registration id with other bytes is not a retry.
    assert_eq!(
        repository
            .register_version(new_version(family.id, 0x20, 2, None, None))
            .unwrap_err(),
        ArtifactError::RegistrationConflict
    );
    // Nor under another family.
    let other = repository
        .create_family(new_family(2), Principal::LocalOwner)
        .unwrap();
    assert_eq!(
        repository
            .register_version(new_version(other.id, 0x20, 1, None, None))
            .unwrap_err(),
        ArtifactError::RegistrationConflict
    );
    // A different registration id for the same bytes is a second version.
    let second = repository
        .register_version(new_version(family.id, 0x21, 1, None, None))
        .unwrap();
    assert_ne!(second.version.id, first.version.id);
    assert_eq!(second.version.ordinal, Ordinal::FIRST.next());
}

/// `v1` → `v2` derived from it, consumed by a run that produced `v3`.
#[test]
fn provenance_walks_three_generations() {
    let (mut repository, family) = repository_with_family();
    let v1 = repository
        .register_version(new_version(family.id, 0x20, 1, None, Some(run(0x10))))
        .unwrap()
        .version;
    repository
        .link_run(RunLink {
            run: run(0x10),
            version: v1.id,
            role: LinkRole::Output,
            linked_at_ms: 1,
        })
        .unwrap();
    let v2 = repository
        .register_version(new_version(family.id, 0x21, 2, Some(v1.id), None))
        .unwrap()
        .version;
    repository
        .link_run(RunLink {
            run: run(0x11),
            version: v2.id,
            role: LinkRole::Input,
            linked_at_ms: 2,
        })
        .unwrap();
    // Linking twice is one link.
    repository
        .link_run(RunLink {
            run: run(0x11),
            version: v2.id,
            role: LinkRole::Input,
            linked_at_ms: 3,
        })
        .unwrap();
    let v3 = repository
        .register_version(new_version(
            family.id,
            0x22,
            3,
            Some(v2.id),
            Some(run(0x11)),
        ))
        .unwrap()
        .version;
    repository
        .link_run(RunLink {
            run: run(0x11),
            version: v3.id,
            role: LinkRole::Output,
            linked_at_ms: 4,
        })
        .unwrap();

    let provenance = repository.provenance(v3.id).unwrap();
    assert_eq!(provenance.subject, v3);
    assert_eq!(provenance.generations(), 3);
    assert_eq!(
        provenance
            .ancestors
            .iter()
            .map(|version| version.id)
            .collect::<Vec<_>>(),
        [v2.id, v1.id]
    );
    assert_eq!(provenance.runs.len(), 3);
    assert_eq!(provenance.links(v3.id, LinkRole::Output)[0].run, run(0x11));
    assert_eq!(provenance.links(v2.id, LinkRole::Input)[0].run, run(0x11));
    assert_eq!(provenance.links(v2.id, LinkRole::Input)[0].linked_at_ms, 2);
    assert_eq!(provenance.links(v1.id, LinkRole::Output)[0].run, run(0x10));
    assert_eq!(v3.produced_by.unwrap().run, run(0x11));

    // The root's provenance is itself.
    let root = repository.provenance(v1.id).unwrap();
    assert!(root.ancestors.is_empty());
    assert_eq!(root.runs.len(), 1);

    // A derivation from nothing is refused.
    assert_eq!(
        repository
            .register_version(new_version(
                family.id,
                0x23,
                4,
                Some(ArtifactVersionId::from_uuid(v4(0x77))),
                None
            ))
            .unwrap_err(),
        ArtifactError::VersionNotFound
    );
    assert_eq!(
        repository
            .link_run(RunLink {
                run: run(0x11),
                version: ArtifactVersionId::from_uuid(v4(0x77)),
                role: LinkRole::Input,
                linked_at_ms: 9,
            })
            .unwrap_err(),
        ArtifactError::VersionNotFound
    );
}

#[test]
fn a_retired_family_refuses_new_versions_and_keeps_the_old_ones() {
    let (mut repository, family) = repository_with_family();
    let kept = repository
        .register_version(new_version(family.id, 0x20, 1, None, None))
        .unwrap()
        .version;
    repository.retire_family(family.id, 500).unwrap();
    assert_eq!(
        repository
            .register_version(new_version(family.id, 0x21, 2, None, None))
            .unwrap_err(),
        ArtifactError::FamilyRetired
    );
    assert_eq!(repository.versions(family.id).unwrap(), vec![kept.clone()]);
    assert_eq!(repository.version(kept.id).unwrap(), kept);
    // A retry of the earlier registration still finds it.
    assert!(
        repository
            .register_version(new_version(family.id, 0x20, 1, None, None))
            .unwrap()
            .retried
    );
}

#[test]
fn two_roots_hold_two_independent_databases() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut on_a = Repository::open(&a.path().join("artifacts")).unwrap();
    on_a.create_family(new_family(1), Principal::LocalOwner)
        .unwrap();
    let on_b = Repository::open(&b.path().join("artifacts")).unwrap();
    assert!(on_b.list_families(workspace(), true).unwrap().is_empty());
    assert!(a.path().join("artifacts").join(DATABASE_FILE).exists());
    assert!(b.path().join("artifacts").join(DATABASE_FILE).exists());
    assert_eq!(on_a.list_families(workspace(), true).unwrap().len(), 1);
}

#[test]
fn a_closed_domain_is_refused_by_the_database() {
    let (repository, family) = repository_with_family();
    let refused = repository.connection().execute(
        "INSERT INTO artifact_permissions (family_id, principal, access, granted_at_ms)
         VALUES (?1, 'local', 'write', 1)",
        params![family.id.to_storage()],
    );
    assert!(refused.is_err(), "'write' is not an access");
}

/// **A result outlives the run that made it, and the walk says so.**
///
/// A person may delete a run and keep what it produced. The version stays
/// and still names its producer, so the walk carries which of the runs it
/// named no longer exist — the difference between a sentence that says *a
/// run that was deleted* and one that prints an id resolving to nothing.
/// A run this store never knew is not deleted; it is simply a run.
#[test]
fn a_walk_names_which_of_its_runs_were_deleted() {
    use koru_artifact_core::RunAuthorityRepository;

    let (mut repository, family) = repository_with_family();
    let v1 = repository
        .register_version(new_version(family.id, 0x20, 1, None, Some(run(0x10))))
        .unwrap()
        .version;
    let v2 = repository
        .register_version(new_version(
            family.id,
            0x21,
            2,
            Some(v1.id),
            Some(run(0x11)),
        ))
        .unwrap()
        .version;
    repository
        .link_run(RunLink {
            run: run(0x12),
            version: v2.id,
            role: LinkRole::Input,
            linked_at_ms: 2,
        })
        .unwrap();

    assert!(
        repository
            .provenance(v2.id)
            .unwrap()
            .deleted_runs
            .is_empty(),
        "nothing is deleted until something is"
    );

    // The run that made the ancestor, and the run that read it, both go.
    repository.forget_authority(run(0x10), 900).unwrap();
    repository.forget_authority(run(0x12), 900).unwrap();

    let walk = repository.provenance(v2.id).unwrap();
    assert_eq!(
        walk.deleted_runs,
        std::collections::BTreeSet::from([run(0x10), run(0x12)]),
        "the producer of an ancestor and an input link are both named"
    );
    assert!(
        !walk.deleted_runs.contains(&run(0x11)),
        "and the run that is still there is not"
    );
}
