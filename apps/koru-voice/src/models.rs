// SPDX-License-Identifier: MPL-2.0

//! **Choosing which installed model a request means, and the store it is asked
//! of.**
//!
//! `koru-voice-models` answers *which models are on this machine*; this module
//! answers *which one is this dictation about*, because that decision needs the
//! request's language, the engine this build hosts and the person's quality
//! tier, and none of those is the model store's business.
//!
//! Everything here runs on the inference thread. `ModelStore::installed` walks
//! directories and `ModelStore::lease` takes a cross-process lock; both are
//! *the world* in the sense [PRINCIPLES](../../../guide/contributing/PRINCIPLES.md) means it,
//! and the thread answering a person may call neither.

use std::path::Path;
use std::sync::{Arc, Mutex};

use koru_voice::{EngineId, ErrorCode, ModelId, ModelTier, Requested, VoiceError};
use koru_voice_models::{
    DiskModelStore, InstalledModel, ModelError, ModelLease, ModelStore, ModelVersion,
    choose_installed,
};

/// The product store: the channel-independent cache, and HTTPS for anything it
/// has not got.
///
/// `root` is the `--root` a rooted worker was given and is ignored when
/// `KORU_VOICE_MODELS` is set — see `ModelPaths::resolve`. The cache is
/// deliberately *not* per channel: an update package is per-channel state, a
/// model is immutable content addressed by its digest, and five channels must
/// not each hold 650 MB of identical bytes (ADR-250).
pub fn open_store(root: Option<&Path>) -> Result<Arc<dyn ModelStore>, ModelError> {
    Ok(Arc::new(DiskModelStore::open(root)?))
}

/// **Which installed model this request means.**
///
/// Two paths, and the difference is who is asking. A `Begin` that names a model
/// is a bench or a test pinning one, so the answer is that model or
/// [`ErrorCode::ModelMissing`]. A `Begin` that names none is the ordinary
/// case — a window knows about languages and has no business knowing model
/// ids — so the choice is made from the language, the tier and the engine this
/// build hosts.
///
/// **Only models for this build's engine are considered**, in both paths.
/// `engine` is inside the signature envelope precisely so verified bytes cannot
/// be routed into the wrong native parser, and offering a `whisper` model to a
/// `sherpa` build would turn that check into a failure at load time instead of
/// a choice that was never made.
pub fn resolve(
    store: &dyn ModelStore,
    named: Option<&ModelId>,
    language: Requested,
    tier: ModelTier,
    engines: &[EngineId],
) -> Result<InstalledModel, VoiceError> {
    let installed = store.installed().map_err(|error| {
        ErrorCode::ModelMissing.with(format!("the model cache could not be read: {error}"))
    })?;
    let supported: Vec<InstalledModel> = installed
        .iter()
        .filter(|model| engines.contains(&model.descriptor.engine))
        .cloned()
        .collect();
    match named {
        Some(id) => newest_of(&supported, id).ok_or_else(|| {
            ErrorCode::ModelMissing.with(format!(
                "no supported build of {id} is installed ({} model(s) in the cache)",
                installed.len()
            ))
        }),
        None => choose_installed(&supported, language, tier, None)
            .cloned()
            .ok_or_else(|| {
                ErrorCode::ModelMissing.with(format!(
                    "no installed supported {} model covers {}",
                    tier.as_str(),
                    language.tag().unwrap_or("auto")
                ))
            }),
    }
}

/// The newest installed build of one model id.
///
/// **"Newest" is the greatest version string, which is a total order and is not
/// semver** — `1.9.0` sorts above `1.10.0`. `ModelVersion` is deliberately an
/// opaque path component (`2026-03-int8` is a legal one), so giving it a grammar
/// here would be inventing one.
///
/// The model store already answers this question better: `prune` orders by the
/// `installed_at_unix` stamp in `.koru-model.json` (QUESTIONS Q-V29). That stamp
/// is on `InstalledMetadata` and **not on `InstalledModel`**, which is all
/// `ModelStore::installed` hands back, so this uses Q-V29's own deterministic
/// tie-break alone. Written up as QUESTIONS Q-V33; it is reachable only when two
/// builds of one id are installed at once *and* a caller pins the id, because
/// the ordinary path names no model and goes through [`choose_installed`].
fn newest_of(installed: &[InstalledModel], id: &ModelId) -> Option<InstalledModel> {
    installed
        .iter()
        .filter(|model| model.id() == id)
        .max_by(|a, b| a.version().as_str().cmp(b.version().as_str()))
        .cloned()
}

/// **A store that never touches a disk**, for this binary's suite and for
/// Plan 1.4's.
///
/// `ModelStore` is a trait rather than a struct for exactly this reason, and it
/// is said so in its own doc comment: a worker tested against `DiskModelStore`
/// would be a worker whose tests need a downloader, a cache root and several
/// hundred megabytes of somebody else's weights.
///
/// It refuses to install anything. Everything a test of the *worker* needs —
/// listing, finding, leasing, and refusing to remove something that is
/// leased — is real.
#[derive(Debug)]
pub struct MemoryModelStore {
    models: Vec<InstalledModel>,
    /// How many leases are outstanding, per `<id>/<version>`. A count rather
    /// than a flag, so `a_lease_is_released_by_dropping_it` can watch it fall.
    leases: Arc<Mutex<std::collections::BTreeMap<String, usize>>>,
    root: std::path::PathBuf,
}

impl MemoryModelStore {
    pub fn new(models: Vec<InstalledModel>) -> MemoryModelStore {
        MemoryModelStore {
            models,
            leases: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            // Not a directory anything reads: the fake exists so the worker can
            // be driven without one. `root()`'s contract is that every path this
            // store answers with lives under it, and every model's `root` here
            // is built from this same prefix.
            root: std::path::PathBuf::from("memory:/models"),
        }
    }

    /// How many leases are outstanding on one model build.
    pub fn leases(&self, id: &ModelId, version: &ModelVersion) -> usize {
        self.leases
            .lock()
            .unwrap()
            .get(&key(id, version))
            .copied()
            .unwrap_or(0)
    }
}

fn key(id: &ModelId, version: &ModelVersion) -> String {
    format!("{id}/{version}")
}

struct MemoryLease {
    installed: InstalledModel,
    leases: Arc<Mutex<std::collections::BTreeMap<String, usize>>>,
}

impl ModelLease for MemoryLease {
    fn installed(&self) -> &InstalledModel {
        &self.installed
    }
}

impl Drop for MemoryLease {
    /// **A lease is released by dropping it, and there is deliberately no
    /// `release()`.** So an inference thread that panics releases its lease,
    /// and so does one that returns early down a path nobody wrote a cleanup
    /// for.
    fn drop(&mut self) {
        let key = key(self.installed.id(), self.installed.version());
        let mut leases = self.leases.lock().unwrap();
        if let Some(count) = leases.get_mut(&key) {
            *count = count.saturating_sub(1);
        }
    }
}

impl ModelStore for MemoryModelStore {
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
            .find(|model| model.id() == id && model.version() == version)
            .cloned())
    }

    fn install(
        &self,
        _entry: &koru_voice_models::CatalogEntry,
        _keys: &koru_voice_models::TrustedKeys,
        _consent: &koru_voice_consent::Granted,
        _progress: &mut dyn FnMut(koru_voice_models::InstallProgress),
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<InstalledModel, ModelError> {
        Err(koru_voice_models::ModelErrorCode::Offline
            .with("an in-memory model store installs nothing"))
    }

    fn lease(
        &self,
        id: &ModelId,
        version: &ModelVersion,
    ) -> Result<Box<dyn ModelLease>, ModelError> {
        let Some(installed) = self.find(id, version)? else {
            return Err(koru_voice_models::ModelErrorCode::NotInstalled
                .with(format!("{id}/{version} is not in this store")));
        };
        *self
            .leases
            .lock()
            .unwrap()
            .entry(key(id, version))
            .or_insert(0) += 1;
        Ok(Box::new(MemoryLease {
            installed,
            leases: self.leases.clone(),
        }))
    }

    fn remove(&self, id: &ModelId, version: &ModelVersion) -> Result<(), ModelError> {
        if self.leases(id, version) > 0 {
            return Err(
                koru_voice_models::ModelErrorCode::Leased.with(format!("{id}/{version} is in use"))
            );
        }
        Ok(())
    }

    fn prune(&self, _id: &ModelId, _keep: usize) -> Result<Vec<ModelVersion>, ModelError> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests;
