// SPDX-License-Identifier: MPL-2.0

//! Standing requirement 7, structurally: no title or filename in any
//! `Debug` rendering or error message — while both still serialize, because
//! redaction is a property of `Debug`, not of the value.

use super::*;

fn assert_redacted(rendered: &str, what: &str) {
    for sentinel in [
        TITLE_SENTINEL,
        FILENAME_SENTINEL,
        super::authority::PATH_SENTINEL,
    ] {
        assert!(
            !rendered.contains(sentinel),
            "{what} leaked {sentinel:?}: {rendered}"
        );
    }
}

#[test]
fn debug_of_everything_that_carries_text_redacts_it() {
    let family = family();
    let version = version(2, Some(version_id(1)));
    let provenance = Provenance {
        subject: version.clone(),
        ancestors: vec![self::version(1, None)],
        runs: vec![RunLink {
            run: run_id(3),
            version: version.id,
            role: LinkRole::Output,
            linked_at_ms: 1,
        }],
        deleted_runs: Default::default(),
    };
    let new_family = NewFamily {
        id: family.id,
        workspace: family.workspace,
        title: family.title.clone(),
        created_by_run: None,
        created_at_ms: 1,
    };
    let new_version = NewVersion {
        registration_id: version.registration_id,
        family: version.family,
        blob: version.blob.clone(),
        size_bytes: 1,
        mime: version.mime.clone(),
        filename: version.filename.clone(),
        produced_by: None,
        derived_from: None,
        created_at_ms: 1,
    };
    let renderings = [
        ("family", format!("{family:?}")),
        ("family (pretty)", format!("{family:#?}")),
        ("version", format!("{version:?}")),
        ("version (pretty)", format!("{version:#?}")),
        ("provenance", format!("{provenance:?}")),
        ("new family", format!("{new_family:?}")),
        ("new version", format!("{new_version:?}")),
        (
            "registered",
            format!(
                "{:?}",
                Registered {
                    version: version.clone(),
                    retried: true
                }
            ),
        ),
        ("authority", format!("{:?}", super::authority::record(&[]))),
        (
            "authority (pretty)",
            format!("{:#?}", super::authority::record(&[])),
        ),
        (
            "token lookup",
            format!(
                "{:?}",
                TokenLookup {
                    authority: super::authority::record(&[]),
                    token_generation: AttemptGeneration::FIRST,
                    retired: false,
                }
            ),
        ),
        (
            "new authority",
            format!(
                "{:?}",
                NewAuthority {
                    run: run_id(3),
                    workspace: WorkspaceId::from_uuid(v4(2)),
                    generation: AttemptGeneration::FIRST,
                    grants: Default::default(),
                    run_workspace: super::authority::record(&[]).run_workspace,
                    token_hash: TokenHash::from_hex("cd".repeat(32)).unwrap(),
                    created_at_ms: 1,
                }
            ),
        ),
    ];
    for (what, rendered) in renderings {
        assert_redacted(&rendered, what);
    }
}

#[test]
fn a_refused_text_never_echoes_the_text() {
    let long = format!("{TITLE_SENTINEL}{}", "x".repeat(MAX_FILENAME_CHARS));
    let error = Filename::new(&long).unwrap_err();
    assert_redacted(&format!("{error:?} {error}"), "filename error");
    let error = ArtifactTitle::new(format!("{TITLE_SENTINEL}\u{0}")).unwrap_err();
    assert_redacted(&format!("{error:?} {error}"), "title error");
}

#[test]
fn no_error_names_a_principal_or_a_family() {
    let rendered = format!(
        "{:?} {}",
        ArtifactError::PermissionDenied,
        ArtifactError::PermissionDenied
    );
    assert_eq!(rendered, "PermissionDenied permission_denied");
}
