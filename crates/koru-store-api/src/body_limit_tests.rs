// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use koru_store_core::model::{BlobHash, Scope, Timestamp};
use koru_store_core::service::Store;
use koru_store_core::text::Locale;
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};
use tower::ServiceExt as _;

use super::{AppState, KoruStore, router};
use crate::routes::workspaces::receive_blob;
use crate::state::{BlobHydrationFailure, BlobHydrationService};

struct HydratesIntoStore {
    state: std::sync::Weak<AppState>,
    bytes: Vec<u8>,
}

impl BlobHydrationService for HydratesIntoStore {
    fn hydrate(
        &self,
        workspace: koru_store_core::WorkspaceId,
        expected: BlobHash,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BlobHydrationFailure>> + Send + 'static>,
    > {
        let state = self.state.clone();
        let bytes = self.bytes.clone();
        Box::pin(async move {
            let state = state.upgrade().ok_or_else(|| BlobHydrationFailure {
                code: "runtime_closed".into(),
                retryable: true,
                waiting_on: Vec::new(),
                hold: None,
            })?;
            let stored = state
                .store
                .lock()
                .map_err(|_| BlobHydrationFailure {
                    code: "internal".into(),
                    retryable: true,
                    waiting_on: Vec::new(),
                    hold: None,
                })?
                .put_blob(
                    workspace,
                    &bytes,
                    "image/png",
                    None,
                    Timestamp::from_millis(1_770_000_000_001),
                    None,
                )
                .map_err(|error| BlobHydrationFailure {
                    code: error.code().as_str().into(),
                    retryable: false,
                    waiting_on: Vec::new(),
                    hold: None,
                })?;
            if stored.hash != expected {
                return Err(BlobHydrationFailure {
                    code: "blob_checksum_mismatch".into(),
                    retryable: false,
                    waiting_on: Vec::new(),
                    hold: None,
                });
            }
            Ok(())
        })
    }
}

fn fixture() -> (
    tempfile::TempDir,
    Arc<AppState>,
    koru_store_core::ids::WorkspaceId,
    String,
) {
    let dir = tempfile::tempdir().unwrap();
    let catalog = SqliteCatalog::open(&dir.path().join("catalog.db")).unwrap();
    let mut store: KoruStore = Store::open(catalog, SqliteBackend::new(dir.path())).unwrap();
    let at = Timestamp::from_millis(1_770_000_000_000);
    let profile = store
        .create_profile("Owner", None, None, Locale::parse("en").unwrap(), at)
        .unwrap();
    let workspace = store.create_workspace(profile.id, "W", at).unwrap().id;
    let state = Arc::new(AppState::new(
        store,
        "<root>".into(),
        "body-limit-test".into(),
        at.millis(),
    ));
    let (token, _) = state.mint_session("test", Scope::ALL.to_vec());
    (dir, state, workspace, token)
}

fn authorised(
    method: &'static str,
    uri: String,
    token: &str,
    content_type: &'static str,
    body: Vec<u8>,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap()
}

/// Axum defaults buffered extractors to 2 MiB. That is not Koru's file limit:
/// OneNote and arbitrary-file import both legitimately send larger blobs.
#[tokio::test]
async fn a_blob_larger_than_axums_default_limit_reaches_the_store() {
    let (_dir, state, workspace, token) = fixture();
    let bytes = vec![0x5a; 2 * 1024 * 1024 + 1];
    let request = authorised(
        "PUT",
        format!("/v1/workspaces/{}/blobs", workspace.to_storage()),
        &token,
        "application/octet-stream",
        bytes,
    );

    let response = router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_multiframe_blob_is_streamed_in_bounded_writer_chunks() {
    use koru_store_core::ports::{CompletedBlobUpload, PendingBlobUpload};

    struct Observed(Arc<std::sync::Mutex<Vec<usize>>>);
    impl PendingBlobUpload for Observed {
        fn write_chunk(&mut self, bytes: &[u8]) -> koru_store_core::Result<()> {
            self.0.lock().unwrap().push(bytes.len());
            Ok(())
        }

        fn finish(self: Box<Self>) -> koru_store_core::Result<CompletedBlobUpload> {
            Ok(CompletedBlobUpload {
                hash: BlobHash::from_digest(BlobHash::BLAKE3, &"b".repeat(64)).unwrap(),
                size: self.0.lock().unwrap().iter().sum::<usize>() as u64,
            })
        }
    }

    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let body = Body::from_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![1_u8; 150_000])),
        Ok(axum::body::Bytes::from(vec![2_u8; 17])),
    ]));
    let completed = receive_blob(Box::new(Observed(Arc::clone(&writes))), body, 200_000)
        .await
        .unwrap();
    let writes = writes.lock().unwrap();
    assert_eq!(writes.iter().sum::<usize>(), 150_017);
    assert!(writes.len() > 2, "the input frames were not subdivided");
    assert!(writes.iter().all(|size| *size <= 64 * 1024), "{writes:?}");
    assert_eq!(completed.size, 150_017);
}

#[tokio::test]
async fn multiple_http_body_frames_arrive_as_one_exact_blob() {
    let (_dir, state, workspace, token) = fixture();
    let body = Body::from_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"one ")),
        Ok(axum::body::Bytes::from_static(b"two ")),
        Ok(axum::body::Bytes::from_static(b"three")),
    ]));
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/v1/workspaces/{}/blobs", workspace.to_storage()))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(body)
        .unwrap();

    let response = router(Arc::clone(&state)).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let blobs = state.store.lock().unwrap().blobs(workspace).unwrap();
    assert_eq!(blobs.len(), 1);
    assert_eq!(
        state
            .store
            .lock()
            .unwrap()
            .get_blob(workspace, &blobs[0].hash)
            .unwrap(),
        b"one two three"
    );
}

#[tokio::test]
async fn a_paused_blob_body_does_not_hold_the_store_mutex() {
    let (_dir, state, workspace, token) = fixture();
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(1);
    sender
        .send(Ok(axum::body::Bytes::from_static(b"first")))
        .await
        .unwrap();
    let (paused, body_paused) = tokio::sync::oneshot::channel();
    let stream = futures_util::stream::unfold(
        (receiver, Some(paused), false),
        |(mut receiver, mut paused, consumed_first)| async move {
            if consumed_first && let Some(paused) = paused.take() {
                let _ = paused.send(());
            }
            receiver
                .recv()
                .await
                .map(|item| (item, (receiver, paused, true)))
        },
    );
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/v1/workspaces/{}/blobs", workspace.to_storage()))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from_stream(stream))
        .unwrap();
    let upload = tokio::spawn(router(Arc::clone(&state)).oneshot(request));

    // A .part file appears inside begin_blob_upload, before its Store guard
    // drops. Observe the body waiting for its next chunk instead of racing
    // that file creation against try_lock.
    tokio::time::timeout(std::time::Duration::from_secs(5), body_paused)
        .await
        .expect("the request did not pause on its body")
        .expect("the body stream ended before pausing");
    let store = state
        .store
        .try_lock()
        .expect("a paused request retained the Store mutex");
    assert_eq!(store.workspace(workspace).unwrap().id, workspace);
    drop(store);

    sender
        .send(Ok(axum::body::Bytes::from_static(b" second")))
        .await
        .unwrap();
    drop(sender);
    assert_eq!(upload.await.unwrap().unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn a_disconnected_or_over_limit_body_leaves_no_part_and_no_metadata() {
    use koru_store_core::ports::{CompletedBlobUpload, PendingBlobUpload};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Lifecycle {
        dropped_unfinished: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        published: bool,
    }
    impl PendingBlobUpload for Lifecycle {
        fn write_chunk(&mut self, _: &[u8]) -> koru_store_core::Result<()> {
            Ok(())
        }

        fn finish(mut self: Box<Self>) -> koru_store_core::Result<CompletedBlobUpload> {
            self.published = true;
            self.finished.store(true, Ordering::SeqCst);
            Ok(CompletedBlobUpload {
                hash: BlobHash::from_digest(BlobHash::BLAKE3, &"c".repeat(64)).unwrap(),
                size: 0,
            })
        }
    }
    impl Drop for Lifecycle {
        fn drop(&mut self) {
            if !self.published {
                self.dropped_unfinished.store(true, Ordering::SeqCst);
            }
        }
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let pending = Box::new(Lifecycle {
        dropped_unfinished: Arc::clone(&dropped),
        finished: Arc::clone(&finished),
        published: false,
    });
    let disconnected = Body::from_stream(futures_util::stream::iter([
        Ok(axum::body::Bytes::from_static(b"partial")),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "gone",
        )),
    ]));
    assert!(receive_blob(pending, disconnected, 100).await.is_err());
    assert!(dropped.load(Ordering::SeqCst));
    assert!(!finished.load(Ordering::SeqCst));

    let dropped = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let pending = Box::new(Lifecycle {
        dropped_unfinished: Arc::clone(&dropped),
        finished: Arc::clone(&finished),
        published: false,
    });
    let too_large = Body::from_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"four")),
        Ok(axum::body::Bytes::from_static(b"!")),
    ]));
    let error = receive_blob(pending, too_large, 4).await.unwrap_err();
    assert_eq!(error.0.code(), koru_store_core::Code::PayloadTooLarge);
    assert!(dropped.load(Ordering::SeqCst));
    assert!(!finished.load(Ordering::SeqCst));
}

/// Imported snapshots travel as JSON arrays of bytes, which can cross 2 MiB
/// even when their opaque Loro payload is much smaller. The route has its own
/// bounded ceiling rather than inheriting Axum's unrelated default.
#[tokio::test]
async fn a_large_imported_snapshot_reaches_the_store() {
    let (_dir, state, workspace, token) = fixture();
    let body = serde_json::to_vec(&serde_json::json!({
        "title": "Large import",
        "kind": "page",
        "parent": null,
        "snapshot": vec![0_u8; 1_100_000],
    }))
    .unwrap();
    assert!(body.len() > 2 * 1024 * 1024);
    let request = authorised(
        "POST",
        format!("/v1/workspaces/{}/documents", workspace.to_storage()),
        &token,
        "application/json",
        body,
    );

    let response = router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_missing_blob_is_hydrated_then_returned_from_local_store() {
    let (_dir, state, workspace, token) = fixture();
    let bytes = b"peer image bytes".to_vec();
    let hash = BlobHash::from_digest(
        BlobHash::BLAKE3,
        &koru_store_core::lifecycle::checksum(&bytes),
    )
    .unwrap();
    assert_eq!(
        state
            .store
            .lock()
            .unwrap()
            .get_blob(workspace, &hash)
            .unwrap_err()
            .code(),
        koru_store_core::Code::BlobNotFound
    );
    assert!(
        state.set_blob_hydration_service(Arc::new(HydratesIntoStore {
            state: Arc::downgrade(&state),
            bytes: bytes.clone(),
        }))
    );
    let request = authorised(
        "GET",
        format!(
            "/v1/workspaces/{}/blobs/{}",
            workspace.to_storage(),
            hash.as_str()
        ),
        &token,
        "application/octet-stream",
        Vec::new(),
    );

    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), bytes);
    assert_eq!(
        state
            .store
            .lock()
            .unwrap()
            .get_blob(workspace, &hash)
            .unwrap(),
        bytes,
        "the response must come from verified local persistence"
    );
}
