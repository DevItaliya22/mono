// SPDX-License-Identifier: MPL-2.0

//! The Welcome page's ticks, through the router against a real store under a
//! temporary directory.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use koru_store_core::model::{Scope, Timestamp};
use koru_store_core::service::Store;
use koru_store_core::text::Locale;
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};
use tower::ServiceExt as _;

use crate::router;
use crate::state::{AppState, KoruStore};

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState>,
    token: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let catalog = SqliteCatalog::open(&dir.path().join("catalog.db")).unwrap();
    let store: KoruStore = Store::open(catalog, SqliteBackend::new(dir.path())).unwrap();
    let state = Arc::new(AppState::new(
        store,
        "<root>".into(),
        "welcome-route-test".into(),
        at().millis(),
    ));
    let (token, _) = state.mint_session("test", Scope::ALL.to_vec());
    Fixture {
        _dir: dir,
        state,
        token,
    }
}

impl Fixture {
    async fn send(&self, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
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

    async fn done(&self, rows: &str) -> serde_json::Value {
        let (status, body) = self
            .send("GET", &format!("/v1/internal/welcome-progress?rows={rows}"))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["done"].clone()
    }
}

#[tokio::test]
async fn a_tick_round_trips_and_an_unticked_row_is_simply_absent() {
    let f = fixture();
    assert_eq!(f.done("learn,ai").await, serde_json::json!([]));

    let (status, body) = f.send("PUT", "/v1/internal/welcome-progress/learn").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["done"], serde_json::json!(["learn"]));
    // Ticking twice is not an error.
    let (status, _) = f.send("PUT", "/v1/internal/welcome-progress/learn").await;
    assert_eq!(status, StatusCode::OK);
    f.send("PUT", "/v1/internal/welcome-progress/first-document")
        .await;

    assert_eq!(
        f.done("first-document,ai,learn").await,
        serde_json::json!(["first-document", "learn"]),
        "only ticked rows, in the order asked"
    );
    assert_eq!(f.done("").await, serde_json::json!([]));
}

/// The ticks belong to the installation, not to a Profile: a Profile created
/// after the tick sees it too, because nothing about the key names one.
#[tokio::test]
async fn a_tick_is_shared_by_every_profile_on_the_installation() {
    let f = fixture();
    f.send("PUT", "/v1/internal/welcome-progress/phone").await;
    {
        let mut store = f.state.store.lock().unwrap();
        store
            .create_profile("Second", None, None, Locale::parse("en").unwrap(), at())
            .unwrap();
        assert_eq!(
            store
                .welcome_progress(&["phone".to_owned(), "computer".to_owned()])
                .unwrap(),
            ["phone"]
        );
    }
    assert_eq!(f.done("phone").await, serde_json::json!(["phone"]));
}

#[tokio::test]
async fn a_malformed_row_id_is_refused_rather_than_written() {
    let f = fixture();
    let (status, body) = f.send("PUT", "/v1/internal/welcome-progress/Peer_Id").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = f
        .send("GET", "/v1/internal/welcome-progress?rows=learn,..%2Fx")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let many = (0..33)
        .map(|i| format!("r{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let (status, _) = f
        .send("GET", &format!("/v1/internal/welcome-progress?rows={many}"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "at most 32 rows a query");
}
