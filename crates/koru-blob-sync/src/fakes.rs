// SPDX-License-Identifier: MPL-2.0

//! Fake Store, object, control and policy ports.
//!
//! Behind `test-support` as well as `cfg(test)`, because the integration
//! tests beside this crate are a separate compilation and would otherwise
//! each grow their own half-copy — which is how two doubles come to disagree
//! about the thing they are both standing in for.
//!
//! **Fake Store, object and policy ports** (Plan B3), which is what lets the
//! engine's decisions be tested without a disk, a network or a Store. The
//! *wire* is not faked here: `tests/sequences.rs` drives S10 and S11 over HTTP
//! against an object service whose every admission is the shared contract's
//! and which `tests/object_service.rs` holds to the `cloud-blob-object-v1`
//! vectors; the hosted service, which is not in this repository, runs the
//! same suites against its real object worker and drives its real control
//! routes against a real database. A fake that answered the wire on its own say-so would only ever
//! reproduce this crate's assumptions about it.

use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use koru_account_core::workspace_authorization::Region;
use koru_cloud_blob::{
    BlobCode,
    wire::{AllocateRequestV1, AllocationV1, BlobStatusV1, ReadCapabilityV1},
};
use koru_device::cloud::CloudId;
use koru_store_core::{DocumentId, WorkspaceId, errors::Code, model::BlobHash};

use crate::{
    allowlist::{AdmittedUrl, ByteOrigins},
    control::BlobControlPlane,
    coverage::{DocumentSample, DocumentState, SampledDocument},
    object::BlobObjectPlane,
    ports::{
        BlobDownloads, BlobPolicy, BlobPorts, BlobStorePort, CloudAccess, CloudAccessFacts,
        LocalBlob, PortFuture, TransferError, TransferHold,
    },
    referenced::{WalkDocument, WalkNode},
};

pub fn hash_of(bytes: &[u8]) -> BlobHash {
    BlobHash::parse(&format!("b3:{}", blake3::hash(bytes).to_hex())).expect("a real address")
}

/// Every request any fake answered, so "performs no network call" is a
/// measurement rather than a claim about intent.
#[derive(Default)]
pub struct Calls {
    pub control: AtomicU64,
    pub bytes: AtomicU64,
    pub walks: AtomicU64,
}

impl Calls {
    pub fn total(&self) -> u64 {
        self.control.load(Ordering::Relaxed) + self.bytes.load(Ordering::Relaxed)
    }
}

// ------------------------------------------------------------------- store

#[derive(Default)]
struct StoreState {
    documents: BTreeMap<WorkspaceId, Vec<SampledDocument>>,
    /// What the next reads answer, in order. `None` in a slot means *the
    /// durable set*; an exhausted queue is the durable set too.
    ///
    /// **How a test arms the race coverage's fourth step is about** rather than
    /// assuming it: the boundary re-read sees a Workspace that moved between
    /// the sample and the commit, which is the only way to prove the commit
    /// was actually conditional on it. A queue rather than a single slot,
    /// because the read that has to move is the *second* one.
    armed_samples: BTreeMap<WorkspaceId, VecDeque<Option<Vec<SampledDocument>>>>,
    local: BTreeMap<(WorkspaceId, String), (LocalBlob, Vec<u8>)>,
    /// Bytes this device holds but which no Workspace is associated with. A
    /// download installs into here first and associates second, so a test can
    /// see that the two are not the same step.
    objects: BTreeMap<String, Vec<u8>>,
    rotten: Vec<String>,
    installs: Vec<(WorkspaceId, String)>,
}

pub struct FakeStore {
    state: Mutex<StoreState>,
    staging: PathBuf,
    calls: Arc<Calls>,
}

impl FakeStore {
    pub fn new(staging: PathBuf, calls: Arc<Calls>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(StoreState::default()),
            staging,
            calls,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoreState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// One durable document referencing these blobs, at a fresh state.
    pub fn document(&self, workspace: WorkspaceId, blobs: &[(&BlobHash, &str)]) -> DocumentId {
        let document = DocumentId::new();
        self.document_at(workspace, document, &state_of(1), blobs);
        document
    }

    /// The same, at an exact document state — what a coverage test needs, so
    /// "this document moved" and "this document did not" are two values rather
    /// than two runs.
    pub fn document_at(
        &self,
        workspace: WorkspaceId,
        document: DocumentId,
        state: &DocumentState,
        blobs: &[(&BlobHash, &str)],
    ) {
        let sampled = SampledDocument {
            document,
            state: Some(state.clone()),
            content: WalkDocument {
                durable: true,
                trashed: false,
                nodes: blobs
                    .iter()
                    .map(|(hash, mime)| WalkNode {
                        blob: Some(hash.as_str().to_owned()),
                        mime: Some((*mime).to_owned()),
                    })
                    .collect(),
            },
        };
        let mut state = self.lock();
        let documents = state.documents.entry(workspace).or_default();
        match documents.iter_mut().find(|held| held.document == document) {
            Some(held) => *held = sampled,
            None => documents.push(sampled),
        }
    }

    /// Arm the race: the next reads of this Workspace's sample answer with
    /// these, in order. `None` in a slot means the durable set; anything past
    /// the end of the queue is the durable set too.
    pub fn arm_samples(&self, workspace: WorkspaceId, samples: Vec<Option<Vec<SampledDocument>>>) {
        self.lock()
            .armed_samples
            .insert(workspace, samples.into_iter().collect());
    }

    /// Every document this Workspace holds, as the sample reads them.
    pub fn sampled(&self, workspace: WorkspaceId) -> Vec<SampledDocument> {
        self.lock()
            .documents
            .get(&workspace)
            .cloned()
            .unwrap_or_default()
    }

    /// This Workspace holds these bytes and is associated with them.
    pub fn hold(&self, workspace: WorkspaceId, bytes: &[u8], mime: &str, filename: Option<&str>) {
        let hash = hash_of(bytes);
        let blob = LocalBlob {
            hash: hash.clone(),
            size: bytes.len() as u64,
            mime: mime.to_owned(),
            filename: filename.map(str::to_owned),
        };
        let mut state = self.lock();
        state.local.insert(
            (workspace, hash.as_str().to_owned()),
            (blob, bytes.to_vec()),
        );
        state
            .objects
            .insert(hash.as_str().to_owned(), bytes.to_vec());
    }

    /// The bytes at this address no longer hash to it.
    pub fn rot(&self, hash: &BlobHash) {
        self.lock().rotten.push(hash.as_str().to_owned());
    }

    pub fn installs(&self) -> Vec<(WorkspaceId, String)> {
        self.lock().installs.clone()
    }

    pub fn associated(&self, workspace: WorkspaceId, hash: &BlobHash) -> bool {
        self.lock()
            .local
            .contains_key(&(workspace, hash.as_str().to_owned()))
    }

    pub fn object_bytes(&self, hash: &BlobHash) -> Option<Vec<u8>> {
        self.lock().objects.get(hash.as_str()).cloned()
    }
}

impl BlobStorePort for FakeStore {
    fn sample_documents(
        &self,
        workspace: WorkspaceId,
    ) -> PortFuture<'_, Result<DocumentSample, TransferError>> {
        self.calls.walks.fetch_add(1, Ordering::Relaxed);
        let mut state = self.lock();
        let armed = state
            .armed_samples
            .get_mut(&workspace)
            .and_then(VecDeque::pop_front)
            .flatten();
        let documents = match armed {
            Some(armed) => armed,
            None => state.documents.get(&workspace).cloned().unwrap_or_default(),
        };
        drop(state);
        Box::pin(async move { Ok(DocumentSample::new(documents)) })
    }

    fn object_present(&self, hash: BlobHash) -> PortFuture<'_, Result<bool, TransferError>> {
        let present = self.lock().objects.contains_key(hash.as_str());
        Box::pin(async move { Ok(present) })
    }

    fn workspace_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<Option<LocalBlob>, TransferError>> {
        // **Associated *and* present**, as the real port answers: a released
        // blob keeps its row and has no object, and this must say `None` for
        // it or a release could never be observed through the port.
        let state = self.lock();
        let found = state
            .local
            .get(&(workspace, hash.as_str().to_owned()))
            .filter(|_| state.objects.contains_key(hash.as_str()))
            .map(|(blob, _)| blob.clone());
        drop(state);
        Box::pin(async move { Ok(found) })
    }

    fn verify_object(
        &self,
        _workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        let rotten = self
            .lock()
            .rotten
            .iter()
            .any(|value| *value == hash.as_str());
        Box::pin(async move {
            if rotten {
                Err(TransferError::Local(Code::BlobChecksumMismatch))
            } else {
                Ok(())
            }
        })
    }

    fn open_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<Box<dyn std::io::Read + Send>, TransferError>> {
        let bytes = self
            .lock()
            .local
            .get(&(workspace, hash.as_str().to_owned()))
            .map(|(_, bytes)| bytes.clone());
        Box::pin(async move {
            match bytes {
                Some(bytes) => {
                    Ok(Box::new(std::io::Cursor::new(bytes)) as Box<dyn std::io::Read + Send>)
                }
                None => Err(TransferError::Local(Code::BlobNotFound)),
            }
        })
    }

    fn install_downloaded(
        &self,
        workspace: WorkspaceId,
        blob: LocalBlob,
        staged: PathBuf,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        // Done before the future, because a `MutexGuard` held across an
        // `await` is a future that is not `Send` and the ports are `dyn`.
        let outcome = (|| {
            let bytes =
                std::fs::read(&staged).map_err(|_| TransferError::Local(Code::StorageFailed))?;
            // The fake asserts the client's half of S10 for every test that
            // uses it: nothing is installed unless the staged file is the
            // whole object and hashes to its own address.
            assert_eq!(
                bytes.len() as u64,
                blob.size,
                "a partial file was installed"
            );
            assert_eq!(
                hash_of(&bytes),
                blob.hash,
                "bytes that do not hash were installed"
            );
            let mut state = self.lock();
            // Object first.
            state
                .objects
                .insert(blob.hash.as_str().to_owned(), bytes.clone());
            state
                .installs
                .push((workspace, blob.hash.as_str().to_owned()));
            // Association last.
            state
                .local
                .insert((workspace, blob.hash.as_str().to_owned()), (blob, bytes));
            Ok(())
        })();
        Box::pin(async move { outcome })
    }

    fn staging_root(&self) -> PathBuf {
        self.staging.clone()
    }

    fn release_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        let mut state = self.lock();
        let key = (workspace, hash.as_str().to_owned());
        // Association first, as the real Store checks it: a hash this
        // Workspace never referenced is not this Workspace's to release.
        if !state.local.contains_key(&key) {
            drop(state);
            return Box::pin(async { Err(TransferError::Local(Code::BlobNotFound)) });
        }
        // The object goes; the row stays.
        let freed = state
            .objects
            .remove(hash.as_str())
            .map_or(0, |bytes| bytes.len() as u64);
        drop(state);
        Box::pin(async move { Ok(freed) })
    }
}

// ----------------------------------------------------------------- access

pub struct FakeAccess {
    facts: Mutex<Option<CloudAccessFacts>>,
}

impl FakeAccess {
    pub fn new(origins: ByteOrigins) -> Arc<Self> {
        Arc::new(Self {
            facts: Mutex::new(Some(CloudAccessFacts {
                api_origin: url::Url::parse("https://us.api.koru.do/").expect("a real origin"),
                bearer: "test-account-token".into(),
                region: Region::Us,
                byte_origins: origins,
            })),
        })
    }

    pub fn restore(&self) {
        *self.facts.lock().unwrap_or_else(|e| e.into_inner()) = Some(CloudAccessFacts {
            api_origin: url::Url::parse("https://us.api.koru.do/").expect("a real origin"),
            bearer: "test-account-token".into(),
            region: Region::Us,
            byte_origins: ByteOrigins::for_region(Region::Us),
        });
    }
    pub fn revoke(&self) {
        *self.facts.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl CloudAccess for FakeAccess {
    fn access(&self, _workspace: WorkspaceId) -> Option<CloudAccessFacts> {
        self.facts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Permitted or not, and — when not — which of the four holds says so.
pub struct SwitchablePolicy(
    pub std::sync::atomic::AtomicBool,
    pub Mutex<crate::ports::TransferHold>,
);

impl SwitchablePolicy {
    pub fn permitting() -> Self {
        Self(
            std::sync::atomic::AtomicBool::new(true),
            Mutex::new(crate::ports::TransferHold::default()),
        )
    }

    /// Hold transfers, with the reason a platform would give.
    pub fn holding(&self, hold: crate::ports::TransferHold) {
        *self.1.lock().unwrap_or_else(|e| e.into_inner()) = hold;
        self.0.store(false, Ordering::Relaxed);
    }

    pub fn permit(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl BlobPolicy for SwitchablePolicy {
    fn transfers_permitted(&self, _workspace: WorkspaceId) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn hold(&self, _workspace: WorkspaceId) -> crate::ports::TransferHold {
        *self.1.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A stand-in for `Store::admit_transport`, with the answer **armed** rather
/// than asserted. It counts the asks too, because *"is the gate asked again
/// inside the transfer"* is a claim about a number, not about intent.
pub struct SwitchableModes {
    admitted: Mutex<std::collections::BTreeSet<&'static str>>,
    pub asked: AtomicU64,
}

impl SwitchableModes {
    /// A Workspace at Cloud: both paths admitted, which is the ladder's top.
    pub fn cloud() -> Arc<Self> {
        Self::with(&["cloud", "peer"])
    }

    /// A Workspace at P2P: peers yes, Cloud no.
    pub fn p2p() -> Arc<Self> {
        Self::with(&["peer"])
    }

    /// Cloud admitted and the peer path closed. Not a rung of the ladder — the
    /// shape a Profile ceiling or a half-applied fence can leave behind, and
    /// the reason the peer arm asks rather than inferring.
    pub fn cloud_only() -> Arc<Self> {
        Self::with(&["cloud"])
    }

    /// A Workspace at Local-only: nothing moves in either direction.
    pub fn local_only() -> Arc<Self> {
        Self::with(&[])
    }

    fn with(paths: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            admitted: Mutex::new(paths.iter().copied().collect()),
            asked: AtomicU64::new(0),
        })
    }

    /// What a restrictive local choice does: the answer changes **now**, before
    /// anything durable has happened.
    pub fn close(&self) {
        self.admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn open_cloud(&self) {
        self.admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("cloud");
    }
}

impl crate::ports::TransportModes for SwitchableModes {
    fn admit(
        &self,
        _workspace: WorkspaceId,
        path: koru_store_core::TransportPath,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        let name = match path {
            koru_store_core::TransportPath::Cloud => "cloud",
            koru_store_core::TransportPath::Peer => "peer",
        };
        let admitted = self
            .admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(name);
        Box::pin(async move {
            if admitted {
                Ok(())
            } else {
                // The mode gate's own refusal, which is what this fake stands
                // in for: B5 gave every hold a name of its own, so a refusal
                // is no longer the same word as a metered pause.
                Err(TransferError::NotPermitted(TransferHold::TransportMode))
            }
        })
    }
}

// ------------------------------------------------------------------- peers

/// A peer holding a blob on a direct path, with how far it gets **armed**.
///
/// The real fall-through is a device leaving the network mid-transfer. Here it
/// is a byte count, so "a peer that dies mid-transfer costs a partial staging
/// file and a fallback, not a stall" is a thing a test can state exactly.
#[derive(Default)]
pub struct FakePeers {
    held: Mutex<BTreeMap<String, (Vec<u8>, String)>>,
    /// How many bytes this peer will serve before it goes away. `None` is a
    /// peer that stays.
    stops_after: Mutex<Option<u64>>,
    pub served: AtomicU64,
}

impl FakePeers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// This peer holds these bytes and advertises this hash.
    pub fn hold(&self, bytes: &[u8], mime: &str) -> BlobHash {
        let hash = hash_of(bytes);
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(hash.as_str().to_owned(), (bytes.to_vec(), mime.to_owned()));
        hash
    }

    /// A peer advertising one address and holding something else at it. Only
    /// the digest can tell, which is why the digest is what decides.
    pub fn hold_as(&self, advertised: &BlobHash, bytes: &[u8], mime: &str, _total: u64) {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).insert(
            advertised.as_str().to_owned(),
            (bytes.to_vec(), mime.to_owned()),
        );
    }

    /// The peer serves this many bytes and then goes away.
    pub fn stops_after(&self, bytes: u64) {
        *self.stops_after.lock().unwrap_or_else(|e| e.into_inner()) = Some(bytes);
    }
}

impl crate::ports::PeerBlobs for FakePeers {
    fn read_range(
        &self,
        _workspace: WorkspaceId,
        hash: BlobHash,
        offset: u64,
        max: u64,
    ) -> PortFuture<'_, Result<Option<crate::ports::PeerChunk>, TransferError>> {
        Box::pin(async move {
            let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            let Some((bytes, mime)) = held.get(hash.as_str()) else {
                return Ok(None);
            };
            let total = bytes.len() as u64;
            let limit = self
                .stops_after
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or(total);
            if offset >= limit {
                // Gone. Whatever is already on the disk stays there.
                return Ok(None);
            }
            let end = (offset + max).min(total).min(limit);
            self.served.fetch_add(end - offset, Ordering::Relaxed);
            Ok(Some(crate::ports::PeerChunk {
                bytes: bytes[offset as usize..end as usize].to_vec(),
                total,
                mime: mime.clone(),
                filename: None,
            }))
        })
    }
}

// -------------------------------------------------------------- downloads

#[derive(Default)]
pub struct RecordingDownloads {
    pub watching: Mutex<Vec<String>>,
    pub arrived: Mutex<Vec<String>>,
}

impl RecordingDownloads {
    /// A document showing this blob is on screen — what makes an absent blob
    /// wanted by a pass (D260).
    pub fn watch(&self, hash: &BlobHash) {
        self.watching
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(hash.as_str().to_owned());
    }
}

impl BlobDownloads for RecordingDownloads {
    fn watching(&self, _workspace: WorkspaceId, hash: &BlobHash) -> bool {
        self.watching
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|value| value == hash.as_str())
    }
    fn arrived(&self, _workspace: WorkspaceId, hash: &BlobHash) {
        self.arrived
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(hash.as_str().to_owned());
    }
}

// ---------------------------------------------------- the two fake planes

#[derive(Default)]
struct PlaneState {
    /// Sessions the control plane opened, by upload id.
    sessions: BTreeMap<String, Session>,
    /// Objects the worker has published, by locator.
    objects: BTreeMap<String, Vec<u8>>,
    /// Durable mappings, by `(workspace, hash)`.
    durable: BTreeMap<(String, String), (String, LocalBlob)>,
    /// A refusal to answer the next call of a named kind, once.
    next_refusal: Option<(&'static str, BlobCode)>,
}

struct Session {
    workspace: String,
    hash: String,
    locator: String,
    size: u64,
    mime: String,
    filename: Option<String>,
    staged: Vec<u8>,
}

/// A control plane and an object plane that agree with each other, in memory.
///
/// Enough for the engine's decisions — ordering, parking, renewal, the
/// allowlist — and deliberately not enough for the contract, which is asserted
/// over HTTP instead (`tests/object_service.rs`, and the real worker's
/// `tests/client_*`).
pub struct FakePlane {
    state: Mutex<PlaneState>,
    calls: Arc<Calls>,
    host: String,
    /// Object-plane bytes actually served. "Peer-first" is a claim about this
    /// number, so it is measured rather than inferred from a call count.
    pub bytes_served: AtomicU64,
}

impl FakePlane {
    pub fn new(calls: Arc<Calls>, host: &str) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PlaneState::default()),
            calls,
            host: host.to_owned(),
            bytes_served: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PlaneState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cloud already holds these bytes for this Workspace.
    pub fn publish(&self, workspace: WorkspaceId, bytes: &[u8], mime: &str) {
        let hash = hash_of(bytes);
        let locator = uuid::Uuid::new_v4().hyphenated().to_string();
        let mut state = self.lock();
        state.objects.insert(locator.clone(), bytes.to_vec());
        state.durable.insert(
            (workspace.to_string(), hash.as_str().to_owned()),
            (
                locator,
                LocalBlob {
                    hash,
                    size: bytes.len() as u64,
                    mime: mime.to_owned(),
                    filename: None,
                },
            ),
        );
    }

    /// A session that has started and not finished: Cloud holds part of this
    /// picture and no mapping for it. What "mid-upload" means to `status`.
    pub fn uploading(&self, workspace: WorkspaceId, hash: &BlobHash, size: u64, sent: u64) {
        let upload = uuid::Uuid::new_v4().hyphenated().to_string();
        let locator = uuid::Uuid::new_v4().hyphenated().to_string();
        self.lock().sessions.insert(
            upload,
            Session {
                workspace: workspace.to_string(),
                hash: hash.as_str().to_owned(),
                locator,
                size,
                mime: "image/png".to_owned(),
                filename: None,
                staged: vec![0u8; sent.min(size) as usize],
            },
        );
    }

    /// Corrupt what a locator serves, so a download's hash cannot match.
    pub fn corrupt(&self, workspace: WorkspaceId, hash: &BlobHash) {
        let mut state = self.lock();
        let Some((locator, _)) = state
            .durable
            .get(&(workspace.to_string(), hash.as_str().to_owned()))
            .cloned()
        else {
            return;
        };
        if let Some(bytes) = state.objects.get_mut(&locator) {
            bytes.reverse();
        }
    }

    /// The next call of `kind` is refused once with `code`.
    pub fn refuse_once(&self, kind: &'static str, code: BlobCode) {
        self.lock().next_refusal = Some((kind, code));
    }

    /// The byte URL this plane hands out. A test that wants a hostile one
    /// constructs the plane with a hostile host.
    fn upload_url(&self, upload: &str) -> String {
        format!("{}v1/objects/uploads/{upload}", self.host)
    }
    fn object_url(&self, locator: &str) -> String {
        format!("{}v1/objects/{locator}", self.host)
    }

    fn take_refusal(&self, kind: &'static str) -> Option<BlobCode> {
        let mut state = self.lock();
        match state.next_refusal {
            Some((named, code)) if named == kind => {
                state.next_refusal = None;
                Some(code)
            }
            _ => None,
        }
    }

    fn session_of(&self, url: &AdmittedUrl) -> Option<String> {
        url.as_url()
            .path()
            .strip_prefix("/v1/objects/uploads/")
            .map(|rest| rest.trim_end_matches("/complete").to_owned())
    }

    fn locator_of(&self, url: &AdmittedUrl) -> Option<String> {
        url.as_url()
            .path()
            .strip_prefix("/v1/objects/")
            .map(str::to_owned)
    }
}

impl BlobControlPlane for FakePlane {
    fn status(
        &self,
        _access: CloudAccessFacts,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<BlobStatusV1, TransferError>> {
        self.calls.control.fetch_add(1, Ordering::Relaxed);
        let state = self.lock();
        let key = (workspace.to_string(), hash.as_str().to_owned());
        let answer = match state.durable.get(&key) {
            Some((_, blob)) => BlobStatusV1::Durable {
                size: blob.size,
                mime: blob.mime.clone(),
                filename: blob.filename.clone(),
                durable_at_millis: 1,
            },
            // **A live session is its own answer**, as the real control plane's
            // §3.1 says: an upload that has started and not finished is neither
            // durable nor missing, and a fake that flattened the three states
            // into two would let coverage call a half-sent picture absent.
            None => match state
                .sessions
                .iter()
                .find(|(_, session)| session.workspace == key.0 && session.hash == key.1)
            {
                Some((upload, session)) => BlobStatusV1::Uploading {
                    upload_id: cloud_id(upload),
                    committed_offset: session.staged.len() as u64,
                    chunk_size: koru_cloud_blob::CHUNK,
                    expires_at_millis: i64::MAX,
                    byte_url: self.upload_url(upload),
                },
                None => BlobStatusV1::Missing { failure: None },
            },
        };
        drop(state);
        Box::pin(async move { Ok(answer) })
    }

    fn allocate(
        &self,
        _access: CloudAccessFacts,
        workspace: WorkspaceId,
        hash: BlobHash,
        request: AllocateRequestV1,
    ) -> PortFuture<'_, Result<AllocationV1, TransferError>> {
        self.calls.control.fetch_add(1, Ordering::Relaxed);
        if let Some(code) = self.take_refusal("allocate") {
            return Box::pin(async move { Err(TransferError::Refused(code)) });
        }
        let mut state = self.lock();
        let key = (workspace.to_string(), hash.as_str().to_owned());
        if let Some((locator, blob)) = state.durable.get(&key).cloned() {
            let _ = locator;
            return Box::pin(async move {
                Ok(AllocationV1::Durable {
                    object_locator: koru_cloud_blob::ObjectLocator::mint(),
                    size: blob.size,
                    mime: blob.mime,
                    filename: blob.filename,
                    durable_at_millis: 1,
                })
            });
        }
        // Idempotent by the triple, exactly as the real one is.
        let existing = state
            .sessions
            .iter()
            .find(|(_, session)| {
                session.workspace == workspace.to_string() && session.hash == hash.as_str()
            })
            .map(|(id, _)| id.clone());
        let upload = existing.unwrap_or_else(|| {
            let id = uuid::Uuid::new_v4().hyphenated().to_string();
            state.sessions.insert(
                id.clone(),
                Session {
                    workspace: workspace.to_string(),
                    hash: hash.as_str().to_owned(),
                    locator: uuid::Uuid::new_v4().hyphenated().to_string(),
                    size: request.size,
                    mime: request.mime.clone(),
                    filename: request.filename.clone(),
                    staged: Vec::new(),
                },
            );
            id
        });
        let session = &state.sessions[&upload];
        let allocation = AllocationV1::Uploading {
            upload_id: cloud_id(&upload),
            object_locator: koru_cloud_blob::ObjectLocator::parse(&session.locator)
                .expect("a minted locator"),
            committed_offset: session.staged.len() as u64,
            chunk_size: koru_cloud_blob::CHUNK,
            expires_at_millis: i64::MAX,
            byte_url: self.upload_url(&upload),
            capability: format!("capability-for-{upload}"),
        };
        drop(state);
        Box::pin(async move { Ok(allocation) })
    }

    fn renew(
        &self,
        _access: CloudAccessFacts,
        _workspace: WorkspaceId,
        upload: CloudId,
    ) -> PortFuture<'_, Result<AllocationV1, TransferError>> {
        self.calls.control.fetch_add(1, Ordering::Relaxed);
        let id = upload.as_uuid().hyphenated().to_string();
        let state = self.lock();
        let Some(session) = state.sessions.get(&id) else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::UploadExpired)) });
        };
        let allocation = AllocationV1::Uploading {
            upload_id: upload,
            object_locator: koru_cloud_blob::ObjectLocator::parse(&session.locator)
                .expect("a minted locator"),
            committed_offset: session.staged.len() as u64,
            chunk_size: koru_cloud_blob::CHUNK,
            expires_at_millis: i64::MAX,
            byte_url: self.upload_url(&id),
            capability: format!("renewed-for-{id}"),
        };
        drop(state);
        Box::pin(async move { Ok(allocation) })
    }

    fn abort(
        &self,
        _access: CloudAccessFacts,
        _workspace: WorkspaceId,
        upload: CloudId,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        self.calls.control.fetch_add(1, Ordering::Relaxed);
        self.lock()
            .sessions
            .remove(&upload.as_uuid().hyphenated().to_string());
        Box::pin(async move { Ok(()) })
    }

    fn read_capability(
        &self,
        _access: CloudAccessFacts,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<ReadCapabilityV1, TransferError>> {
        self.calls.control.fetch_add(1, Ordering::Relaxed);
        if let Some(code) = self.take_refusal("read_capability") {
            return Box::pin(async move { Err(TransferError::Refused(code)) });
        }
        let state = self.lock();
        let Some((locator, blob)) = state
            .durable
            .get(&(workspace.to_string(), hash.as_str().to_owned()))
            .cloned()
        else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::ObjectMissing)) });
        };
        let answer = ReadCapabilityV1 {
            capability: format!("read-{locator}"),
            byte_url: self.object_url(&locator),
            size: blob.size,
            mime: blob.mime,
            filename: blob.filename,
        };
        drop(state);
        Box::pin(async move { Ok(answer) })
    }
}

impl BlobObjectPlane for FakePlane {
    fn committed_offset(
        &self,
        url: AdmittedUrl,
        _capability: String,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        self.calls.bytes.fetch_add(1, Ordering::Relaxed);
        let offset = self
            .session_of(&url)
            .and_then(|id| self.lock().sessions.get(&id).map(|s| s.staged.len() as u64));
        Box::pin(async move { offset.ok_or(TransferError::Refused(BlobCode::UploadExpired)) })
    }

    fn patch(
        &self,
        url: AdmittedUrl,
        _capability: String,
        start: u64,
        total: u64,
        body: Vec<u8>,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        self.calls.bytes.fetch_add(1, Ordering::Relaxed);
        if let Some(code) = self.take_refusal("patch") {
            return Box::pin(async move { Err(TransferError::Refused(code)) });
        }
        let Some(id) = self.session_of(&url) else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::WrongObject)) });
        };
        let mut state = self.lock();
        let Some(session) = state.sessions.get_mut(&id) else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::UploadExpired)) });
        };
        if total != session.size {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::LengthMismatch)) });
        }
        if start != session.staged.len() as u64 {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::RangeConflict)) });
        }
        session.staged.extend_from_slice(&body);
        let offset = session.staged.len() as u64;
        drop(state);
        Box::pin(async move { Ok(offset) })
    }

    fn complete(
        &self,
        url: AdmittedUrl,
        _capability: String,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        self.calls.bytes.fetch_add(1, Ordering::Relaxed);
        let Some(id) = self.session_of(&url) else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::WrongObject)) });
        };
        let mut state = self.lock();
        let Some(session) = state.sessions.remove(&id) else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::UploadExpired)) });
        };
        if session.staged.len() as u64 != session.size {
            state.sessions.insert(id, session);
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::LengthMismatch)) });
        }
        if hash_of(&session.staged).as_str() != session.hash {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::HashMismatch)) });
        }
        state
            .objects
            .insert(session.locator.clone(), session.staged.clone());
        state.durable.insert(
            (session.workspace.clone(), session.hash.clone()),
            (
                session.locator.clone(),
                LocalBlob {
                    hash: BlobHash::parse(&session.hash).expect("a real address"),
                    size: session.size,
                    mime: session.mime.clone(),
                    filename: session.filename.clone(),
                },
            ),
        );
        drop(state);
        Box::pin(async move { Ok(()) })
    }

    fn read_range(
        &self,
        url: AdmittedUrl,
        _capability: String,
        start: u64,
        length: u64,
    ) -> PortFuture<'_, Result<Vec<u8>, TransferError>> {
        self.calls.bytes.fetch_add(1, Ordering::Relaxed);
        if let Some(code) = self.take_refusal("read_range") {
            return Box::pin(async move { Err(TransferError::Refused(code)) });
        }
        let bytes = self
            .locator_of(&url)
            .and_then(|locator| self.lock().objects.get(&locator).cloned());
        Box::pin(async move {
            let bytes = bytes.ok_or(TransferError::Refused(BlobCode::WrongObject))?;
            let start = start as usize;
            let end = (start + length as usize).min(bytes.len());
            if start >= bytes.len() {
                return Err(TransferError::Refused(BlobCode::MalformedRange));
            }
            self.bytes_served
                .fetch_add((end - start) as u64, Ordering::Relaxed);
            Ok(bytes[start..end].to_vec())
        })
    }
}

fn cloud_id(value: &str) -> CloudId {
    CloudId::from_bytes(
        *uuid::Uuid::parse_str(value)
            .expect("a minted id")
            .as_bytes(),
    )
}

/// Everything one engine test needs, assembled.
pub struct Harness {
    pub engine: Arc<crate::engine::BlobEngine>,
    pub store: Arc<FakeStore>,
    pub plane: Arc<FakePlane>,
    pub access: Arc<FakeAccess>,
    pub downloads: Arc<RecordingDownloads>,
    pub policy: Arc<SwitchablePolicy>,
    pub modes: Arc<SwitchableModes>,
    pub peers: Arc<FakePeers>,
    pub calls: Arc<Calls>,
    pub workspace: WorkspaceId,
    _root: tempfile::TempDir,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_host("https://us.data.koru.do/")
    }

    /// A Workspace at one rung of the ladder, so §6's table can be asserted a
    /// row at a time.
    pub fn at_mode(modes: Arc<SwitchableModes>) -> Self {
        let mut harness = Self::new();
        harness.rebuild_with(modes);
        harness
    }

    fn rebuild_with(&mut self, modes: Arc<SwitchableModes>) {
        self.modes = modes.clone();
        self.engine = crate::engine::BlobEngine::new(
            BlobPorts {
                store: self.store.clone(),
                access: self.access.clone(),
                policy: self.policy.clone(),
                modes,
                peers: self.peers.clone(),
                downloads: self.downloads.clone(),
            },
            self.plane.clone(),
            self.plane.clone(),
        );
    }

    pub fn with_host(host: &str) -> Self {
        let root = tempfile::tempdir().expect("a private root");
        let calls = Arc::new(Calls::default());
        let store = FakeStore::new(root.path().join("blobs").join("staging"), calls.clone());
        let plane = FakePlane::new(calls.clone(), host);
        let access = FakeAccess::new(ByteOrigins::for_region(Region::Us));
        let downloads = Arc::new(RecordingDownloads::default());
        let policy = Arc::new(SwitchablePolicy::permitting());
        // A Cloud Workspace by default, which is what every test written before
        // the gate existed assumed and what most of them are still about.
        let modes = SwitchableModes::cloud();
        // No peer holds anything unless a test says so, which is what every
        // Cloud-plane test in this crate assumes.
        let peers = FakePeers::new();
        let engine = crate::engine::BlobEngine::new(
            BlobPorts {
                store: store.clone(),
                access: access.clone(),
                policy: policy.clone(),
                modes: modes.clone(),
                peers: peers.clone(),
                downloads: downloads.clone(),
            },
            plane.clone(),
            plane.clone(),
        );
        Self {
            engine,
            store,
            plane,
            access,
            downloads,
            policy,
            modes,
            peers,
            calls,
            workspace: WorkspaceId::new(),
            _root: root,
        }
    }
}

/// A document state a test can order: two different numbers are two different
/// states, and the higher one includes the lower.
///
/// Real states are encoded Loro version vectors, so the fake mints real ones —
/// a fake state that did not decode would make every inclusion answer `false`
/// and every coverage test pass for the wrong reason.
pub fn state_of(counter: i32) -> DocumentState {
    let mut version = loro::VersionVector::default();
    version.set_last(loro::ID::new(1, counter));
    DocumentState::from_bytes(version.encode())
}
