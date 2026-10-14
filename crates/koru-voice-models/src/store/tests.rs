// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::descriptor::Sha256Digest;
use crate::fixtures;
use koru_voice::VoiceLanguage;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn installed(descriptor: ModelDescriptor) -> InstalledModel {
    let root = PathBuf::from("/cache").join(descriptor.path_components().join("/"));
    InstalledModel { descriptor, root }
}

/// A lease that counts itself, so the test below can assert on the thing the
/// module note promises: **a lease is released by dropping it.**
struct CountingLease {
    model: InstalledModel,
    live: Arc<AtomicUsize>,
}

impl ModelLease for CountingLease {
    fn installed(&self) -> &InstalledModel {
        &self.model
    }
}

impl Drop for CountingLease {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The store a worker can be built against before Plan 1.2 exists — which is
/// the point of the trait, and the reason this file can test the load path with
/// no disk and no downloader.
struct FakeStore {
    root: PathBuf,
    models: Vec<InstalledModel>,
    live: Arc<AtomicUsize>,
}

impl FakeStore {
    fn with(models: Vec<InstalledModel>) -> FakeStore {
        FakeStore {
            root: PathBuf::from("/cache"),
            models,
            live: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ModelStore for FakeStore {
    fn root(&self) -> &Path {
        &self.root
    }

    fn installed(&self) -> Result<Vec<InstalledModel>, ModelError> {
        Ok(self.models.clone())
    }

    fn find(
        &self,
        id: &ModelId,
        version: &ModelVersion,
    ) -> Result<Option<InstalledModel>, ModelError> {
        Ok(self
            .models
            .iter()
            .find(|m| m.id() == id && m.version() == version)
            .cloned())
    }

    fn install(
        &self,
        _entry: &crate::catalog::CatalogEntry,
        _keys: &TrustedKeys,
        _consent: &koru_voice_consent::Granted,
        _progress: &mut dyn FnMut(InstallProgress),
        _cancel: &AtomicBool,
    ) -> Result<InstalledModel, ModelError> {
        Err(ModelErrorCode::Offline.with("the fake store installs nothing"))
    }

    fn lease(
        &self,
        id: &ModelId,
        version: &ModelVersion,
    ) -> Result<Box<dyn ModelLease>, ModelError> {
        let model = self
            .find(id, version)?
            .ok_or_else(|| ModelErrorCode::NotInstalled.with(format!("{id}/{version}")))?;
        self.live.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(CountingLease {
            model,
            live: Arc::clone(&self.live),
        }))
    }

    fn remove(&self, _id: &ModelId, _version: &ModelVersion) -> Result<(), ModelError> {
        Err(ModelErrorCode::Leased.with("the fake store removes nothing"))
    }

    fn prune(&self, _id: &ModelId, _keep: usize) -> Result<Vec<ModelVersion>, ModelError> {
        Ok(Vec::new())
    }
}

/// **A name out of an archive is remote input however carefully it was
/// verified.** The signature says who published the bytes, not that every name
/// inside them is sensible, so every way out of the directory is refused rather
/// than normalised.
#[test]
fn a_file_inside_a_model_may_not_leave_it() {
    let model = installed(fixtures::descriptor());
    let good = model.file("encoder.onnx").unwrap();
    assert!(good.starts_with(&model.root));
    assert!(
        model
            .file("tokens/vocab.txt")
            .unwrap()
            .starts_with(&model.root)
    );

    for bad in [
        "",
        "..",
        "../secrets",
        "sub/../../secrets",
        "./encoder.onnx",
        "/etc/passwd",
        "C:\\Windows\\system32",
        "sub\\file",
    ] {
        let error = model.file(bad).unwrap_err();
        assert_eq!(
            error.code,
            ModelErrorCode::PathEscape,
            "{bad:?} was allowed"
        );
    }
}

/// The same rules `Catalog::choose` follows, applied to bytes that are already
/// here — because a person who is offline still has to be able to start a
/// dictation.
#[test]
fn choosing_an_installed_model_matches_the_catalog_rules() {
    let compact = installed(fixtures::descriptor());
    let accurate = installed(ModelDescriptor {
        tier: ModelTier::HighAccuracy,
        digest: Sha256Digest::parse(
            "2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap(),
        ..fixtures::descriptor()
    });
    let models = vec![compact, accurate];
    let sherpa = EngineId::parse("sherpa").unwrap();
    let whisper = EngineId::parse("whisper").unwrap();

    let chosen = choose_installed(
        &models,
        VoiceLanguage::En.into(),
        ModelTier::HighAccuracy,
        None,
    )
    .unwrap();
    assert_eq!(chosen.descriptor.tier, ModelTier::HighAccuracy);
    assert!(
        choose_installed(&models, VoiceLanguage::Ja.into(), ModelTier::Compact, None).is_none()
    );
    assert!(
        choose_installed(&models, Requested::Auto, ModelTier::Compact, Some(&sherpa)).is_some()
    );
    assert!(
        choose_installed(&models, Requested::Auto, ModelTier::Compact, Some(&whisper)).is_none()
    );
    assert!(choose_installed(&[], Requested::Auto, ModelTier::Compact, None).is_none());
}

/// `engine` is inside the signature envelope precisely so that verified bytes
/// cannot be routed into the wrong native parser — and a check nobody performs
/// is a field nobody signed. This is the check.
#[test]
fn loading_refuses_a_model_built_for_another_engine() {
    let store = FakeStore::with(vec![installed(fixtures::descriptor())]);
    let id = ModelId::parse("parakeet-tdt-v3").unwrap();
    let version = ModelVersion::parse("1.2.0").unwrap();

    let loaded =
        LoadedModel::open(&store, &id, &version, &EngineId::parse("sherpa").unwrap()).unwrap();
    assert_eq!(loaded.descriptor().id, id);
    assert!(
        loaded
            .file("encoder.onnx")
            .unwrap()
            .starts_with(loaded.root())
    );
    drop(loaded);

    let error =
        LoadedModel::open(&store, &id, &version, &EngineId::parse("whisper").unwrap()).unwrap_err();
    assert_eq!(error.code, ModelErrorCode::EngineMismatch);
}

#[test]
fn loading_a_model_that_is_not_here_says_so() {
    let store = FakeStore::with(Vec::new());
    let error = LoadedModel::open(
        &store,
        &ModelId::parse("parakeet-tdt-v3").unwrap(),
        &ModelVersion::parse("1.2.0").unwrap(),
        &EngineId::parse("sherpa").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code, ModelErrorCode::NotInstalled);
    // A worker has to answer its session in the session's own vocabulary, and
    // *not installed* is the one a download would fix.
    assert_eq!(
        error.session_error().code,
        koru_voice::ErrorCode::ModelMissing
    );
}

/// **There is deliberately no `release()`.** An inference thread that panics
/// releases its lease, and so does one that returns early down a path nobody
/// wrote a cleanup for.
#[test]
fn a_lease_is_released_by_dropping_it() {
    let store = FakeStore::with(vec![installed(fixtures::descriptor())]);
    let live = Arc::clone(&store.live);
    let id = ModelId::parse("parakeet-tdt-v3").unwrap();
    let version = ModelVersion::parse("1.2.0").unwrap();

    assert_eq!(live.load(Ordering::Acquire), 0);
    let held = store.lease(&id, &version).unwrap();
    assert_eq!(live.load(Ordering::Acquire), 1);
    assert_eq!(held.installed().version(), &version);
    drop(held);
    assert_eq!(live.load(Ordering::Acquire), 0);

    // And through the load interface, which owns the lease it took.
    let loaded =
        LoadedModel::open(&store, &id, &version, &EngineId::parse("sherpa").unwrap()).unwrap();
    assert_eq!(live.load(Ordering::Acquire), 1);
    drop(loaded);
    assert_eq!(live.load(Ordering::Acquire), 0);
}

/// A `Debug` on the load handle prints ids and never a path a person's home
/// directory is under — the logging rule, applied where a derived `Debug` would
/// have walked straight into a field.
#[test]
fn the_load_handle_debugs_as_ids() {
    let store = FakeStore::with(vec![installed(fixtures::descriptor())]);
    let loaded = LoadedModel::open(
        &store,
        &ModelId::parse("parakeet-tdt-v3").unwrap(),
        &ModelVersion::parse("1.2.0").unwrap(),
        &EngineId::parse("sherpa").unwrap(),
    )
    .unwrap();
    let printed = format!("{loaded:?}");
    assert!(printed.contains("parakeet-tdt-v3"), "{printed}");
    assert!(printed.contains("1.2.0"), "{printed}");
    assert!(!printed.contains("/cache"), "{printed}");
}
