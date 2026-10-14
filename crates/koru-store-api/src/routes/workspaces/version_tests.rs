// SPDX-License-Identifier: MPL-2.0

//! `GET /v1/workspaces/{id}/versions/{version}/snapshot` through the
//! router, against a real store under a temporary directory: the bytes a
//! version id names, unmoved by an edit to the document afterwards, and a
//! stable code for a version the workspace does not hold.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use koru_store_core::ids::{DocumentId, DocumentVersionId, WorkspaceId};
use koru_store_core::model::{Scope, Timestamp};
use koru_store_core::ports::HistoryReason;
use koru_store_core::service::Store;
use koru_store_core::text::Locale;
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};
use tower::ServiceExt as _;

use crate::router;
use crate::state::{AppState, KoruStore};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000 + ms)
}

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState>,
    workspace: WorkspaceId,
    token: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let catalog = SqliteCatalog::open(&dir.path().join("catalog.db")).unwrap();
    let mut store: KoruStore = Store::open(catalog, SqliteBackend::new(dir.path())).unwrap();
    let profile = store
        .create_profile("Owner", None, None, Locale::parse("en").unwrap(), at(0))
        .unwrap();
    let workspace = store.create_workspace(profile.id, "W", at(0)).unwrap().id;
    let state = Arc::new(AppState::new(
        store,
        "<root>".into(),
        "version-route-test".into(),
        at(0).millis(),
    ));
    let (token, _) = state.mint_session("test", Scope::ALL.to_vec());
    Fixture {
        _dir: dir,
        state,
        workspace,
        token,
    }
}

impl Fixture {
    fn store(&self) -> std::sync::MutexGuard<'_, KoruStore> {
        self.state.store.lock().unwrap()
    }

    /// A document holding one line of text, imported the way a client's
    /// snapshot reaches the store.
    fn a_document(&self, text: &str) -> DocumentId {
        let doc = loro::LoroDoc::new();
        doc.get_text("body").insert(0, text).unwrap();
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        self.store()
            .import_document(self.workspace, "title", "page", None, snapshot, at(1))
            .unwrap()
            .0
    }

    fn capture(&self, document: DocumentId) -> DocumentVersionId {
        self.store()
            .capture_history(
                self.workspace,
                document,
                None,
                HistoryReason::Manual,
                at(2),
                None,
                None,
                None,
            )
            .unwrap()
            .version
    }

    /// Edit the document after the capture, as an editor's replica would.
    fn edit(&self, document: DocumentId, text: &str) {
        let fork = self
            .store()
            .fork_document(self.workspace, document)
            .unwrap();
        let before = fork.oplog_vv();
        fork.get_text("body").insert(0, text).unwrap();
        let delta = fork.export(loro::ExportMode::updates(&before)).unwrap();
        self.store()
            .apply_update(self.workspace, document, delta, at(3), None)
            .unwrap();
    }

    async fn get(&self, workspace: WorkspaceId, version: &str) -> axum::response::Response {
        let request = Request::builder()
            .method("GET")
            .uri(format!(
                "/v1/workspaces/{}/versions/{version}/snapshot",
                workspace.to_storage()
            ))
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .body(Body::empty())
            .unwrap();
        router(Arc::clone(&self.state))
            .oneshot(request)
            .await
            .unwrap()
    }
}

fn text_of(snapshot: &[u8]) -> String {
    let doc = loro::LoroDoc::new();
    doc.import(snapshot).unwrap();
    doc.get_text("body").to_string()
}

async fn bytes(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// UC2 at the route: the version answers the bytes it was captured with,
/// named by document, checksum and moment, however the document changed
/// since.
#[tokio::test]
async fn a_pinned_version_answers_its_captured_bytes_after_the_source_is_edited() {
    let f = fixture();
    let document = f.a_document("captured");
    let version = f.capture(document);
    f.edit(document, "edited then ");

    let response = f.get(f.workspace, &version.to_storage()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
    assert_eq!(
        headers.get("x-koru-document-id").unwrap(),
        document.to_storage().as_str()
    );
    let expected = f
        .store()
        .document_version(f.workspace, version)
        .unwrap()
        .expect("the version is held");
    assert_eq!(
        headers.get("x-koru-checksum").unwrap(),
        expected.checksum.as_str()
    );
    assert_eq!(
        headers.get("x-koru-captured-at").unwrap(),
        at(2).millis().to_string().as_str()
    );
    let body = bytes(response).await;
    assert_eq!(body, expected.snapshot);
    assert_eq!(text_of(&body), "captured");
    // The live document moved on; the version did not.
    let live = f.store().snapshot(f.workspace, document).unwrap();
    assert_eq!(text_of(&live), "edited then captured");
}

#[tokio::test]
async fn a_version_the_workspace_does_not_hold_answers_a_stable_code() {
    let f = fixture();
    let response = f
        .get(f.workspace, &DocumentVersionId::new().to_storage())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = String::from_utf8(bytes(response).await).unwrap();
    assert!(
        body.contains("\"code\":\"document_version_not_found\""),
        "{body}"
    );
}

/// Scoped to the workspace's own repository: another workspace's version
/// id is not found rather than served.
#[tokio::test]
async fn another_workspaces_version_is_not_found_here() {
    let f = fixture();
    let document = f.a_document("elsewhere");
    let version = f.capture(document);
    let other = {
        let mut store = f.store();
        let profile = store
            .create_profile("Other", None, None, Locale::parse("en").unwrap(), at(4))
            .unwrap();
        store.create_workspace(profile.id, "O", at(4)).unwrap().id
    };
    let response = f.get(other, &version.to_storage()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = f.get(f.workspace, &version.to_storage()).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_malformed_version_id_is_refused_as_invalid() {
    let f = fixture();
    let response = f.get(f.workspace, "not-a-version").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
