// SPDX-License-Identifier: MPL-2.0

//! The person's side of a run's folder, over the real repository: which
//! runs a workspace offers, which of a scan's paths are still not results,
//! and a Register that is the run's own `create` with a person's scope in
//! front of it.

use koru_artifact_core::{ArtifactTitle, BlobHash, Filename, MediaType, RelativePath, WorkspaceId};

use super::*;
use crate::authority::tests::issue_request;
use crate::tests::{both, context, service, v4};
use crate::{ArtifactServerPort, RunAuthority, StagedFile};

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(v4(0xa0))
}

fn staged(path: &str, content: u8) -> StagedFile {
    StagedFile {
        relative_path: path.to_owned(),
        filename: Some(Filename::new(path.rsplit('/').next().unwrap()).unwrap()),
        blob: BlobHash::from_digest("b3", &format!("{content:02x}").repeat(32)).unwrap(),
        size_bytes: 100 + u64::from(content),
    }
}

fn issue(service: &Service<koru_artifact_sqlite::Repository>, run: u8) -> RunAuthorityRecord {
    service.issue(both(), issue_request(run, &[])).unwrap();
    service
        .run_folder(both(), RunId::from_uuid(v4(run)))
        .unwrap()
        .record
}

fn paths(values: &[&str]) -> Vec<RelativePath> {
    values
        .iter()
        .map(|value| RelativePath::new(*value).unwrap())
        .collect()
}

/// The surface's first question, and D29's fact beside it: a run that has
/// registered nothing is a run whose count is zero, not a run whose list
/// happens to be empty.
#[test]
fn a_workspaces_runs_are_listed_with_what_each_has_already_registered() {
    let service = service();
    let first = issue(&service, 1);
    let second = issue(&service, 2);
    service
        .register_file(both(), &second, staged("report.md", 1), None, None)
        .unwrap();

    let folders = service.run_folders(both(), workspace()).unwrap();
    let by_run: Vec<_> = folders
        .iter()
        .map(|target| (target.record.run, target.registered))
        .collect();
    assert!(by_run.contains(&(first.run, 0)), "{by_run:?}");
    assert!(by_run.contains(&(second.run, 1)), "{by_run:?}");
    // Another workspace's runs are not this workspace's business.
    assert!(
        service
            .run_folders(both(), WorkspaceId::from_uuid(v4(0xc0)))
            .unwrap()
            .is_empty()
    );
    // A run this device never issued an authority for has no folder to
    // resolve a path against, and says so rather than guessing one.
    assert_eq!(
        service
            .run_folder(both(), RunId::from_uuid(v4(0x33)))
            .unwrap_err()
            .code,
        "authority_not_found"
    );
}

/// A file is a result when the family the run's own `create` would have
/// made for that path exists — the same identity either way, so an agent's
/// registration and a person's press cannot disagree about what has been
/// adopted.
#[test]
fn registering_a_file_takes_it_out_of_the_unregistered_list_by_the_runs_own_identity() {
    let service = service();
    let record = issue(&service, 1);
    let found = paths(&["report.md", "out/data.csv", "notes.txt"]);

    let before = service
        .unregistered(both(), record.run, found.clone())
        .unwrap();
    assert_eq!(
        before, found,
        "nothing is a result yet, and the order holds"
    );

    let registered = service
        .register_file(both(), &record, staged("out/data.csv", 2), None, None)
        .unwrap();
    assert!(!registered.retried);
    // No title was given, so the file's own last segment named it.
    assert_eq!(registered.family.title.as_str(), "data.csv");
    assert_eq!(registered.family.created_by_run, Some(record.run));
    assert_eq!(registered.family.workspace, workspace());
    assert_eq!(registered.version.ordinal.get(), 1);
    assert_eq!(
        registered.version.produced_by.map(|by| by.run),
        Some(record.run),
        "provenance comes from the record, never from the call"
    );

    let after = service.unregistered(both(), record.run, found).unwrap();
    assert_eq!(after, paths(&["report.md", "notes.txt"]));
}

/// A second press on the same row is the same version, not a second one:
/// the identity is `(run, attempt, path, bytes)`, which is exactly what the
/// run's own retry relies on.
#[test]
fn pressing_register_twice_finds_the_version_the_first_press_made() {
    let service = service();
    let record = issue(&service, 1);
    let first = service
        .register_file(both(), &record, staged("report.md", 1), None, None)
        .unwrap();
    let second = service
        .register_file(both(), &record, staged("report.md", 1), None, None)
        .unwrap();
    assert!(second.retried);
    assert_eq!(second.version.id, first.version.id);

    // The same path written again with new bytes is the *next* version of
    // the same result, which is what makes a family a family.
    let third = service
        .register_file(both(), &record, staged("report.md", 9), None, None)
        .unwrap();
    assert!(!third.retried);
    assert_eq!(third.family.id, first.family.id);
    assert_eq!(third.version.ordinal.get(), 2);
    assert_eq!(third.version.derived_from, Some(first.version.id));
}

/// A retired result takes no new version, and says so with the code the
/// database raises — the refusal the surface renders in the person's own
/// language.
#[test]
fn registering_into_a_retired_result_is_refused_with_the_retired_code() {
    let service = service();
    let record = issue(&service, 1);
    let registered = service
        .register_file(both(), &record, staged("report.md", 1), None, None)
        .unwrap();
    service.retire_family(both(), registered.family.id).unwrap();

    let refused = service
        .register_file(both(), &record, staged("report.md", 9), None, None)
        .unwrap_err();
    assert_eq!(refused.code, "artifact_family_retired");
    assert!(!refused.retryable);
    // And it stays adopted: retiring is a mark on a result, not an undo of
    // one, so the row is not offered as *not yet a result* again.
    let listed = service
        .unregistered(both(), record.run, paths(&["report.md"]))
        .unwrap();
    assert!(
        listed.is_empty(),
        "the family still exists, so it is adopted"
    );
}

/// A title the person supplied is used as given; a media type too.
#[test]
fn a_supplied_title_and_media_type_are_used_as_given() {
    let service = service();
    let record = issue(&service, 1);
    let registered = service
        .register_file(
            both(),
            &record,
            staged("out/q3.md", 1),
            Some(&ArtifactTitle::new("Quarterly findings").unwrap()),
            Some(&MediaType::new("text/markdown").unwrap()),
        )
        .unwrap();
    assert_eq!(registered.family.title.as_str(), "Quarterly findings");
    assert_eq!(registered.version.mime.as_str(), "text/markdown");
}

/// Revocation withdraws the *agent's* ability to act. The person adopting a
/// file it left behind is a different question, and D37 keeps the files for
/// the life of the run.
#[test]
fn a_revoked_run_still_lets_a_person_register_what_it_left_behind() {
    let service = service();
    let record = issue(&service, 1);
    service.revoke(both(), record.run).unwrap();
    let record = service.run_folder(both(), record.run).unwrap().record;
    assert_eq!(record.state, koru_artifact_core::AuthorityState::Revoked);
    let registered = service
        .register_file(both(), &record, staged("report.md", 1), None, None)
        .unwrap();
    assert_eq!(registered.version.ordinal.get(), 1);
}

/// Scope is decided before anything else, on every one of the three.
#[test]
fn every_run_folder_call_needs_its_own_scope() {
    let service = service();
    let record = issue(&service, 1);
    let read_only = context(&[ArtifactScope::Read]);
    let write_only = context(&[ArtifactScope::Write]);

    assert_eq!(
        service
            .run_folders(write_only.clone(), workspace())
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service
            .unregistered(write_only.clone(), record.run, paths(&["a.md"]))
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service
            .register_file(read_only, &record, staged("a.md", 1), None, None)
            .unwrap_err()
            .code,
        "forbidden"
    );
    // And the two reads work with the read scope alone.
    assert!(service.run_folders(both(), workspace()).is_ok());
    assert!(service.run_folder(both(), record.run).is_ok());
}
