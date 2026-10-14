// SPDX-License-Identifier: MPL-2.0

//! **The store suite.**
//!
//! Two things about it are deliberate. The archives are *built*, as real zips,
//! rather than checked in — the reason `fixtures` gives for generating a key
//! pair applies word for word to a fixture archive. And the concurrency tests
//! use two `DiskModelStore` values on two threads rather than two processes:
//! `fs2`'s locks are per open file handle on Windows and per open file
//! description on Unix, so two handles inside one process exclude each other
//! exactly as two processes do. What that cannot exercise is a *killed* holder,
//! which is why crash recovery is asserted against files written by hand in
//! `lock/tests.rs` — a lease nothing holds the lock on is precisely what a dead
//! process leaves.

use super::*;
use crate::descriptor::Sha256Digest;
use crate::fixtures::{self, Signer};
use crate::source::DirectorySource;
use crate::store::{LoadedModel, anything_installed};
use crate::tier::ModelTier;
use std::io::Write;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Barrier};
use url::Url;

/// One test's world: a cache root, a directory of archives standing in for a
/// download host, and the key that signs its catalog.
struct Scene {
    dir: tempfile::TempDir,
    signer: Signer,
}

impl Scene {
    fn new() -> Scene {
        Scene {
            dir: tempfile::tempdir().unwrap(),
            signer: Signer::new(),
        }
    }

    fn cache(&self) -> ModelPaths {
        ModelPaths::at(self.dir.path().join("cache"))
    }

    fn host(&self) -> PathBuf {
        let host = self.dir.path().join("host");
        std::fs::create_dir_all(&host).unwrap();
        host
    }

    fn store(&self) -> DiskModelStore {
        DiskModelStore::new(self.cache(), Box::new(DirectorySource::new(self.host())))
    }

    /// A zip holding one named file, published on the fixture host, and the
    /// entry that describes and signs it.
    fn publish(&self, name: &str, version: &str, contents: &[u8]) -> CatalogEntry {
        let archive = self.host().join(name);
        write_zip(&archive, contents);
        let (size, digest) = Sha256Digest::of_file(&archive).unwrap();
        let descriptor = ModelDescriptor {
            version: ModelVersion::parse(version).unwrap(),
            size,
            digest,
            ..fixtures::descriptor()
        };
        CatalogEntry {
            signature: self.signer.sign(&descriptor),
            descriptor,
            url: Url::parse(&format!("https://models.koru.do/v1/{name}")).unwrap(),
            mirrors: Vec::new(),
        }
    }
}

/// A one-entry zip. `koru_update::extract_zip` is what reads it, so what is
/// written here is exactly what the product path meets.
fn write_zip(path: &Path, contents: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file::<_, ()>("model.onnx", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(contents).unwrap();
    zip.finish().unwrap();
}

/// **The consent this entry was offered under.**
///
/// Every install in this suite carries one, because the signature says it
/// must: what these tests are about is the store, and a suite that could not
/// reach `install` without writing a consent file to a disk would be a suite
/// asserting on `koru-voice-consent` by accident. `Granted::recorded` is the
/// in-memory grant the consent step itself holds between the button and the
/// write, so this is the product's own path rather than a test seam.
fn consented(entry: &CatalogEntry) -> koru_voice_consent::Granted {
    koru_voice_consent::Granted::recorded(entry.terms(), 1)
}

fn install(
    store: &DiskModelStore,
    entry: &CatalogEntry,
    keys: &TrustedKeys,
) -> Result<InstalledModel, ModelError> {
    let cancel = AtomicBool::new(false);
    store.install(entry, keys, &consented(entry), &mut |_| {}, &cancel)
}

/// **The order is normative**: the signature over the descriptor is checked
/// first, before a byte is fetched. A source that was never asked is what says
/// so — asserting on the error code alone would pass even if the whole archive
/// had been downloaded first.
#[test]
fn a_forged_descriptor_is_refused_before_anything_is_fetched() {
    let scene = Scene::new();
    let mut entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    // A real signature, over a different descriptor. The case a digest check
    // alone could never catch.
    entry.signature = scene
        .signer
        .sign_bytes(b"koru-model-descriptor/1\nid\tsomething-else\n");
    let counted = Arc::new(AtomicUsize::new(0));
    let store = DiskModelStore::new(
        scene.cache(),
        Box::new(CountingSource {
            inner: DirectorySource::new(scene.host()),
            calls: counted.clone(),
        }),
    );

    let error = install(&store, &entry, &scene.signer.trusted).unwrap_err();

    assert_eq!(error.code, ModelErrorCode::SignatureInvalid);
    assert_eq!(counted.load(Ordering::Acquire), 0, "it fetched anyway");
    assert!(store.installed().unwrap().is_empty());
}

/// **A build that pins no key refuses every install**, which is the honest
/// state of the tree until a key is minted — `MODEL_SIGNING_KEYS` is empty and
/// Q-V14 records why that is correct rather than unfinished.
#[test]
fn a_build_with_no_trust_root_installs_nothing() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();

    let error = install(&store, &entry, TrustedKeys::pinned()).unwrap_err();

    assert_eq!(error.code, ModelErrorCode::NoTrustRoot);
    assert!(store.installed().unwrap().is_empty());
}

/// A whole install, and then the thing it exists for: **loading it needs no
/// network.** The second store links a source that refuses every URL, and the
/// descriptor it hands back is the *signed* one, read off the disk.
#[test]
fn an_installed_model_loads_with_the_network_disabled() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.2.0.zip", "1.2.0", b"weights");
    install(&scene.store(), &entry, &scene.signer.trusted).unwrap();

    let offline = DiskModelStore::offline(scene.cache());
    let found = offline
        .find(entry.id(), entry.version())
        .unwrap()
        .expect("installed");
    assert_eq!(found.descriptor, entry.descriptor);
    assert_eq!(
        found.root,
        scene.cache().model_dir(&entry.descriptor),
        "cache identity is the digest"
    );

    let loaded = LoadedModel::open(&offline, entry.id(), entry.version(), entry.engine()).unwrap();
    assert_eq!(
        std::fs::read(loaded.file("model.onnx").unwrap()).unwrap(),
        b"weights"
    );
    // Nothing above may have asked the source for anything; the source refuses
    // everything, so an install attempt is the proof.
    let error = install(&offline, &entry, &scene.signer.trusted);
    assert!(
        error.is_ok(),
        "an install of what is already here is not a fetch"
    );
}

/// **A wrong digest never publishes, and leaves nothing a later read mistakes
/// for installed** — including the archive, which is deleted so that a retry is
/// a retry rather than the same failure for ever.
#[test]
fn a_wrong_digest_publishes_nothing_and_keeps_no_archive() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    // The catalog is signed, and the host serves other bytes — a mirror doing
    // the one thing the ADR says a mirror must not be able to get away with.
    write_zip(&scene.host().join("model-1.0.0.zip"), b"different weights");
    let store = scene.store();

    let error = install(&store, &entry, &scene.signer.trusted).unwrap_err();

    assert!(
        matches!(
            error.code,
            ModelErrorCode::DigestMismatch | ModelErrorCode::SizeMismatch
        ),
        "{error}"
    );
    assert!(store.installed().unwrap().is_empty());
    assert!(store.find(entry.id(), entry.version()).unwrap().is_none());
    assert!(
        !scene.cache().model_dir(&entry.descriptor).exists(),
        "a directory was left where a reader would find it"
    );
    assert!(
        !scene
            .cache()
            .archive_path(&entry.descriptor.digest)
            .exists(),
        "the wrong bytes were kept, so every retry fails identically for ever"
    );
    // Nothing is left staging either.
    assert_eq!(
        std::fs::read_dir(scene.cache().staging_dir())
            .map(|entries| entries.count())
            .unwrap_or(0),
        0
    );
}

/// **Two processes installing the same digest produce one install.** The second
/// waits on the per-digest lock and then finds the work done — so the source is
/// asked exactly once and there is exactly one directory.
#[test]
fn two_installers_of_one_digest_produce_one_install() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let calls = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let keys = scene.signer.trusted.clone();

    let outcomes: Vec<InstalledModel> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let store = DiskModelStore::new(
                    scene.cache(),
                    Box::new(CountingSource {
                        inner: DirectorySource::new(scene.host()),
                        calls: calls.clone(),
                    }),
                );
                let entry = entry.clone();
                let keys = keys.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    install(&store, &entry, &keys).unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    assert_eq!(outcomes[0].root, outcomes[1].root);
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the same bytes were fetched twice"
    );
    assert_eq!(scene.store().installed().unwrap().len(), 1);
}

/// Meeting the same `id`/`version` with different bytes is a **hard failure**,
/// never a silent replace and never a second directory shadowing the first
/// (ADR-250).
#[test]
fn a_second_digest_under_one_version_is_an_identity_conflict() {
    let scene = Scene::new();
    let first = scene.publish("model-a.zip", "1.0.0", b"weights");
    install(&scene.store(), &first, &scene.signer.trusted).unwrap();
    let rebuilt = scene.publish("model-b.zip", "1.0.0", b"rebuilt weights");
    assert_ne!(first.descriptor.digest, rebuilt.descriptor.digest);

    let error = install(&scene.store(), &rebuilt, &scene.signer.trusted).unwrap_err();

    assert_eq!(error.code, ModelErrorCode::IdentityConflict);
    let store = scene.store();
    assert_eq!(store.installed().unwrap().len(), 1);
    assert_eq!(
        store
            .find(first.id(), first.version())
            .unwrap()
            .unwrap()
            .descriptor
            .digest,
        first.descriptor.digest,
        "the install that was already here was disturbed"
    );
}

/// Installing what is already installed is the same install, not a second
/// download — the idempotence `ModelStore::install` promises.
#[test]
fn installing_what_is_already_here_fetches_nothing() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let calls = Arc::new(AtomicUsize::new(0));
    let store = DiskModelStore::new(
        scene.cache(),
        Box::new(CountingSource {
            inner: DirectorySource::new(scene.host()),
            calls: calls.clone(),
        }),
    );

    let first = install(&store, &entry, &scene.signer.trusted).unwrap();
    let second = install(&store, &entry, &scene.signer.trusted).unwrap();

    assert_eq!(first.root, second.root);
    assert_eq!(calls.load(Ordering::Acquire), 1);
}

/// **`remove` refuses a leased model.** An instruction must not silently do
/// nothing, so this is an error rather than a no-op — and the lease is a
/// cross-process claim, which is the only kind that means anything in a cache
/// two channels share.
#[test]
fn remove_refuses_a_leased_model_and_takes_it_afterwards() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    install(&store, &entry, &scene.signer.trusted).unwrap();

    let lease = store.lease(entry.id(), entry.version()).unwrap();
    let error = store.remove(entry.id(), entry.version()).unwrap_err();
    assert_eq!(error.code, ModelErrorCode::Leased);
    assert!(store.find(entry.id(), entry.version()).unwrap().is_some());

    drop(lease);
    store.remove(entry.id(), entry.version()).unwrap();
    assert!(store.find(entry.id(), entry.version()).unwrap().is_none());
    assert!(store.installed().unwrap().is_empty());
    assert!(
        !scene.cache().model_dir(&entry.descriptor).exists(),
        "the bytes are still there"
    );
}

/// Removing something that is not here is a refusal rather than a silent
/// success — the difference `ModelStore::find` draws between a question and a
/// demand.
#[test]
fn removing_what_is_not_here_says_so() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let error = scene
        .store()
        .remove(entry.id(), entry.version())
        .unwrap_err();
    assert_eq!(error.code, ModelErrorCode::NotInstalled);
}

/// **`prune` keeps a leased model and is not an error.** That is the whole
/// difference from `remove`: housekeeping must not fail because somebody is
/// dictating.
#[test]
fn prune_keeps_the_newest_and_never_deletes_a_leased_build() {
    let scene = Scene::new();
    let mut entries = Vec::new();
    for (n, version) in ["1.0.0", "1.1.0", "1.2.0"].iter().enumerate() {
        let entry = scene.publish(
            &format!("model-{version}.zip"),
            version,
            format!("weights {n}").as_bytes(),
        );
        install(&scene.store(), &entry, &scene.signer.trusted).unwrap();
        // The install order is the order `prune` reads, and its stamp has
        // one-second resolution — so the ordering is made explicit rather than
        // being left to how fast the machine is.
        stamp(&scene.cache(), &entry.descriptor, 1_000 + n as u64);
        entries.push(entry);
    }
    let store = scene.store();
    assert_eq!(store.installed().unwrap().len(), 3);

    // The oldest is held open by another worker.
    let lease = store.lease(entries[0].id(), entries[0].version()).unwrap();
    let deleted = store.prune(entries[0].id(), 2).unwrap();
    assert!(deleted.is_empty(), "a leased build was deleted");
    assert_eq!(store.installed().unwrap().len(), 3);

    drop(lease);
    let deleted = store.prune(entries[0].id(), 2).unwrap();
    assert_eq!(deleted, vec![entries[0].descriptor.version.clone()]);
    let left: Vec<String> = store
        .installed()
        .unwrap()
        .iter()
        .map(|m| m.version().as_str().to_string())
        .collect();
    assert_eq!(left, vec!["1.1.0".to_string(), "1.2.0".to_string()]);
}

/// Pruning a model with fewer builds than `keep` deletes nothing and is not an
/// error — the case a housekeeping pass hits on almost every run.
#[test]
fn prune_below_the_threshold_does_nothing() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    install(&store, &entry, &scene.signer.trusted).unwrap();

    assert!(store.prune(entry.id(), 2).unwrap().is_empty());
    assert_eq!(store.installed().unwrap().len(), 1);
}

/// A cancelled install publishes nothing. Cancellation is not a failure, and it
/// has its own code so a caller can tell it from one.
#[test]
fn a_cancelled_install_publishes_nothing() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    let cancel = AtomicBool::new(true);

    let error = store
        .install(
            &entry,
            &scene.signer.trusted,
            &consented(&entry),
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();

    assert_eq!(error.code, ModelErrorCode::Cancelled);
    assert!(store.installed().unwrap().is_empty());
    assert!(!scene.cache().model_dir(&entry.descriptor).exists());
}

/// The metadata file is what says *installed*, and it has to agree with the
/// three directory names it was found under. One moved into another model's
/// directory names a different model and is not an install.
#[test]
fn metadata_that_names_another_model_is_not_an_install() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    let installed = install(&store, &entry, &scene.signer.trusted).unwrap();

    let path = installed.root.join(METADATA_FILE);
    let mut metadata: InstalledMetadata =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    metadata.descriptor.version = ModelVersion::parse("9.9.9").unwrap();
    std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();

    assert!(store.find(entry.id(), entry.version()).unwrap().is_none());
    assert!(store.installed().unwrap().is_empty());
}

/// A directory with no metadata at all — a half-extracted model from a build
/// that did not publish atomically, or a person's stray folder — is not an
/// install either.
#[test]
fn a_directory_with_no_metadata_is_not_an_install() {
    let scene = Scene::new();
    let descriptor = fixtures::descriptor();
    let dir = scene.cache().model_dir(&descriptor);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), b"half a model").unwrap();

    let store = scene.store();
    assert!(store.installed().unwrap().is_empty());
    assert!(
        store
            .find(&descriptor.id, &descriptor.version)
            .unwrap()
            .is_none()
    );
}

/// **Every path the store answers with lands under its own root**, whichever
/// of the three rules resolved it — the invariant that keeps
/// `KORU_VOICE_MODELS` from writing outside the root it was pointed away from.
#[test]
fn every_path_the_store_answers_lands_under_its_root() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let rooted = scene.dir.path().join("root");
    for paths in [
        // `KORU_VOICE_MODELS`, which deliberately points outside the root it
        // overrides — and must still contain everything the store answers.
        ModelPaths::decide(
            Some(scene.dir.path().join("overridden")),
            Some(scene.dir.path()),
            None,
        )
        .unwrap(),
        ModelPaths::decide(None, Some(rooted.as_path()), None).unwrap(),
        ModelPaths::decide(None, None, Some(scene.dir.path().join("local"))).unwrap(),
    ] {
        let store = DiskModelStore::new(paths, Box::new(DirectorySource::new(scene.host())));
        let root = store.root().to_path_buf();
        let installed = install(&store, &entry, &scene.signer.trusted).unwrap();
        assert!(installed.root.starts_with(&root), "{:?}", installed.root);
        assert!(installed.file("model.onnx").unwrap().starts_with(&root));
        for model in store.installed().unwrap() {
            assert!(model.root.starts_with(&root));
        }
    }
}

/// The store sweeps staging debris before it stages anything of its own, so a
/// process killed mid-install does not leave a directory that grows for ever.
#[test]
fn an_install_sweeps_debris_a_dead_installer_left() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let staging = scene.cache().staging_dir();
    std::fs::create_dir_all(staging.join("dead-1-1")).unwrap();
    std::fs::write(staging.join("dead-1-1").join("half"), b"abandoned").unwrap();
    std::fs::write(staging.join("dead-1-1.busy"), b"").unwrap();

    install(&scene.store(), &entry, &scene.signer.trusted).unwrap();

    assert!(!staging.join("dead-1-1").exists());
    assert_eq!(
        std::fs::read_dir(&staging).unwrap().count(),
        0,
        "a finished install left staging behind"
    );
}

/// An archive that is not a zip is refused with a message that says so, rather
/// than being published as a directory containing nothing. The descriptor
/// carries no `format` field — Q-V27.
#[test]
fn an_archive_that_is_not_a_zip_is_refused() {
    let scene = Scene::new();
    std::fs::write(scene.host().join("model-1.0.0.tar.gz"), b"not a zip at all").unwrap();
    let archive = scene.host().join("model-1.0.0.tar.gz");
    let (size, digest) = Sha256Digest::of_file(&archive).unwrap();
    let descriptor = ModelDescriptor {
        size,
        digest,
        ..fixtures::descriptor()
    };
    let entry = CatalogEntry {
        signature: scene.signer.sign(&descriptor),
        descriptor,
        url: Url::parse("https://models.koru.do/v1/model-1.0.0.tar.gz").unwrap(),
        mirrors: Vec::new(),
    };
    let store = scene.store();

    let error = install(&store, &entry, &scene.signer.trusted).unwrap_err();

    assert_eq!(error.code, ModelErrorCode::InstallFailed);
    assert!(error.message.contains("zip"), "{}", error.message);
    assert!(store.installed().unwrap().is_empty());
}

/// The install stages arrive in the order the enum declares them, which is what
/// makes a progress surface possible at all.
#[test]
fn the_stages_arrive_in_order() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    let mut stages = Vec::new();
    let cancel = AtomicBool::new(false);

    store
        .install(
            &entry,
            &scene.signer.trusted,
            &consented(&entry),
            &mut |p| stages.push(p.stage),
            &cancel,
        )
        .unwrap();

    stages.dedup();
    assert_eq!(
        stages,
        vec![
            InstallStage::Resolving,
            InstallStage::Downloading,
            InstallStage::Verifying,
            InstallStage::Unpacking,
            InstallStage::Publishing,
        ]
    );
}

/// `choose_installed` over a real store, so the offline path a person who is on
/// a train actually walks is exercised end to end rather than against a fake.
#[test]
fn choosing_offline_picks_from_what_is_on_the_disk() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    install(&scene.store(), &entry, &scene.signer.trusted).unwrap();

    let offline = DiskModelStore::offline(scene.cache());
    let models = offline.installed().unwrap();
    let chosen = crate::store::choose_installed(
        &models,
        koru_voice::Requested::Fixed(koru_voice::VoiceLanguage::En),
        ModelTier::Compact,
        Some(entry.engine()),
    )
    .expect("the model that is here");
    assert_eq!(chosen.descriptor.digest, entry.descriptor.digest);
}

/// Rewrite an installed model's stamp, so `prune`'s ordering is a fact the test
/// states rather than a race against a one-second clock.
fn stamp(paths: &ModelPaths, descriptor: &ModelDescriptor, at: u64) {
    let path = paths.model_dir(descriptor).join(METADATA_FILE);
    let mut metadata: InstalledMetadata =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    metadata.installed_at_unix = at;
    std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
}

/// A source that counts how many times it was asked. What several tests here
/// assert on is *that it was not asked*, which no error code can prove.
struct CountingSource {
    inner: DirectorySource,
    calls: Arc<AtomicUsize>,
}

impl ModelSource for CountingSource {
    fn fetch(
        &self,
        url: &url::Url,
        descriptor: &ModelDescriptor,
        destination: &Path,
        progress: &mut dyn FnMut(InstallProgress),
        cancel: &AtomicBool,
    ) -> Result<(), ModelError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.inner
            .fetch(url, descriptor, destination, progress, cancel)
    }
}

// ------------------------------------------------------------------ consent

/// **No path reaches a download without consent**, and the proof is that the
/// source was never asked.
///
/// The first half of the gate is the signature — a caller with no
/// `Granted` cannot call `install` at all, and that is a compile error rather
/// than a test. This is the second half: a token that exists and does not
/// cover this offer.
#[test]
fn a_grant_that_does_not_cover_the_offer_fetches_nothing() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let counted = Arc::new(AtomicUsize::new(0));
    let store = DiskModelStore::new(
        scene.cache(),
        Box::new(CountingSource {
            inner: DirectorySource::new(scene.host()),
            calls: counted.clone(),
        }),
    );
    // Agreed to a different licence. Everything else about the offer matches.
    let elsewhere = koru_voice_consent::Granted::recorded(
        koru_voice_consent::Terms {
            licence: "CC-BY-NC-4.0".into(),
            ..entry.terms()
        },
        1,
    );
    let cancel = AtomicBool::new(false);

    let error = store
        .install(
            &entry,
            &scene.signer.trusted,
            &elsewhere,
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();

    assert_eq!(error.code, ModelErrorCode::ConsentRequired);
    assert_eq!(counted.load(Ordering::Acquire), 0, "it fetched anyway");
    assert!(!anything_installed(&store));
}

/// A download somebody agreed to and then stopped leaves nothing any later
/// reader calls installed — which is the question a catalog surface actually
/// asks, rather than the directory listing underneath it.
#[test]
fn a_cancelled_download_leaves_nothing_reported_as_installed() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    let cancel = AtomicBool::new(true);

    let error = store
        .install(
            &entry,
            &scene.signer.trusted,
            &consented(&entry),
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();

    assert_eq!(error.code, ModelErrorCode::Cancelled);
    assert!(!anything_installed(&store));
    // And a second reader, opened fresh on the same cache, agrees — the answer
    // is the disk's rather than this store's memory of what it did.
    assert!(!anything_installed(&DiskModelStore::offline(scene.cache())));
}

/// **Installed state is derived, and this is what that buys.**
///
/// A model is installed with consent recorded beside it; the cache is then
/// emptied the way another channel's `prune` or a person with a file manager
/// would empty it. The store answers *not installed* — and the consent record
/// is byte-for-byte what it was, because nothing about it describes the disk.
#[test]
fn an_emptied_cache_reports_not_installed_and_changes_no_stored_flag() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    let record = koru_voice_consent::path_in(scene.dir.path());
    koru_voice_consent::grant(&record, entry.terms(), 1_700_000_000).unwrap();

    install(&store, &entry, &scene.signer.trusted).unwrap();
    assert!(anything_installed(&store));
    let before = std::fs::read(&record).unwrap();

    std::fs::remove_dir_all(scene.cache().root()).unwrap();

    assert!(
        !anything_installed(&DiskModelStore::offline(scene.cache())),
        "an emptied cache holds no model"
    );
    assert_eq!(
        std::fs::read(&record).unwrap(),
        before,
        "nothing about deleting a model touches what somebody agreed to"
    );
    assert!(
        koru_voice_consent::read(&record).is_granted(),
        "deleting the model does not revoke consent"
    );
}

/// The other direction, and the reason the two facts are two files: taking
/// consent back leaves the bytes exactly where they were. Somebody who changes
/// their mind about dictating has not asked for 600 MB to be thrown away.
#[test]
fn revoking_consent_deletes_no_model() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = scene.store();
    let record = koru_voice_consent::path_in(scene.dir.path());
    koru_voice_consent::grant(&record, entry.terms(), 1).unwrap();
    install(&store, &entry, &scene.signer.trusted).unwrap();

    koru_voice_consent::revoke(&record).unwrap();

    assert!(!koru_voice_consent::read(&record).is_granted());
    assert!(anything_installed(&store), "the bytes are still here");
}

// ------------------------------------------------------------ trust policy

/// **The MVP path, and the whole reason `TrustPolicy` has a second arm.**
///
/// `MODEL_SIGNING_KEYS` is empty and staying empty until before public release,
/// so a build that could only verify could not install anything at all — and
/// the consent step, the progress toast and the download would be unbuildable
/// and untestable. Under `DigestOnly` the *signature* is skipped and nothing
/// else is: the entry here carries one that could never verify, and the bytes
/// still have to hash to the signed digest.
#[test]
fn a_digest_only_build_installs_a_model_nothing_signed() {
    let scene = Scene::new();
    let mut entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    entry.signature = ModelSignature::parse("not a signature at all").unwrap();
    let store = DiskModelStore::with_trust(
        scene.cache(),
        Box::new(DirectorySource::new(scene.host())),
        TrustPolicy::digest_only_on(Some("beta")).unwrap(),
    );
    let cancel = AtomicBool::new(false);

    let installed = store
        .install(
            &entry,
            TrustedKeys::pinned(),
            &consented(&entry),
            &mut |_| {},
            &cancel,
        )
        .expect("the MVP path installs");

    assert_eq!(installed.descriptor, entry.descriptor);
    assert!(anything_installed(&store));
}

/// **And the digest is still the digest.** Skipping the signature is not
/// skipping verification: bytes that are not the ones the catalog described are
/// refused under this policy exactly as they are under the other.
#[test]
fn a_digest_only_build_still_refuses_the_wrong_bytes() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    // Replace the published archive with different bytes, leaving the entry's
    // signed size and digest describing what used to be there.
    write_zip(&scene.host().join("model-1.0.0.zip"), b"something else");
    let store = DiskModelStore::with_trust(
        scene.cache(),
        Box::new(DirectorySource::new(scene.host())),
        TrustPolicy::digest_only_on(Some("nightly")).unwrap(),
    );
    let cancel = AtomicBool::new(false);

    let error = store
        .install(
            &entry,
            TrustedKeys::pinned(),
            &consented(&entry),
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();

    assert!(
        matches!(
            error.code,
            ModelErrorCode::DigestMismatch | ModelErrorCode::SizeMismatch
        ),
        "{error}"
    );
    assert!(!anything_installed(&store));
}

/// **`Signed` is unchanged and still refuses.** The default store, on the same
/// entry the MVP path just installed, answers `no_trust_root` — every
/// signature test in this crate keeps meaning what it meant.
#[test]
fn the_signed_policy_still_refuses_what_digest_only_accepts() {
    let scene = Scene::new();
    let mut entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    entry.signature = ModelSignature::parse("not a signature at all").unwrap();

    let error = install(&scene.store(), &entry, TrustedKeys::pinned()).unwrap_err();

    assert_eq!(error.code, ModelErrorCode::NoTrustRoot);
    assert!(!anything_installed(&scene.store()));
}

/// A descriptor that breaks its own rules is refused under `DigestOnly` too.
/// The signature is what is skipped; the field rules are not, and a policy that
/// dropped those would not be digest-only, it would be nothing.
#[test]
fn a_digest_only_build_still_validates_the_descriptor() {
    let scene = Scene::new();
    let mut entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    entry.descriptor.languages.clear();
    let store = DiskModelStore::with_trust(
        scene.cache(),
        Box::new(DirectorySource::new(scene.host())),
        TrustPolicy::digest_only_on(Some("beta")).unwrap(),
    );
    let cancel = AtomicBool::new(false);

    let error = store
        .install(
            &entry,
            TrustedKeys::pinned(),
            &consented(&entry),
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();

    assert_eq!(error.code, ModelErrorCode::DescriptorInvalid);
    assert!(!anything_installed(&store));
}

/// Consent is still the first gate. A build that skips the signature does not
/// skip the question — the two are independent, and the order is what a person
/// is told about first.
#[test]
fn a_digest_only_build_still_needs_consent() {
    let scene = Scene::new();
    let entry = scene.publish("model-1.0.0.zip", "1.0.0", b"weights");
    let store = DiskModelStore::with_trust(
        scene.cache(),
        Box::new(DirectorySource::new(scene.host())),
        TrustPolicy::digest_only_on(Some("beta")).unwrap(),
    );
    let elsewhere = koru_voice_consent::Granted::recorded(
        koru_voice_consent::Terms {
            licence: "CC-BY-NC-4.0".into(),
            ..entry.terms()
        },
        1,
    );
    let cancel = AtomicBool::new(false);

    let error = store
        .install(
            &entry,
            TrustedKeys::pinned(),
            &elsewhere,
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();

    assert_eq!(error.code, ModelErrorCode::ConsentRequired);
}
