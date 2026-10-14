// SPDX-License-Identifier: MPL-2.0

//! Read-only, gateway-independent: the folders, read out of a file
//! nothing is holding open.

use std::{collections::BTreeSet, path::Path};

use koru_artifact_core::{
    AttemptGeneration, NewAuthority, RunAuthorityRepository, TokenHash, WorkspaceId,
};
use rusqlite::Connection;
use uuid::Uuid;

use super::*;
use crate::{DATABASE_FILE, Repository};

fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

fn run(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

fn folder(name: &str) -> std::path::PathBuf {
    if cfg!(windows) {
        Path::new(r"C:\runs").join(name)
    } else {
        Path::new("/runs").join(name)
    }
}

fn authority(n: u8, name: &str) -> NewAuthority {
    NewAuthority {
        run: run(n),
        workspace: WorkspaceId::from_uuid(v4(0xa0)),
        generation: AttemptGeneration::FIRST,
        grants: BTreeSet::new(),
        run_workspace: RunWorkspace::new(&folder(name)).unwrap(),
        token_hash: TokenHash::from_hex(format!("{n:02x}").repeat(32)).unwrap(),
        created_at_ms: 100,
    }
}

#[test]
fn the_run_folders_read_with_the_gateway_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATABASE_FILE);
    let mut repository = Repository::open(dir.path()).unwrap();
    repository.issue_authority(authority(1, "alpha")).unwrap();
    repository.issue_authority(authority(2, "beta")).unwrap();
    repository.revoke_authority(run(2), 900).unwrap();
    // The gateway goes.
    drop(repository);

    let rows = run_workspaces(&path).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.run).collect::<Vec<_>>(),
        vec![run(1), run(2)]
    );
    assert_eq!(rows[0].workspace.as_path(), folder("alpha"));
    assert_eq!(rows[0].state, AuthorityState::Active);
    // A revoked authority still names its folder: the files are still
    // there, and the disk they hold is the question being asked.
    assert_eq!(rows[1].state, AuthorityState::Revoked);
}

#[test]
fn an_absent_database_is_reported_and_nothing_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DATABASE_FILE);
    assert_eq!(run_workspaces(&path), Err(InspectError::Absent));
    assert!(!path.exists(), "inspecting creates nothing");
}

#[test]
fn an_older_database_names_no_folders_and_a_newer_one_is_refused() {
    let older = tempfile::tempdir().unwrap();
    let path = older.path().join(DATABASE_FILE);
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version = 1;")
        .unwrap();
    assert_eq!(run_workspaces(&path), Ok(Vec::new()));

    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 99;")
        .unwrap();
    assert_eq!(
        run_workspaces(&path),
        Err(InspectError::SchemaTooNew {
            discovered: 99,
            supported: ARTIFACTS_SCHEMA_VERSION,
        })
    );
    let mode: String = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "delete", "the refusal changed nothing");
}
