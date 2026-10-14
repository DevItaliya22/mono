// SPDX-License-Identifier: MPL-2.0

//! The authority record against a real database: issue once, grants as a
//! set, a token that resolves, an advance that retires the old token as
//! *stale* rather than *unknown*, a revocation that stays marked, two
//! roots, and the closed domains the database `CHECK`s.

use std::path::Path;

use koru_artifact_core::{AuthorityRefusal, RunWorkspace};
use uuid::Uuid;

use super::*;

fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

fn run(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

fn workspace(n: u8) -> WorkspaceId {
    WorkspaceId::from_uuid(v4(n))
}

fn token(n: u8) -> TokenHash {
    TokenHash::from_hex(format!("{n:02x}").repeat(32)).unwrap()
}

fn run_workspace() -> RunWorkspace {
    let path = if cfg!(windows) {
        Path::new(r"C:\runs\one")
    } else {
        Path::new("/runs/one")
    };
    RunWorkspace::new(path).unwrap()
}

fn new_authority(n: u8, grants: &[WorkspaceId]) -> NewAuthority {
    NewAuthority {
        run: run(n),
        workspace: workspace(0xa0),
        generation: AttemptGeneration::FIRST,
        grants: grants.iter().copied().collect(),
        run_workspace: run_workspace(),
        token_hash: token(n),
        created_at_ms: 100,
    }
}

#[test]
fn issuing_records_the_grants_as_a_set_and_refuses_a_second_issue() {
    let mut repository = Repository::open_in_memory().unwrap();
    let issued = repository
        .issue_authority(new_authority(1, &[workspace(0xa0), workspace(0xb0)]))
        .unwrap();
    assert_eq!(issued.run, run(1));
    assert_eq!(issued.workspace, workspace(0xa0));
    assert_eq!(issued.generation, AttemptGeneration::FIRST);
    assert_eq!(
        issued.grants,
        BTreeSet::from([workspace(0xa0), workspace(0xb0)])
    );
    assert_eq!(issued.run_workspace, run_workspace());
    assert_eq!(issued.state, AuthorityState::Active);
    assert_eq!(issued.revoked_at_ms, None);
    assert_eq!(repository.authority(run(1)).unwrap(), issued);
    assert_eq!(
        repository
            .issue_authority(new_authority(1, &[]))
            .unwrap_err(),
        ArtifactError::AuthorityExists
    );
    assert_eq!(
        repository.authority(run(2)).unwrap_err(),
        ArtifactError::AuthorityNotFound
    );
    // An empty grant set is a legal record: no workspace access.
    let none = repository.issue_authority(new_authority(2, &[])).unwrap();
    assert!(none.grants.is_empty());
}

#[test]
fn a_token_resolves_and_an_advance_retires_it_as_stale() {
    let mut repository = Repository::open_in_memory().unwrap();
    repository
        .issue_authority(new_authority(1, &[workspace(0xa0)]))
        .unwrap();
    let first = repository.lookup_token(&token(1)).unwrap().unwrap();
    assert_eq!(first.token_generation, AttemptGeneration::FIRST);
    assert!(!first.retired);
    assert!(AuthorityRefusal::decide(Some(first)).is_ok());
    assert_eq!(repository.lookup_token(&token(9)).unwrap(), None);
    assert_eq!(
        AuthorityRefusal::decide(repository.lookup_token(&token(9)).unwrap()),
        Err(AuthorityRefusal::Unknown)
    );

    // Not newer: refused with the fence's own code, and nothing moves.
    assert_eq!(
        repository
            .advance_authority(run(1), AttemptGeneration::FIRST, token(2), 200)
            .unwrap_err(),
        ArtifactError::StaleGeneration
    );
    assert_eq!(repository.lookup_token(&token(2)).unwrap(), None);

    let second = AttemptGeneration::FIRST.next();
    let advanced = repository
        .advance_authority(run(1), second, token(2), 200)
        .unwrap();
    assert_eq!(advanced.generation, second);
    assert_eq!(
        advanced.grants,
        BTreeSet::from([workspace(0xa0)]),
        "an advance re-reads the grants and never widens them"
    );
    // The old token is kept and marked, so it is refused as *stale*.
    let stale = repository.lookup_token(&token(1)).unwrap().unwrap();
    assert!(stale.retired);
    assert_eq!(stale.token_generation, AttemptGeneration::FIRST);
    assert_eq!(
        AuthorityRefusal::decide(Some(stale)),
        Err(AuthorityRefusal::StaleGeneration)
    );
    let current = repository.lookup_token(&token(2)).unwrap().unwrap();
    assert!(!current.retired);
    assert_eq!(current.token_generation, second);
    assert_eq!(
        AuthorityRefusal::decide(Some(current)).unwrap().generation,
        second
    );
    // A generation may be skipped (a resume that failed before its token
    // was used) but never repeated.
    assert_eq!(
        repository
            .advance_authority(run(1), second, token(3), 300)
            .unwrap_err(),
        ArtifactError::StaleGeneration
    );
    assert_eq!(
        repository
            .advance_authority(run(2), second, token(3), 300)
            .unwrap_err(),
        ArtifactError::AuthorityNotFound
    );
}

#[test]
fn revocation_is_a_mark_that_every_token_then_reads() {
    let mut repository = Repository::open_in_memory().unwrap();
    repository.issue_authority(new_authority(1, &[])).unwrap();
    let revoked = repository.revoke_authority(run(1), 500).unwrap();
    assert_eq!(revoked.state, AuthorityState::Revoked);
    assert_eq!(revoked.revoked_at_ms, Some(500));
    assert_eq!(
        AuthorityRefusal::decide(repository.lookup_token(&token(1)).unwrap()),
        Err(AuthorityRefusal::Revoked)
    );
    // A second revocation keeps the first mark; an advance is refused.
    assert_eq!(
        repository
            .revoke_authority(run(1), 900)
            .unwrap()
            .revoked_at_ms,
        Some(500)
    );
    assert_eq!(
        repository
            .advance_authority(run(1), AttemptGeneration::FIRST.next(), token(2), 600)
            .unwrap_err(),
        ArtifactError::AuthorityRevoked
    );
    assert_eq!(
        repository.revoke_authority(run(2), 500).unwrap_err(),
        ArtifactError::AuthorityNotFound
    );
    // The row is still there to read — retired, not erased.
    assert_eq!(
        repository.authority(run(1)).unwrap().state,
        AuthorityState::Revoked
    );
}

#[test]
fn two_roots_hold_two_independent_authorities() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut on_a = Repository::open(&a.path().join("artifacts")).unwrap();
    on_a.issue_authority(new_authority(1, &[])).unwrap();
    let on_b = Repository::open(&b.path().join("artifacts")).unwrap();
    assert_eq!(on_b.lookup_token(&token(1)).unwrap(), None);
    assert_eq!(
        on_b.authority(run(1)).unwrap_err(),
        ArtifactError::AuthorityNotFound
    );
    // And a reopen of the same file finds the same record: the authority
    // outlives the process that issued it.
    drop(on_a);
    let again = Repository::open(&a.path().join("artifacts")).unwrap();
    assert!(again.lookup_token(&token(1)).unwrap().is_some());
}

#[test]
fn the_closed_domains_are_refused_by_the_database() {
    let mut repository = Repository::open_in_memory().unwrap();
    repository.issue_authority(new_authority(1, &[])).unwrap();
    let bad_state = repository.connection().execute(
        "UPDATE run_authorities SET state = 'paused' WHERE run_id = ?1",
        params![run(1).to_storage()],
    );
    assert!(bad_state.is_err(), "'paused' is not a state");
    let bad_generation = repository.connection().execute(
        "INSERT INTO run_authority_tokens (token_hash, run_id, attempt_generation, issued_at_ms)
         VALUES (?1, ?2, 0, 1)",
        params![token(5).as_str(), run(1).to_storage()],
    );
    assert!(bad_generation.is_err(), "generation zero was never minted");
    let orphan = repository.connection().execute(
        "INSERT INTO run_authority_grants (run_id, workspace_id) VALUES (?1, ?2)",
        params![run(7).to_storage(), workspace(1).to_storage()],
    );
    assert!(orphan.is_err(), "a grant needs its authority");
}

/// The person's surface asks which runs of *this* workspace have a folder
/// to look in (D19, D37). Newest first, another workspace's runs left out,
/// and a revoked run still listed — an agent that may no longer act still
/// left files behind.
#[test]
fn the_authorities_of_a_workspace_are_listed_newest_first_including_revoked() {
    let mut repository = Repository::open_in_memory().unwrap();
    for (n, created_at_ms) in [(1u8, 100i64), (2, 300), (3, 200)] {
        let mut authority = new_authority(n, &[]);
        authority.created_at_ms = created_at_ms;
        repository.issue_authority(authority).unwrap();
    }
    // A fourth run in a different workspace.
    let mut elsewhere = new_authority(4, &[]);
    elsewhere.workspace = workspace(0xc0);
    repository.issue_authority(elsewhere).unwrap();
    repository.revoke_authority(run(3), 400).unwrap();

    let listed = repository
        .authorities_in_workspace(workspace(0xa0))
        .unwrap();
    assert_eq!(
        listed.iter().map(|record| record.run).collect::<Vec<_>>(),
        vec![run(2), run(3), run(1)],
        "newest first"
    );
    assert_eq!(listed[1].state, AuthorityState::Revoked);
    assert_eq!(
        repository
            .authorities_in_workspace(workspace(0xc0))
            .unwrap()
            .len(),
        1
    );
    // A workspace with no runs is an empty list, never an error: the
    // surface's empty state is a state.
    assert!(
        repository
            .authorities_in_workspace(workspace(0xd0))
            .unwrap()
            .is_empty()
    );
}

/// **Deleting a run removes what authorized it and keeps that it existed.**
///
/// The record, its grants and its tokens go — there is nothing left for them
/// to authorize, and the record holds the run's working folder, which is a
/// person's own path. One row saying the run was deleted takes their place.
#[test]
fn forgetting_a_run_removes_its_authority_and_records_the_deletion() {
    let mut repository = Repository::open_in_memory().unwrap();
    repository
        .issue_authority(new_authority(1, &[workspace(0xa0)]))
        .unwrap();
    repository
        .issue_authority(new_authority(2, &[workspace(0xa0)]))
        .unwrap();

    repository.forget_authority(run(1), 900).unwrap();

    assert_eq!(
        repository.authority(run(1)).err(),
        Some(ArtifactError::AuthorityNotFound),
        "the record is gone"
    );
    assert_eq!(
        repository.lookup_token(&token(1)).unwrap(),
        None,
        "and so is the token that resolved through it"
    );
    assert_eq!(
        repository.deleted_runs(&BTreeSet::from([run(1)])).unwrap(),
        BTreeSet::from([run(1)]),
        "what stays is that it was deleted"
    );
    // The run beside it kept everything.
    assert!(repository.authority(run(2)).is_ok());
    assert!(repository.lookup_token(&token(2)).unwrap().is_some());
    assert!(
        repository
            .deleted_runs(&BTreeSet::from([run(2)]))
            .unwrap()
            .is_empty()
    );
}

/// **Idempotent, and never an error for a run this store never knew.** A run
/// that arrived by sync has no authority here and deleting it is still a
/// delete; the moment of the first deletion is the one that stands.
#[test]
fn forgetting_is_idempotent_and_works_on_a_run_with_no_authority() {
    let mut repository = Repository::open_in_memory().unwrap();
    repository.forget_authority(run(9), 100).unwrap();
    repository.forget_authority(run(9), 500).unwrap();
    assert_eq!(
        repository.deleted_runs(&BTreeSet::from([run(9)])).unwrap(),
        BTreeSet::from([run(9)])
    );
    let at: i64 = repository
        .connection()
        .query_row(
            "SELECT deleted_at_ms FROM deleted_runs WHERE run_id = ?1",
            params![run(9).to_storage()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(at, 100, "the first deletion is the one that happened");
}

/// **A run absent from the answer is simply a run.** Asked about a set the
/// caller already holds, so a store that has deleted a thousand runs does
/// not pay for nine hundred and ninety of them to draw one line.
#[test]
fn deleted_runs_answers_only_about_the_runs_it_was_asked_about() {
    let mut repository = Repository::open_in_memory().unwrap();
    repository.forget_authority(run(1), 100).unwrap();
    repository.forget_authority(run(2), 100).unwrap();
    assert_eq!(
        repository
            .deleted_runs(&BTreeSet::from([run(2), run(3)]))
            .unwrap(),
        BTreeSet::from([run(2)])
    );
    assert!(
        repository
            .deleted_runs(&BTreeSet::new())
            .unwrap()
            .is_empty()
    );
}
