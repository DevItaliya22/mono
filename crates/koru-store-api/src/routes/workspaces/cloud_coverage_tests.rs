// SPDX-License-Identifier: MPL-2.0

//! `GET /v1/workspaces/{id}/documents/{document}/cloud-sync/coverage`,
//! through the router against a real store under a temporary directory, with
//! an engine that answers a fixed report rather than moving anything.
//!
//! **What only this layer decides** is which document the answer is about,
//! the scope it asks for, and that a store with no engine says so instead of
//! answering an empty report. The counting itself is the engine's and is
//! pinned in `koru-blob-sync`.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use koru_store_core::ids::{DocumentId, WorkspaceId};
use koru_store_core::model::{Scope, Timestamp};
use koru_store_core::service::Store;
use koru_store_core::text::Locale;
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};
use koru_store_wire::internal::{CloudDocumentCoverage, CloudDocumentFiles};
use tower::ServiceExt as _;

use crate::router;
use crate::state::{AppState, BlobTransferStatusService, KoruStore};

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

/// Answers what it was given, and records who it was asked about — a read
/// must be a read, about the document in the path.
#[derive(Default)]
struct FixedEngine {
    answer: Mutex<CloudDocumentCoverage>,
    asked: Mutex<Vec<(WorkspaceId, DocumentId)>>,
}

impl BlobTransferStatusService for FixedEngine {
    fn status(&self, _workspace: WorkspaceId) -> koru_store_wire::internal::BlobTransferReport {
        unreachable!("this route does not read the workspace's counts")
    }

    fn set_paused(
        &self,
        _workspace: WorkspaceId,
        _paused: bool,
    ) -> koru_store_wire::internal::BlobTransferReport {
        unreachable!("this route pauses nothing")
    }

    fn enabled(&self, _workspace: WorkspaceId) {
        unreachable!("this route is not a trigger")
    }

    fn file(
        &self,
        _workspace: WorkspaceId,
        _file: DocumentId,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<koru_store_wire::internal::BlobFileReport>>
                + Send
                + '_,
        >,
    > {
        unreachable!("this route explains no selected file")
    }

    fn release_local(
        &self,
        _workspace: WorkspaceId,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<koru_store_wire::internal::BlobReleaseReport, &'static str>,
                > + Send
                + '_,
        >,
    > {
        unreachable!("this route releases nothing")
    }

    fn waiting(&self, _workspace: WorkspaceId) -> crate::state::WaitingFuture<'_> {
        unreachable!("this route asks about one document")
    }

    fn document_coverage(
        &self,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> crate::state::DocumentCoverageFuture<'_> {
        self.asked.lock().unwrap().push((workspace, document));
        let answer = self.answer.lock().unwrap().clone();
        Box::pin(async move { Ok(answer) })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState>,
    workspace: WorkspaceId,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let catalog = SqliteCatalog::open(&dir.path().join("catalog.db")).unwrap();
    let mut store: KoruStore = Store::open(catalog, SqliteBackend::new(dir.path())).unwrap();
    let profile = store
        .create_profile("Owner", None, None, Locale::parse("en").unwrap(), at())
        .unwrap();
    let workspace = store
        .create_workspace(profile.id, "Photos", at())
        .unwrap()
        .id;
    let state = Arc::new(AppState::new(
        store,
        "<root>".into(),
        "cloud-coverage-route-test".into(),
        at().millis(),
    ));
    Fixture {
        _dir: dir,
        state,
        workspace,
    }
}

impl Fixture {
    fn engine(&self) -> Arc<FixedEngine> {
        let engine = Arc::new(FixedEngine::default());
        assert!(self.state.set_blob_transfer_status_service(engine.clone()));
        engine
    }

    async fn ask(&self, document: DocumentId, scopes: &[Scope]) -> (StatusCode, serde_json::Value) {
        let (token, _) = self.state.mint_session("test", scopes.to_vec());
        let request = Request::builder()
            .method("GET")
            .uri(format!(
                "/v1/workspaces/{}/documents/{}/cloud-sync/coverage",
                self.workspace.to_storage(),
                document.to_storage()
            ))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = router(self.state.clone())
            .oneshot(request)
            .await
            .expect("the router answers");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }
}

/// **The engine's counts, about the document in the path** — asked once, and
/// answered with the workspace and document it is about so a stale answer is
/// recognisable.
#[tokio::test]
async fn a_documents_coverage_is_the_engines_answer_about_that_document() {
    let fixture = fixture();
    let engine = fixture.engine();
    *engine.answer.lock().unwrap() = CloudDocumentCoverage {
        synced: Some(true),
        files: Some(CloudDocumentFiles {
            images_total: 7,
            images_synced: 7,
            files_total: 1,
            files_synced: 1,
        }),
        ..CloudDocumentCoverage::default()
    };
    let document = DocumentId::new();
    let (status, body) = fixture.ask(document, &[Scope::ReadDocuments]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workspace"], fixture.workspace.to_storage());
    assert_eq!(body["document"], document.to_storage());
    assert_eq!(body["synced"], true);
    assert_eq!(body["files"]["images_total"], 7);
    assert_eq!(body["files"]["images_synced"], 7);
    assert_eq!(body["files"]["files_total"], 1);
    assert_eq!(
        *engine.asked.lock().unwrap(),
        vec![(fixture.workspace, document)]
    );
}

/// *Not measured* travels as absent, never as `false` or zero.
#[tokio::test]
async fn an_unmeasured_document_answers_absent_rather_than_no() {
    let fixture = fixture();
    fixture.engine();
    let (status, body) = fixture
        .ask(DocumentId::new(), &[Scope::ReadDocuments])
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["synced"].is_null(), "{body}");
    assert!(body["files"].is_null(), "{body}");
}

/// A store with no blob engine says so: *nothing to measure here* is not an
/// empty report.
#[tokio::test]
async fn a_store_with_no_engine_refuses_rather_than_answering_empty() {
    let fixture = fixture();
    let (status, body) = fixture
        .ask(DocumentId::new(), &[Scope::ReadDocuments])
        .await;
    assert!(!status.is_success(), "{status} {body}");
    assert_eq!(body["code"], "blob_transfer_unavailable", "{body}");
}

/// Reading one document's coverage needs the scope that reads documents.
#[tokio::test]
async fn reading_a_documents_coverage_needs_read_documents() {
    let fixture = fixture();
    let engine = fixture.engine();
    let (status, body) = fixture
        .ask(DocumentId::new(), &[Scope::ManageWorkspaces])
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(engine.asked.lock().unwrap().is_empty());
}
