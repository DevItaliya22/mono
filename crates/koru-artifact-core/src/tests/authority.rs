// SPDX-License-Identifier: MPL-2.0

//! The authority record's pure rules: the grant decision (D28), the token
//! resolution (§7.3's fencing and revocation), the redactions, and the
//! folder a registration is confined to.

use std::collections::BTreeSet;
use std::path::Path;

use super::*;

pub(crate) const PATH_SENTINEL: &str = "PATH-SENTINEL";

fn absolute(tail: &str) -> std::path::PathBuf {
    if cfg!(windows) {
        Path::new(r"C:\").join(tail)
    } else {
        Path::new("/").join(tail)
    }
}

pub(crate) fn record(grants: &[WorkspaceId]) -> RunAuthorityRecord {
    RunAuthorityRecord {
        run: run_id(3),
        workspace: WorkspaceId::from_uuid(v4(2)),
        generation: AttemptGeneration::FIRST.next(),
        grants: grants.iter().copied().collect(),
        run_workspace: RunWorkspace::new(&absolute(PATH_SENTINEL)).unwrap(),
        state: AuthorityState::Active,
        created_at_ms: 1,
        revoked_at_ms: None,
    }
}

#[test]
fn the_grant_rule_is_the_workspace_set_plus_the_runs_own_families() {
    let own_workspace = WorkspaceId::from_uuid(v4(2));
    let elsewhere = WorkspaceId::from_uuid(v4(7));
    let mine = family(); // created by run 3, in workspace 2
    let theirs = ArtifactFamily {
        id: family_id(4),
        workspace: elsewhere,
        created_by_run: Some(run_id(9)),
        ..family()
    };
    let by_hand_here = ArtifactFamily {
        id: family_id(5),
        created_by_run: None,
        ..family()
    };

    // The delegating workspace as the initial grant: everything in it.
    let granted = record(&[own_workspace]);
    assert_eq!(authorize_family(&granted, &mine), Ok(()));
    assert_eq!(authorize_family(&granted, &by_hand_here), Ok(()));
    assert_eq!(
        authorize_family(&granted, &theirs),
        Err(ArtifactError::PermissionDenied)
    );

    // An empty set denies all workspace access — except what the run
    // itself made, which it wrote the bytes of.
    let none = record(&[]);
    assert_eq!(authorize_family(&none, &mine), Ok(()));
    assert_eq!(
        authorize_family(&none, &by_hand_here),
        Err(ArtifactError::PermissionDenied)
    );
    assert_eq!(
        authorize_family(&none, &theirs),
        Err(ArtifactError::PermissionDenied)
    );

    // Another workspace granted explicitly reaches that one and no other.
    let other = record(&[elsewhere]);
    assert_eq!(authorize_family(&other, &theirs), Ok(()));
    assert_eq!(
        authorize_family(&other, &by_hand_here),
        Err(ArtifactError::PermissionDenied)
    );
}

fn lookup(
    generation: AttemptGeneration,
    retired: bool,
    state: AuthorityState,
) -> Option<TokenLookup> {
    let mut authority = record(&[]);
    authority.state = state;
    Some(TokenLookup {
        authority,
        token_generation: generation,
        retired,
    })
}

#[test]
fn a_token_resolves_to_exactly_one_of_four_answers() {
    let current = AttemptGeneration::FIRST.next();
    assert!(AuthorityRefusal::decide(lookup(current, false, AuthorityState::Active)).is_ok());
    assert_eq!(
        AuthorityRefusal::decide(None),
        Err(AuthorityRefusal::Unknown)
    );
    assert_eq!(
        AuthorityRefusal::decide(lookup(
            AttemptGeneration::FIRST,
            true,
            AuthorityState::Active
        )),
        Err(AuthorityRefusal::StaleGeneration)
    );
    // A retired token learns nothing about a later revocation: the fence
    // is checked first.
    assert_eq!(
        AuthorityRefusal::decide(lookup(
            AttemptGeneration::FIRST,
            true,
            AuthorityState::Revoked
        )),
        Err(AuthorityRefusal::StaleGeneration)
    );
    // A token whose generation is not the record's is stale even if the
    // store forgot to mark it — belt and braces.
    assert_eq!(
        AuthorityRefusal::decide(lookup(
            AttemptGeneration::FIRST,
            false,
            AuthorityState::Active
        )),
        Err(AuthorityRefusal::StaleGeneration)
    );
    assert_eq!(
        AuthorityRefusal::decide(lookup(current, false, AuthorityState::Revoked)),
        Err(AuthorityRefusal::Revoked)
    );
    for (refusal, code) in [
        (AuthorityRefusal::StaleGeneration, "stale_generation"),
        (AuthorityRefusal::Revoked, "authority_revoked"),
        (AuthorityRefusal::Unknown, "authority_unknown"),
    ] {
        assert_eq!(refusal.code(), code);
        assert_eq!(refusal.to_string(), code);
    }
}

#[test]
fn the_run_workspace_is_absolute_and_redacted_and_the_folder_is_under_it() {
    assert_eq!(
        RunWorkspace::new(Path::new("relative/folder")).unwrap_err(),
        ArtifactError::InvalidRequest
    );
    let workspace = RunWorkspace::new(&absolute(PATH_SENTINEL)).unwrap();
    let rendered = format!("{workspace:?} {:?}", record(&[]));
    assert!(!rendered.contains(PATH_SENTINEL), "{rendered}");
    assert_eq!(
        serde_json::to_value(&workspace).unwrap(),
        serde_json::Value::String(absolute(PATH_SENTINEL).to_str().unwrap().to_owned())
    );
    let folder = record(&[]).artifacts_folder();
    assert_eq!(
        folder,
        absolute(PATH_SENTINEL)
            .join(ARTIFACTS_DIR)
            .join(run_id(3).to_storage())
    );
    assert_eq!(ARTIFACTS_DIR, ".artifacts");
}

#[test]
fn a_token_hash_is_sixty_four_lowercase_hex_and_debugs_a_prefix() {
    let hash = TokenHash::from_hex("ab".repeat(32)).unwrap();
    assert_eq!(format!("{hash:?}"), "token(abababab)");
    assert_eq!(
        TokenHash::from_hex("AB".repeat(32)).unwrap_err(),
        ArtifactError::InvalidRequest
    );
    assert_eq!(
        TokenHash::from_hex("ab".repeat(31)).unwrap_err(),
        ArtifactError::InvalidRequest
    );
    assert_eq!(
        TokenHash::from_hex(format!("{}zz", "ab".repeat(31))).unwrap_err(),
        ArtifactError::InvalidRequest
    );
}

#[test]
fn the_record_round_trips_with_its_grants_as_a_sorted_set() {
    let a = WorkspaceId::from_uuid(v4(9));
    let b = WorkspaceId::from_uuid(v4(4));
    let record = record(&[a, b]);
    let json = serde_json::to_value(&record).unwrap();
    let grants: Vec<_> = json["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(grants, [b.to_storage(), a.to_storage()]);
    assert_eq!(json["state"], "active");
    assert!(json.get("revoked_at_ms").is_none());
    let back: RunAuthorityRecord = serde_json::from_value(json).unwrap();
    assert_eq!(back, record);
    assert_eq!(back.grants, BTreeSet::from([a, b]));
    for state in [AuthorityState::Active, AuthorityState::Revoked] {
        assert_eq!(AuthorityState::parse(state.as_str()), Some(state));
    }
    assert_eq!(AuthorityState::parse("paused"), None);
}
