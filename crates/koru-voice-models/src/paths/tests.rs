// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::fixtures;

fn cache() -> ModelPaths {
    ModelPaths::at(PathBuf::from("/cache"))
}

/// The rule, all three arms, with the world passed in rather than set — which
/// is the whole reason [`ModelPaths::decide`] exists beside
/// [`ModelPaths::resolve`].
#[test]
fn the_override_wins_then_the_root_then_the_channel_independent_default() {
    let overridden = ModelPaths::decide(
        Some(PathBuf::from("/elsewhere")),
        Some(Path::new("/root")),
        Some(PathBuf::from("/local")),
    )
    .unwrap();
    assert_eq!(overridden.root(), Path::new("/elsewhere"));

    let rooted = ModelPaths::decide(
        None,
        Some(Path::new("/root")),
        Some(PathBuf::from("/local")),
    )
    .unwrap();
    assert_eq!(
        rooted.root(),
        Path::new("/root").join("voice").join("models")
    );

    let default = ModelPaths::decide(None, None, Some(PathBuf::from("/local"))).unwrap();
    assert_eq!(
        default.root(),
        Path::new("/local").join("Koru").join("models"),
        "the default cache carries no channel segment — ADR-250"
    );
}

/// A store may not guess a directory. `guide/contributing/PRINCIPLES.md` and the
/// "losing content" section of AGENTS.md both say so, and the failure is a
/// value rather than a log line.
#[test]
fn with_nothing_to_go_on_it_refuses_rather_than_guessing() {
    let error = ModelPaths::decide(None, None, None).unwrap_err();
    assert_eq!(error.code, ModelErrorCode::Io);
    assert!(error.message.contains("KORU_VOICE_MODELS"));
}

/// **Every path lands under the resolved cache root** — the invariant Plan 1.2
/// names, and the one that keeps `KORU_VOICE_MODELS` from writing outside the
/// root it was pointed away from.
#[test]
fn every_resolved_path_lands_under_the_cache_root() {
    let descriptor = fixtures::descriptor();
    for paths in [
        ModelPaths::decide(
            Some(PathBuf::from("/elsewhere")),
            Some(Path::new("/root")),
            Some(PathBuf::from("/local")),
        )
        .unwrap(),
        ModelPaths::decide(
            None,
            Some(Path::new("/root")),
            Some(PathBuf::from("/local")),
        )
        .unwrap(),
        ModelPaths::decide(None, None, Some(PathBuf::from("/local"))).unwrap(),
    ] {
        let root = paths.root().to_path_buf();
        for path in [
            paths.id_dir(&descriptor.id),
            paths.version_dir(&descriptor.id, &descriptor.version),
            paths.model_dir(&descriptor),
            paths.staging_dir(),
            paths.parts_dir(),
            paths.archive_path(&descriptor.digest),
            paths.install_lock(&descriptor.digest),
            paths.leases_dir(&descriptor.digest),
        ] {
            assert!(
                path.starts_with(&root),
                "{} escapes {}",
                path.display(),
                root.display()
            );
        }
    }
}

/// The digest is the last component, because cache identity is the digest.
#[test]
fn a_model_directory_is_id_then_version_then_digest() {
    let descriptor = fixtures::descriptor();
    let dir = cache().model_dir(&descriptor);
    let tail: Vec<String> = dir
        .components()
        .rev()
        .take(3)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        tail,
        vec![
            descriptor.digest.as_str().to_string(),
            descriptor.version.as_str().to_string(),
            descriptor.id.as_str().to_string(),
        ]
    );
}

/// An id may not begin with a dot — `koru_voice::ModelId` refuses one — so the
/// four internal directories can never be shadowed by a model, and the walk
/// needs one rule rather than four names.
#[test]
fn no_model_id_can_name_an_internal_directory() {
    for name in [STAGING, PARTS, LOCKS, LEASES] {
        assert!(ModelPaths::is_internal(name));
        assert!(
            ModelId::parse(name).is_err(),
            "{name} is a legal model id, so it could shadow the store's own directory"
        );
    }
    assert!(!ModelPaths::is_internal("parakeet-tdt-v3"));
}
