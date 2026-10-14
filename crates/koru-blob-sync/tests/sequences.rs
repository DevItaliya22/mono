// SPDX-License-Identifier: MPL-2.0

//! **S10 and S11, over HTTP, against the in-process object service.**
//!
//! These two sequences are the client's. They run over a real socket rather
//! than against an in-memory plane, because the thing being asserted is an
//! ordering *across* two processes — a client that hashes what a service
//! actually served, and a client that resumes from bytes a previous process
//! actually synced. The service is [`common::ObjectService`], held to the
//! contract vectors in `object_service.rs`; the same suite runs against the
//! hosted service's real object worker, which is not in this repository.
//!
//! ```text
//! S10  read_capability → get:range:0 → kill:client → restart:client
//!      → get:range:resume → hash:ok → install_object → write_association → announce
//!      association_before_bytes: false      resends: 0
//!
//! S11  read_capability → get:full → hash:mismatch → quarantine
//!      association_before_bytes: false      renders: false
//! ```

mod common;

use std::sync::{Arc, Mutex};

use koru_account_core::workspace_authorization::Region;
use koru_blob_sync::{
    AdmittedUrl, BlobEngine, BlobObjectPlane, BlobPorts, ByteOrigins, HttpObjectPlane, PortFuture,
    StagingArea, TransferError, Trigger,
    fakes::{FakeAccess, RecordingDownloads, SwitchablePolicy},
};
use koru_cloud_blob::BlobCode;
use koru_store_core::WorkspaceId;

use common::{FileStore, ObjectService, Published, TestControl, hash_of, publish};

/// The real HTTP object plane, with every range it asked for written down.
///
/// `resends: 0` is a claim about which bytes crossed the wire, so it is
/// measured here rather than inferred from the file that came out the far end.
struct CountingObjects {
    inner: HttpObjectPlane,
    ranges: Mutex<Vec<(u64, u64)>>,
}

impl CountingObjects {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: HttpObjectPlane::new().expect("an http object plane"),
            ranges: Mutex::new(Vec::new()),
        })
    }
    fn ranges(&self) -> Vec<(u64, u64)> {
        self.ranges.lock().expect("the counter's own lock").clone()
    }
}

impl BlobObjectPlane for CountingObjects {
    fn committed_offset(
        &self,
        url: AdmittedUrl,
        capability: String,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        self.inner.committed_offset(url, capability)
    }
    fn patch(
        &self,
        url: AdmittedUrl,
        capability: String,
        start: u64,
        total: u64,
        body: Vec<u8>,
    ) -> PortFuture<'_, Result<u64, TransferError>> {
        self.inner.patch(url, capability, start, total, body)
    }
    fn complete(
        &self,
        url: AdmittedUrl,
        capability: String,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        self.inner.complete(url, capability)
    }
    fn read_range(
        &self,
        url: AdmittedUrl,
        capability: String,
        start: u64,
        length: u64,
    ) -> PortFuture<'_, Result<Vec<u8>, TransferError>> {
        self.ranges
            .lock()
            .expect("the counter's own lock")
            .push((start, length));
        self.inner.read_range(url, capability, start, length)
    }
}

struct Client {
    engine: Arc<BlobEngine>,
    store: Arc<FileStore>,
    objects: Arc<CountingObjects>,
    downloads: Arc<RecordingDownloads>,
    staging: StagingArea,
}

/// A client whose staging lives under `root`, so "restart the client" is a
/// second client over the same directory — which is what a restart is.
fn client(
    root: &std::path::Path,
    service_base: &str,
    control: Arc<TestControl>,
    origins: ByteOrigins,
) -> Client {
    let store = FileStore::new(root);
    let objects = CountingObjects::new();
    let downloads = Arc::new(RecordingDownloads::default());
    let engine = BlobEngine::new(
        BlobPorts {
            store: store.clone(),
            access: FakeAccess::new(origins),
            policy: Arc::new(SwitchablePolicy::permitting()),
            // A Cloud Workspace: what these lanes are about is the wire
            // contract, and §6's gate has its own coverage beside the engine.
            modes: koru_blob_sync::fakes::SwitchableModes::cloud(),
            // No peer path in these lanes: they drive the object service.
            peers: std::sync::Arc::new(koru_blob_sync::NoPeerBlobs),
            downloads: downloads.clone(),
        },
        control,
        objects.clone(),
    );
    let _ = service_base;
    let staging = StagingArea::new(store.staging_root_path());
    Client {
        engine,
        store,
        objects,
        downloads,
        staging,
    }
}

/// The local stack's origin, named explicitly. The production set does not
/// hold a loopback address and must not.
fn local_origins(service_base: &str) -> ByteOrigins {
    ByteOrigins::none()
        .with_development_origins(service_base)
        .expect("a local origin")
}

fn picture(bytes: usize) -> Vec<u8> {
    (0..bytes).map(|index| (index % 251) as u8).collect()
}

// ------------------------------------------------------------------- S10

/// **S10.** A download killed mid-transfer resumes from the synced length,
/// re-sends nothing, hashes at exact length, installs the object, writes the
/// association last, and announces.
#[tokio::test(flavor = "multi_thread")]
async fn s10_a_killed_download_resumes_and_associates_last() {
    let service = ObjectService::start().await;
    // Three chunks and a bit, so a resume lands mid-object rather than on a
    // boundary an implementation might handle specially.
    let bytes = picture(koru_cloud_blob::CHUNK as usize * 3 + 1_234);
    let hash = hash_of(&bytes);
    let (locator, dataset) = publish(&service, &bytes).await;

    let control = TestControl::new(&service.base);
    control.publish(
        &hash,
        Published {
            locator,
            dataset,
            size: bytes.len() as u64,
            mime: "image/png".into(),
            filename: Some("shot.png".into()),
        },
    );

    let root = tempfile::tempdir().expect("a private root");
    let workspace = WorkspaceId::new();
    let origins = local_origins(&service.base);

    // --- the first client: it syncs one chunk and dies.
    let first = client(root.path(), &service.base, control.clone(), origins.clone());
    first.staging.ensure().expect("staging");
    let prefix = koru_cloud_blob::CHUNK as usize;
    first
        .staging
        .append(&hash, 0, &bytes[..prefix])
        .expect("a synced chunk");
    drop(first);

    // --- the second client, over the same directory. This is the restart.
    let resumed = client(root.path(), &service.base, control, origins);
    resumed.store.document(workspace, &[(&hash, "image/png")]);
    let pass = resumed.engine.reconcile(workspace, Trigger::Recovery).await;
    assert_eq!(pass.downloaded, 1, "{pass:?}");
    assert_eq!(pass.session_refusal, None);

    // **`resends: 0`** — every range asked for starts where the last one
    // ended, and the first starts at the prefix the previous process synced.
    let ranges = resumed.objects.ranges();
    assert!(!ranges.is_empty());
    let mut expected = prefix as u64;
    for (start, length) in &ranges {
        assert_eq!(
            *start, expected,
            "a range re-sent bytes or left a gap: {ranges:?}"
        );
        expected += length;
    }
    assert_eq!(expected, bytes.len() as u64);

    // The object is whole, the association exists, and the staging file is
    // gone because it became the object.
    assert!(resumed.store.associated(workspace, &hash));
    assert_eq!(
        resumed.store.installs(),
        vec![hash.as_str().to_owned()],
        "association_before_bytes"
    );
    assert_eq!(resumed.staging.synced_length(&hash).expect("staging"), 0);

    // **Identity, never bytes.**
    assert_eq!(
        *resumed
            .downloads
            .arrived
            .lock()
            .expect("the recorder's own lock"),
        vec![hash.as_str().to_owned()]
    );
    service.shutdown().await;
}

// ------------------------------------------------------------------- S11

/// **S11.** A download whose bytes do not hash is quarantined, never rendered
/// and never associated.
///
/// The service really serves the bytes it really holds; what is wrong is the
/// mapping, which names an address those bytes do not have. That is the only
/// honest way to make a service serve wrong bytes without corrupting a volume
/// by hand, and it is also the real-world shape of the fault: a mapping
/// pointing at the wrong object.
#[tokio::test(flavor = "multi_thread")]
async fn s11_bytes_that_do_not_hash_are_quarantined_and_never_associated() {
    let service = ObjectService::start().await;
    let served = picture(9_000);
    let (locator, dataset) = publish(&service, &served).await;

    // The address the client asked for, which those bytes do not have.
    let asked_for = hash_of(b"a completely different picture");
    let control = TestControl::new(&service.base);
    control.publish(
        &asked_for,
        Published {
            locator,
            dataset,
            size: served.len() as u64,
            mime: "image/png".into(),
            filename: None,
        },
    );

    let root = tempfile::tempdir().expect("a private root");
    let workspace = WorkspaceId::new();
    let client = client(
        root.path(),
        &service.base,
        control,
        local_origins(&service.base),
    );
    client
        .store
        .document(workspace, &[(&asked_for, "image/png")]);

    // Asked for, or the pass would not try (D260) and there would be
    // nothing to quarantine.
    client.downloads.watch(&asked_for);
    let pass = client.engine.reconcile(workspace, Trigger::Enabled).await;

    assert_eq!(pass.downloaded, 0);
    assert_eq!(
        pass.parked,
        vec![(asked_for.clone(), BlobCode::HashMismatch.as_str())],
        "{pass:?}"
    );
    // `association_before_bytes: false`, and `renders: false`.
    assert!(!client.store.associated(workspace, &asked_for));
    assert!(client.store.installs().is_empty());
    assert!(
        client
            .downloads
            .arrived
            .lock()
            .expect("the recorder's own lock")
            .is_empty(),
        "a view was told about bytes that were never published"
    );

    // Quarantined: out of staging, so a resume cannot append to known-bad
    // bytes, and still on the disk, because a person may want to know what
    // arrived.
    assert_eq!(
        client.staging.synced_length(&asked_for).expect("staging"),
        0
    );
    let quarantined: Vec<_> = std::fs::read_dir(client.staging.quarantine_dir())
        .expect("a quarantine directory")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(quarantined.len(), 1, "nothing was set aside");
    assert_eq!(
        std::fs::metadata(quarantined[0].path())
            .expect("the quarantined file")
            .len(),
        served.len() as u64,
        "the quarantined bytes are not what arrived"
    );

    // The pass did not end: a refusal about one blob defers that blob.
    assert_eq!(pass.session_refusal, None);
    service.shutdown().await;
}

/// **A returned byte URL outside the allowlist is refused**, against the real
/// HTTP plane rather than a double that could only ever refuse politely — and
/// no socket is opened to find out.
#[tokio::test(flavor = "multi_thread")]
async fn a_returned_byte_url_outside_the_allowlist_never_reaches_a_socket() {
    let service = ObjectService::start().await;
    let bytes = picture(4_096);
    let hash = hash_of(&bytes);
    let (locator, dataset) = publish(&service, &bytes).await;

    let control = TestControl::new(&service.base);
    control.publish(
        &hash,
        Published {
            locator,
            dataset,
            size: bytes.len() as u64,
            mime: "image/png".into(),
            filename: None,
        },
    );

    let root = tempfile::tempdir().expect("a private root");
    let workspace = WorkspaceId::new();
    // The production US set. It does not hold a loopback origin, and must not.
    let client = client(
        root.path(),
        &service.base,
        control,
        ByteOrigins::for_region(Region::Us),
    );
    client.store.document(workspace, &[(&hash, "image/png")]);

    // On screen, so the pass tries to fetch it at all (D260).
    client.downloads.watch(&hash);
    let pass = client.engine.reconcile(workspace, Trigger::Enabled).await;
    assert_eq!(pass.session_refusal, Some("byte_origin_refused"));
    assert!(
        client.objects.ranges().is_empty(),
        "a byte request was made to a host this device does not hold"
    );
    assert!(!client.store.associated(workspace, &hash));
    service.shutdown().await;
}

/// **An upload driven end to end through the real byte routes**: allocate,
/// stream bounded chunks, complete, and poll to durable — where "durable"
/// means the service's verification job published the object and its own byte
/// route will serve it.
///
/// The control plane here is still a double; the same upload against the
/// **real** control routes and a real database is a test of the hosted
/// public API, which is not in this repository.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_reaches_the_object_service_and_becomes_readable() {
    let service = ObjectService::start().await;
    let control = TestControl::new(&service.base).probing();
    let root = tempfile::tempdir().expect("a private root");
    let workspace = WorkspaceId::new();
    let client = client(
        root.path(),
        &service.base,
        control,
        local_origins(&service.base),
    );

    // Two chunks and a remainder, so the final short chunk is exercised.
    let bytes = picture(koru_cloud_blob::CHUNK as usize * 2 + 77);
    let blob = client.store.hold(workspace, &bytes, "image/png");
    client
        .store
        .document(workspace, &[(&blob.hash, "image/png")]);

    let pass = client.engine.reconcile(workspace, Trigger::Enabled).await;
    assert_eq!(pass.uploaded, 1, "{pass:?}");
    assert_eq!(pass.session_refusal, None);
    assert!(pass.parked.is_empty());

    let status = client.engine.status(workspace);
    assert_eq!(status.referenced, 1);
    assert_eq!(status.bytes_complete, bytes.len() as u64);

    // A second pass finds it already there and sends nothing.
    let again = client
        .engine
        .reconcile(workspace, Trigger::MappingChanged)
        .await;
    assert_eq!(again.already_durable, 1, "{again:?}");
    service.shutdown().await;
}
