// SPDX-License-Identifier: MPL-2.0

//! The object service on a real socket, and the two doubles around it.
//!
//! Shared by every integration binary. `bounded_memory.rs` has to be its own
//! process — a counting global allocator measures everything in one — and a
//! second half-copy of this harness is how two doubles come to disagree about
//! the thing they are both standing in for.
//!
//! What is real here: the client's HTTP plane, every capability signature, and
//! every admission the byte routes make, which are the shared contract's own
//! functions ([`ObjectService`] says which). What is a double: the object
//! service's storage, and the control plane, because the database half of the
//! contract is asserted by the hosted control plane's own PostgreSQL tests.
//! The hosted service, which is not in this repository, runs the same suites
//! against its real object worker.

#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use koru_account_core::workspace_authorization::Region;
use koru_account_session::cloud::blob::{
    BLOB_CAPABILITY_TYPE, BlobCapabilityClaimsV1, BlobPurpose, blob_capability_expiry,
    blob_capability_not_before,
};
use koru_blob_sync::{
    BlobControlPlane, BlobStorePort, CloudAccessFacts, LocalBlob, PortFuture, TransferError,
    WalkDocument, WalkNode,
};
use koru_cloud_blob::{
    BlobCode, ObjectLocator,
    wire::{AllocateRequestV1, AllocationV1, BlobStatusV1, ReadCapabilityV1},
};
use koru_device::cloud::CloudId;
use koru_store_core::{WorkspaceId, errors::Code, model::BlobHash};

pub const SEED: [u8; 32] = [7; 32];
pub const KEY_ID: &str = "regional";
pub const ISSUER: &str = "https://us.example.invalid";
/// The audience the object service is configured to answer for. It is **not** the host
/// the client dials: the service compares a capability's `aud` with its own
/// configuration, and a harness that made the two the same would not notice if
/// that stopped being true.
pub const AUDIENCE: &str = "us.data.koru.do";

pub fn fresh() -> CloudId {
    CloudId::from_bytes(*uuid::Uuid::new_v4().as_bytes())
}

pub fn hash_of(bytes: &[u8]) -> BlobHash {
    BlobHash::parse(&format!("b3:{}", blake3::hash(bytes).to_hex())).expect("a real address")
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// A real compact JWS the object service verifies.
pub fn sign(claims: &BlobCapabilityClaimsV1) -> String {
    let header = serde_json::json!({"alg":"EdDSA","typ":BLOB_CAPABILITY_TYPE,"kid":KEY_ID});
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("a header")),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims"))
    );
    format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(
            SigningKey::from_bytes(&SEED)
                .sign(input.as_bytes())
                .to_bytes()
        )
    )
}

pub fn claims(
    purpose: BlobPurpose,
    dataset: (CloudId, CloudId),
    sub: CloudId,
    loc: CloudId,
    len: u64,
    hash: &str,
) -> BlobCapabilityClaimsV1 {
    let nbf = blob_capability_not_before(now_millis());
    BlobCapabilityClaimsV1 {
        v: 1,
        pur: purpose.as_str().into(),
        iss: ISSUER.into(),
        aud: AUDIENCE.into(),
        rgn: Region::Us,
        acc: fresh(),
        wsp: dataset.0,
        inc: dataset.1,
        eph: 7,
        sub,
        loc,
        bh: hash.to_string(),
        len,
        nbf,
        exp: blob_capability_expiry(nbf).expect("an expiry"),
        jti: fresh(),
    }
}

// ------------------------------------------------------- the object service

mod object_service;

pub use object_service::ObjectService;

/// Put an object on the service's volume the way a real client would.
pub async fn publish(service: &ObjectService, bytes: &[u8]) -> (ObjectLocator, (CloudId, CloudId)) {
    let http = reqwest::Client::new();
    let hash = format!("b3:{}", blake3::hash(bytes).to_hex());
    let upload = fresh();
    let locator = fresh();
    let dataset = (fresh(), fresh());
    let write = sign(&claims(
        BlobPurpose::Write,
        dataset,
        upload,
        locator,
        bytes.len() as u64,
        &hash,
    ));
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + koru_cloud_blob::CHUNK as usize).min(bytes.len()) - 1;
        let response = http
            .patch(format!("{}/v1/objects/uploads/{upload}", service.base))
            .header("authorization", format!("Capability {write}"))
            .header(
                "content-range",
                format!("bytes {offset}-{end}/{}", bytes.len()),
            )
            .body(bytes[offset..=end].to_vec())
            .send()
            .await
            .expect("a chunk");
        assert_eq!(response.status(), 200, "chunk at {offset}");
        offset = end + 1;
    }
    let response = http
        .post(format!(
            "{}/v1/objects/uploads/{upload}/complete",
            service.base
        ))
        .header("authorization", format!("Capability {write}"))
        .send()
        .await
        .expect("complete");
    assert_eq!(response.status(), 202);

    let locator_value = ObjectLocator::from_uuid(locator.as_uuid());
    for _ in 0..1_200 {
        if service
            .published(locator_value, dataset, bytes.len() as u64, &hash)
            .await
        {
            return (locator_value, dataset);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the object was never published");
}

// ------------------------------------------------------ the control double

pub struct Published {
    pub locator: ObjectLocator,
    pub dataset: (CloudId, CloudId),
    pub size: u64,
    pub mime: String,
    pub filename: Option<String>,
}

struct Session {
    upload: CloudId,
    locator: CloudId,
    dataset: (CloudId, CloudId),
    hash: String,
    size: u64,
}

/// A control plane that mints **real** capabilities and holds no database.
pub struct TestControl {
    base: String,
    published: Mutex<BTreeMap<String, Published>>,
    sessions: Mutex<BTreeMap<String, Session>>,
    /// Whether the double should ask the object service whether an upload has become
    /// durable. Off by default, because most tests publish by hand.
    poll_service: Mutex<Option<Arc<ServiceProbe>>>,
}

/// What the double needs to answer "is it durable yet" honestly: the object service's
/// own byte route, asked the way a client asks.
pub struct ServiceProbe {
    pub base: String,
}

impl TestControl {
    pub fn new(base: &str) -> Arc<Self> {
        Arc::new(Self {
            base: base.to_owned(),
            published: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(BTreeMap::new()),
            poll_service: Mutex::new(None),
        })
    }

    /// Answer `durable` by asking the object service, so an upload driven through this
    /// double reaches the same terminal state a real one does.
    pub fn probing(self: &Arc<Self>) -> Arc<Self> {
        *self.poll_service.lock().expect("the double's own lock") = Some(Arc::new(ServiceProbe {
            base: self.base.clone(),
        }));
        self.clone()
    }

    /// Cloud holds `locator`, and answers for it under `address`.
    ///
    /// The address is a separate argument on purpose: S11 needs a mapping
    /// whose bytes do not hash to the address a client asked for, which is the
    /// only honest way to make a real service serve wrong bytes.
    pub fn publish(&self, address: &BlobHash, entry: Published) {
        self.published
            .lock()
            .expect("the double's own lock")
            .insert(address.as_str().to_owned(), entry);
    }
}

impl BlobControlPlane for TestControl {
    fn status(
        &self,
        _access: CloudAccessFacts,
        _workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<BlobStatusV1, TransferError>> {
        if let Some(entry) = self
            .published
            .lock()
            .expect("the double's own lock")
            .get(hash.as_str())
        {
            let answer = BlobStatusV1::Durable {
                size: entry.size,
                mime: entry.mime.clone(),
                filename: entry.filename.clone(),
                durable_at_millis: 1,
            };
            return Box::pin(async move { Ok(answer) });
        }
        // The session half: this double has no report seam, so "durable" is
        // asked of the object service's own byte route — which is what durability
        // actually means on that side.
        let probe = self
            .poll_service
            .lock()
            .expect("the double's own lock")
            .clone();
        let session = self
            .sessions
            .lock()
            .expect("the double's own lock")
            .values()
            .find(|session| session.hash == hash.as_str())
            .map(|session| (session.locator, session.dataset, session.size));
        Box::pin(async move {
            let (Some(probe), Some((locator, dataset, size))) = (probe, session) else {
                return Ok(BlobStatusV1::Missing { failure: None });
            };
            let locator = ObjectLocator::from_uuid(locator.as_uuid());
            let subject = CloudId::from_bytes(*locator.as_uuid().as_bytes());
            let read = sign(&claims(
                BlobPurpose::Read,
                dataset,
                subject,
                subject,
                size,
                hash.as_str(),
            ));
            let durable = reqwest::Client::new()
                .get(format!("{}/v1/objects/{locator}", probe.base))
                .header("authorization", format!("Capability {read}"))
                .header("range", "bytes=0-0")
                .send()
                .await
                .map(|response| response.status() == 206)
                .unwrap_or(false);
            Ok(if durable {
                BlobStatusV1::Durable {
                    size,
                    mime: "application/octet-stream".into(),
                    filename: None,
                    durable_at_millis: 1,
                }
            } else {
                BlobStatusV1::Missing { failure: None }
            })
        })
    }

    fn allocate(
        &self,
        _access: CloudAccessFacts,
        _workspace: WorkspaceId,
        hash: BlobHash,
        request: AllocateRequestV1,
    ) -> PortFuture<'_, Result<AllocationV1, TransferError>> {
        let mut sessions = self.sessions.lock().expect("the double's own lock");
        // Idempotent by the hash, exactly as the real one is by the triple.
        let existing = sessions
            .iter()
            .find(|(_, session)| session.hash == hash.as_str())
            .map(|(id, _)| id.clone());
        let id = existing.unwrap_or_else(|| {
            let upload = fresh();
            let id = upload.as_uuid().hyphenated().to_string();
            sessions.insert(
                id.clone(),
                Session {
                    upload,
                    locator: fresh(),
                    dataset: (fresh(), fresh()),
                    hash: hash.as_str().to_owned(),
                    size: request.size,
                },
            );
            id
        });
        let session = &sessions[&id];
        let allocation = AllocationV1::Uploading {
            upload_id: session.upload,
            object_locator: ObjectLocator::from_uuid(session.locator.as_uuid()),
            committed_offset: 0,
            chunk_size: koru_cloud_blob::CHUNK,
            expires_at_millis: i64::MAX,
            byte_url: format!("{}/v1/objects/uploads/{id}", self.base),
            capability: sign(&claims(
                BlobPurpose::Write,
                session.dataset,
                session.upload,
                session.locator,
                session.size,
                &session.hash,
            )),
        };
        drop(sessions);
        Box::pin(async move { Ok(allocation) })
    }

    fn renew(
        &self,
        _access: CloudAccessFacts,
        _workspace: WorkspaceId,
        upload: CloudId,
    ) -> PortFuture<'_, Result<AllocationV1, TransferError>> {
        let id = upload.as_uuid().hyphenated().to_string();
        let sessions = self.sessions.lock().expect("the double's own lock");
        let Some(session) = sessions.get(&id) else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::UploadExpired)) });
        };
        let allocation = AllocationV1::Uploading {
            upload_id: session.upload,
            object_locator: ObjectLocator::from_uuid(session.locator.as_uuid()),
            committed_offset: 0,
            chunk_size: koru_cloud_blob::CHUNK,
            expires_at_millis: i64::MAX,
            byte_url: format!("{}/v1/objects/uploads/{id}", self.base),
            capability: sign(&claims(
                BlobPurpose::Write,
                session.dataset,
                session.upload,
                session.locator,
                session.size,
                &session.hash,
            )),
        };
        drop(sessions);
        Box::pin(async move { Ok(allocation) })
    }

    fn abort(
        &self,
        _access: CloudAccessFacts,
        _workspace: WorkspaceId,
        _upload: CloudId,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        Box::pin(async move { Ok(()) })
    }

    fn read_capability(
        &self,
        _access: CloudAccessFacts,
        _workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<ReadCapabilityV1, TransferError>> {
        let held = self.published.lock().expect("the double's own lock");
        let mapped = held.get(hash.as_str()).map(|entry| {
            (
                entry.locator,
                entry.dataset,
                entry.size,
                entry.mime.clone(),
                entry.filename.clone(),
            )
        });
        drop(held);
        let mapped = mapped.or_else(|| {
            let sessions = self.sessions.lock().expect("the double's own lock");
            sessions
                .values()
                .find(|session| session.hash == hash.as_str())
                .map(|session| {
                    (
                        ObjectLocator::from_uuid(session.locator.as_uuid()),
                        session.dataset,
                        session.size,
                        "application/octet-stream".to_string(),
                        None,
                    )
                })
        });
        let Some((locator, dataset, size, mime, filename)) = mapped else {
            return Box::pin(async move { Err(TransferError::Refused(BlobCode::ObjectMissing)) });
        };
        let subject = CloudId::from_bytes(*locator.as_uuid().as_bytes());
        let answer = ReadCapabilityV1 {
            capability: sign(&claims(
                BlobPurpose::Read,
                dataset,
                subject,
                subject,
                size,
                hash.as_str(),
            )),
            byte_url: format!("{}/v1/objects/{locator}", self.base),
            size,
            mime,
            filename,
        };
        Box::pin(async move { Ok(answer) })
    }
}

// ------------------------------------------------------ a file-backed store

/// A Store double whose objects are **files**, because the in-memory one would
/// make every read a copy of the payload — which is the allocation
/// `bounded_memory.rs` exists to disprove.
pub struct FileStore {
    root: PathBuf,
    state: Mutex<FileStoreState>,
}

#[derive(Default)]
struct FileStoreState {
    documents: BTreeMap<WorkspaceId, Vec<WalkDocument>>,
    associated: BTreeMap<(WorkspaceId, String), LocalBlob>,
    installs: Vec<String>,
}

impl FileStore {
    pub fn new(root: &Path) -> Arc<Self> {
        std::fs::create_dir_all(root.join("objects")).expect("an object directory");
        std::fs::create_dir_all(root.join("blobs").join("staging")).expect("a staging directory");
        Arc::new(Self {
            root: root.to_path_buf(),
            state: Mutex::new(FileStoreState::default()),
        })
    }

    fn object_path(&self, hash: &BlobHash) -> PathBuf {
        self.root.join("objects").join(hash.file_name())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FileStoreState> {
        self.state.lock().expect("the double's own lock")
    }

    /// Write an object to this device, associated with a Workspace. The bytes
    /// are handed over once and never held.
    pub fn hold(&self, workspace: WorkspaceId, bytes: &[u8], mime: &str) -> LocalBlob {
        let hash = hash_of(bytes);
        std::fs::write(self.object_path(&hash), bytes).expect("an object");
        let blob = LocalBlob {
            hash: hash.clone(),
            size: bytes.len() as u64,
            mime: mime.to_owned(),
            filename: None,
        };
        self.lock()
            .associated
            .insert((workspace, hash.as_str().to_owned()), blob.clone());
        blob
    }

    pub fn document(&self, workspace: WorkspaceId, hashes: &[(&BlobHash, &str)]) {
        self.lock()
            .documents
            .entry(workspace)
            .or_default()
            .push(WalkDocument {
                durable: true,
                trashed: false,
                nodes: hashes
                    .iter()
                    .map(|(hash, mime)| WalkNode {
                        blob: Some(hash.as_str().to_owned()),
                        mime: Some((*mime).to_owned()),
                    })
                    .collect(),
            });
    }

    /// This device no longer holds the object and is no longer associated
    /// with it — the second machine, without a second machine.
    pub fn forget(&self, workspace: WorkspaceId, hash: &BlobHash) {
        let _ = std::fs::remove_file(self.object_path(hash));
        self.lock()
            .associated
            .remove(&(workspace, hash.as_str().to_owned()));
    }

    pub fn associated(&self, workspace: WorkspaceId, hash: &BlobHash) -> bool {
        self.lock()
            .associated
            .contains_key(&(workspace, hash.as_str().to_owned()))
    }

    pub fn installs(&self) -> Vec<String> {
        self.lock().installs.clone()
    }

    /// The same path the port reports, for a test that has to look inside the
    /// staging area a client is about to use.
    pub fn staging_root_path(&self) -> PathBuf {
        self.root.join("blobs").join("staging")
    }
}

impl BlobStorePort for FileStore {
    fn sample_documents(
        &self,
        workspace: WorkspaceId,
    ) -> PortFuture<'_, Result<koru_blob_sync::DocumentSample, TransferError>> {
        let documents = self
            .lock()
            .documents
            .get(&workspace)
            .cloned()
            .unwrap_or_default();
        Box::pin(async move {
            Ok(koru_blob_sync::DocumentSample::new(
                documents
                    .into_iter()
                    .enumerate()
                    .map(|(ordinal, content)| koru_blob_sync::SampledDocument {
                        document: koru_store_core::DocumentId::new(),
                        state: Some(koru_blob_sync::fakes::state_of(ordinal as i32 + 1)),
                        content,
                    })
                    .collect(),
            ))
        })
    }

    fn object_present(&self, hash: BlobHash) -> PortFuture<'_, Result<bool, TransferError>> {
        let present = self.object_path(&hash).exists();
        Box::pin(async move { Ok(present) })
    }

    fn workspace_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<Option<LocalBlob>, TransferError>> {
        let found = self
            .lock()
            .associated
            .get(&(workspace, hash.as_str().to_owned()))
            .cloned();
        Box::pin(async move { Ok(found) })
    }

    fn verify_object(
        &self,
        _workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        // Hashed in fixed buffers, like the real one: verifying an object must
        // not be the allocation the transfer avoided.
        let path = self.object_path(&hash);
        Box::pin(async move {
            let mut file =
                std::fs::File::open(path).map_err(|_| TransferError::Local(Code::BlobNotFound))?;
            let mut hasher = blake3::Hasher::new();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                use std::io::Read as _;
                let read = file
                    .read(&mut buffer)
                    .map_err(|_| TransferError::Local(Code::StorageFailed))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            if hasher.finalize().to_hex().as_str() == hash.digest() {
                Ok(())
            } else {
                Err(TransferError::Local(Code::BlobChecksumMismatch))
            }
        })
    }

    fn open_blob(
        &self,
        _workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<Box<dyn std::io::Read + Send>, TransferError>> {
        let path = self.object_path(&hash);
        Box::pin(async move {
            std::fs::File::open(path)
                .map(|file| Box::new(file) as Box<dyn std::io::Read + Send>)
                .map_err(|_| TransferError::Local(Code::BlobNotFound))
        })
    }

    fn install_downloaded(
        &self,
        workspace: WorkspaceId,
        blob: LocalBlob,
        staged: PathBuf,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        let object = self.object_path(&blob.hash);
        let outcome = (|| {
            // The object, by rename — no copy and no buffer, which is what a
            // real atomic install is.
            std::fs::rename(&staged, &object)
                .map_err(|_| TransferError::Local(Code::StorageFailed))?;
            let mut state = self.lock();
            state.installs.push(blob.hash.as_str().to_owned());
            // The association, last.
            state
                .associated
                .insert((workspace, blob.hash.as_str().to_owned()), blob);
            Ok(())
        })();
        Box::pin(async move { outcome })
    }

    fn staging_root(&self) -> PathBuf {
        self.root.join("blobs").join("staging")
    }

    fn release_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        let key = (workspace, hash.as_str().to_owned());
        if !self.lock().associated.contains_key(&key) {
            return Box::pin(async { Err(TransferError::Local(Code::BlobNotFound)) });
        }
        let object = self.object_path(&hash);
        // The object goes; the association stays.
        let freed = std::fs::metadata(&object)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let _ = std::fs::remove_file(&object);
        Box::pin(async move { Ok(freed) })
    }
}
