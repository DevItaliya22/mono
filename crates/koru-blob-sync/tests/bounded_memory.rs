// SPDX-License-Identifier: MPL-2.0

//! **Bounded memory, measured rather than asserted.**
//!
//! The claim this file exists to prove is that neither half of a transfer
//! allocates in proportion to the size of the transfer — that the cost of
//! moving a person's 2 GiB file, on the *client*, is the chunk and the stream
//! buffer and not the file. The hosted object worker (not in this repository)
//! proves the same thing for the service; this is the other end of the same
//! wire, and it is the end that runs on a phone.
//!
//! The service here is the in-process [`common::ObjectService`], which shares
//! this process and so is counted too; it holds at most a chunk per request
//! and streams downloads in fixed buffers, so it cannot hide a client that
//! grows with the payload — it can only add to the peak. The same measurement
//! against the real worker belongs to the hosted object worker, which is not
//! in this repository.
//!
//! It lives in its own test binary on purpose. A counting global allocator is
//! process-wide, so a measurement sharing a process with eight other test
//! threads would be measuring them too. Here there is one test, and the only
//! allocations are the ones this transfer caused.
//!
//! **The assertion is a ratio, not a wall clock** (`PRINCIPLES.md`): the same
//! transfer is run at one size and at four times that size, and the peak must
//! not follow. A byte budget alone would pass on a machine with a generous
//! allocator and fail on a busy one; a ratio says the thing we actually mean.

mod common;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use koru_blob_sync::{
    BlobEngine, BlobPorts, ByteOrigins, HttpObjectPlane, Trigger,
    fakes::{FakeAccess, RecordingDownloads, SwitchablePolicy},
};
use koru_store_core::WorkspaceId;

use common::{FileStore, ObjectService, TestControl};

// ---------------------------------------------------------------- the measure

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: every method forwards to the system allocator with the same layout
// and pointer it was given; the counters are the only addition and they are
// plain relaxed atomics that cannot affect the allocation itself.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size() as isize);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            record(new_size as isize - layout.size() as isize);
        }
        moved
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size() as isize);
        }
        pointer
    }
}

fn record(delta: isize) {
    let live = if delta >= 0 {
        LIVE.fetch_add(delta as usize, Ordering::Relaxed) + delta as usize
    } else {
        LIVE.fetch_sub((-delta) as usize, Ordering::Relaxed)
            .saturating_sub((-delta) as usize)
    };
    PEAK.fetch_max(live, Ordering::Relaxed);
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Start a measurement window at the current live total, and return the peak
/// *above* that baseline once the window closes.
struct Window(usize);

impl Window {
    fn open() -> Self {
        let live = LIVE.load(Ordering::Relaxed);
        PEAK.store(live, Ordering::Relaxed);
        Self(live)
    }
    fn peak_above_baseline(&self) -> usize {
        PEAK.load(Ordering::Relaxed).saturating_sub(self.0)
    }
}

// ---------------------------------------------------------------- the harness

/// One round trip at `size`: the engine uploads the local object through the
/// real byte routes, the device forgets it, and the engine fetches it back.
/// Returns the peak live allocation each half cost, above its own baseline.
async fn round_trip(
    service: &ObjectService,
    control: Arc<TestControl>,
    size: usize,
) -> (usize, usize) {
    let root = tempfile::tempdir().expect("a private root");
    let store = FileStore::new(root.path());
    let workspace = WorkspaceId::new();
    let downloads = Arc::new(RecordingDownloads::default());
    let engine = BlobEngine::new(
        BlobPorts {
            store: store.clone(),
            access: FakeAccess::new(
                ByteOrigins::none()
                    .with_development_origins(&service.base)
                    .expect("a local origin"),
            ),
            policy: Arc::new(SwitchablePolicy::permitting()),
            // A Cloud Workspace: what these lanes are about is the wire
            // contract, and §6's gate has its own coverage beside the engine.
            modes: koru_blob_sync::fakes::SwitchableModes::cloud(),
            // No peer path in these lanes: they drive the object service.
            peers: std::sync::Arc::new(koru_blob_sync::NoPeerBlobs),
            downloads: downloads.clone(),
        },
        control,
        Arc::new(HttpObjectPlane::new().expect("an http object plane")),
    );

    // The payload is built and written to disk **before** the window opens:
    // holding the bytes in order to create the file is the test's cost, not
    // the transfer's, and it is exactly the allocation this measurement would
    // otherwise be swamped by.
    let bytes: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
    let blob = store.hold(workspace, &bytes, "application/octet-stream");
    store.document(workspace, &[(&blob.hash, "application/octet-stream")]);
    let hash = blob.hash.clone();
    drop(bytes);

    let uploading = Window::open();
    let pass = engine.reconcile(workspace, Trigger::Enabled).await;
    let upload_peak = uploading.peak_above_baseline();
    assert_eq!(pass.uploaded, 1, "{pass:?}");

    // The device forgets it: no association, no object. This is the second
    // machine, without a second machine.
    store.forget(workspace, &hash);

    // A document showing it is on screen, or the pass would leave it on
    // Cloud (D260) and measure nothing.
    downloads.watch(&hash);
    let downloading = Window::open();
    let pass = engine.reconcile(workspace, Trigger::MappingChanged).await;
    let download_peak = downloading.peak_above_baseline();
    assert_eq!(pass.downloaded, 1, "{pass:?}");
    assert!(store.associated(workspace, &hash));

    (upload_peak, download_peak)
}

/// Neither half of a transfer allocates in proportion to the transfer.
///
/// Run at one size and at four times it. A peak that followed the payload
/// would quadruple; a bounded one barely moves.
#[tokio::test(flavor = "multi_thread")]
async fn neither_an_upload_nor_a_download_allocates_in_proportion_to_the_transfer() {
    let service = ObjectService::start().await;
    let control = TestControl::new(&service.base).probing();

    // A first pass so every buffer, connection and allocation arena the
    // runtime is going to want has already been taken. Measuring a cold
    // process would be measuring its startup.
    let _ = round_trip(&service, control.clone(), koru_cloud_blob::CHUNK as usize).await;

    let small = 4 * koru_cloud_blob::CHUNK as usize;
    let large = 4 * small;
    let (small_upload, small_download) = round_trip(&service, control.clone(), small).await;
    let (large_upload, large_download) = round_trip(&service, control, large).await;

    // The measurement itself, printed so a person reading a CI log sees the
    // numbers rather than only that an assertion held.
    eprintln!(
        "bounded_memory client upload={small}B:{small_upload}B {large}B:{large_upload}B  \
         download={small}B:{small_download}B {large}B:{large_download}B \
         chunk={}B",
        koru_cloud_blob::CHUNK
    );

    for (name, small_peak, large_peak) in [
        ("upload", small_upload, large_upload),
        ("download", small_download, large_download),
    ] {
        // Four times the payload. A peak that followed it would be four times
        // too.
        assert!(
            large_peak < small_peak + large / 2,
            "{name}: peak grew with the payload — {small_peak} bytes for {small}, \
             {large_peak} for {large}"
        );
        // And an absolute bound, because a ratio alone would be satisfied by
        // two equally terrible numbers: one chunk in flight, the framework's
        // headroom around it, and nothing that scales with the object.
        assert!(
            large_peak < 16 * koru_cloud_blob::CHUNK as usize,
            "{name}: {large_peak} bytes peak for a {large}-byte transfer"
        );
    }

    service.shutdown().await;
}
