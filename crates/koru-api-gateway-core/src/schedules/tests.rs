// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use super::*;

type DelegateResult<T> = std::result::Result<T, Failure>;

#[test]
fn every_schedule_route_is_catalogued_and_documented() {
    let mut seen = BTreeSet::new();
    for route in ROUTES {
        assert!(seen.insert((route.method, route.path)));
        assert!(route.path.starts_with("/v1/schedules"));
    }
    let mut document = serde_json::json!({"paths": {}, "components": {"schemas": {}}});
    extend_openapi(&mut document);
    for route in ROUTES {
        assert_eq!(
            document["paths"][route.path][route.method.to_lowercase()]["operationId"],
            route.operation_id
        );
    }
    assert!(document["components"]["schemas"]["CreateScheduleRequest"].is_object());
    assert!(document["paths"]["/v1/schedules"]["post"]["responses"]["201"].is_object());
    assert!(document["paths"]["/v1/schedules"]["post"]["responses"]["409"].is_object());
    assert!(document["paths"]["/v1/schedules"]["post"]["responses"]["413"].is_object());
    assert!(document["paths"]["/v1/schedules"]["post"]["responses"]["422"].is_object());
    assert!(document["components"]["schemas"]["RenameScheduleRequest"].is_object());
    assert!(document["paths"]["/v1/schedules/rename"]["post"]["responses"]["200"].is_object());
    assert!(document["paths"]["/v1/schedules/rename"]["post"]["responses"]["404"].is_object());
    assert!(document["paths"]["/v1/schedules/rename"]["post"]["responses"]["409"].is_object());
    assert!(document["components"]["schemas"]["EditScheduleRequest"].is_object());
    assert!(document["paths"]["/v1/schedules/edit"]["post"]["responses"]["200"].is_object());
    assert!(document["paths"]["/v1/schedules/edit"]["post"]["responses"]["409"].is_object());
    assert!(document["paths"]["/v1/schedules"]["get"]["responses"]["409"].is_null());
    assert!(document["paths"]["/v1/schedules/stream"]["get"]["responses"]["409"].is_null());
    assert!(document["paths"]["/v1/schedules/stream"]["get"]["responses"]["200"]
        ["content"]["text/event-stream"]
        .is_object());
}

#[test]
fn the_router_mounts_every_schedule_path() {
    let _ = router();
}

#[test]
fn schedule_failures_have_stable_statuses() {
    assert_eq!(
        status_for(&Failure::new("invalid_schedule", "", false)),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        status_for(&Failure::new("schedule_unavailable", "", true)),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        status_for(&Failure::new("schedule_conflict", "", false)),
        StatusCode::CONFLICT
    );
    assert_eq!(
        status_for(&Failure::new("schedule_name_conflict", "", false)),
        StatusCode::CONFLICT
    );
    assert_eq!(
        status_for(&Failure::new("schedule_revision_conflict", "", false)),
        StatusCode::CONFLICT
    );
    assert_eq!(
        status_for(&Failure::new("schedule_not_found", "", false)),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_for(&Failure::new("schedule_not_owner", "", false)),
        StatusCode::FORBIDDEN
    );
}

#[test]
fn dropping_an_idle_sse_state_cancels_without_waiting_for_its_bridge() {
    use std::sync::{Condvar, Mutex, mpsc};
    use std::time::Duration;

    struct Cancellation {
        state: Arc<(Mutex<bool>, Condvar)>,
    }
    impl ScheduleStreamCancellation for Cancellation {
        fn cancel(&self) {
            let (cancelled, wake) = &*self.state;
            *cancelled.lock().unwrap() = true;
            wake.notify_all();
        }
    }

    let state = Arc::new((Mutex::new(false), Condvar::new()));
    let bridge_state = Arc::clone(&state);
    let (release, wait) = mpsc::channel();
    let bridge = std::thread::spawn(move || {
        let (cancelled, wake) = &*bridge_state;
        let guard = cancelled.lock().unwrap();
        let guard = wake.wait_while(guard, |cancelled| !*cancelled).unwrap();
        // Keeping this guard during a simulated slow exit made the assertion
        // below wait on the bridge's mutex, even though Drop had returned.
        drop(guard);
        wait.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    let (_send, receive) = tokio::sync::mpsc::channel(1);
    let stream = ScheduleSseState {
        receive,
        cancellation: Arc::new(Cancellation {
            state: Arc::clone(&state),
        }),
    };

    let (finished, dropped) = mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(stream);
        let _ = finished.send(());
    });
    // The bridge is held open until Drop returns. This proves ordering rather
    // than measuring scheduler latency under a loaded workspace test run.
    let returned = dropped.recv_timeout(Duration::from_secs(5));
    let cancelled = *state.0.lock().unwrap();
    let bridge_still_running = !bridge.is_finished();
    // Also release the fixture if the regression is that Drop never cancelled.
    Cancellation {
        state: Arc::clone(&state),
    }
    .cancel();
    let _ = release.send(());
    dropper.join().unwrap();
    bridge.join().unwrap();
    assert!(returned.is_ok(), "Drop must return before the bridge exits");
    assert!(cancelled);
    assert!(bridge_still_running);
}

struct NoopCancellation;

impl ScheduleStreamCancellation for NoopCancellation {
    fn cancel(&self) {}
}

struct OneChange(Option<ScheduleChange>);

impl ScheduleStream for OneChange {
    fn next(&mut self) -> Option<ScheduleChange> {
        self.0.take()
    }

    fn cancellation(&self) -> Arc<dyn ScheduleStreamCancellation> {
        Arc::new(NoopCancellation)
    }
}

struct IdleCancellation {
    state: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl ScheduleStreamCancellation for IdleCancellation {
    fn cancel(&self) {
        let (cancelled, wake) = &*self.state;
        *cancelled.lock().unwrap() = true;
        wake.notify_all();
    }
}

struct IdleStream {
    state: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    exited: std::sync::mpsc::Sender<()>,
}

impl ScheduleStream for IdleStream {
    fn next(&mut self) -> Option<ScheduleChange> {
        let (cancelled, wake) = &*self.state;
        let guard = cancelled.lock().unwrap();
        let _guard = wake.wait_while(guard, |cancelled| !*cancelled).unwrap();
        self.exited.send(()).unwrap();
        None
    }

    fn cancellation(&self) -> Arc<dyn ScheduleStreamCancellation> {
        Arc::new(IdleCancellation {
            state: Arc::clone(&self.state),
        })
    }
}

enum ScheduleStreamFixture {
    OneChange,
    Idle {
        opened: std::sync::mpsc::Sender<()>,
        exited: std::sync::mpsc::Sender<()>,
    },
}

struct ScheduleOnlyDelegate {
    stream: ScheduleStreamFixture,
}

impl DelegatePort for ScheduleOnlyDelegate {
    fn serving(&self) -> bool {
        true
    }

    fn create(
        &self,
        _: RequestContext,
        _: koru_delegate_api::wire::CreateRunRequest,
    ) -> DelegateResult<koru_delegate_api::wire::CreateRunResponse> {
        unreachable!()
    }

    fn list(
        &self,
        _: RequestContext,
        _: koru_delegate_core::WorkspaceId,
    ) -> DelegateResult<koru_delegate_api::wire::ListRunsResponse> {
        unreachable!()
    }

    fn run(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
        _: koru_delegate_api::wire::TimelineQuery,
    ) -> DelegateResult<koru_delegate_api::wire::RunView> {
        unreachable!()
    }

    fn cancel(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
    ) -> DelegateResult<koru_delegate_api::wire::CancelRunResponse> {
        unreachable!()
    }

    fn resume(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
        _: koru_delegate_api::wire::ResumeRunRequest,
    ) -> DelegateResult<koru_delegate_api::wire::ResumeRunResponse> {
        unreachable!()
    }

    fn trash(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
        _: koru_delegate_api::wire::TrashRunRequest,
    ) -> DelegateResult<koru_delegate_api::wire::TrashRunResponse> {
        unreachable!()
    }

    fn restore(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
    ) -> DelegateResult<koru_delegate_api::wire::RestoreRunResponse> {
        unreachable!()
    }

    fn purge(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
    ) -> DelegateResult<koru_delegate_api::wire::PurgeRunResponse> {
        unreachable!()
    }

    fn rate(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
        _: koru_delegate_api::wire::RateRunRequest,
    ) -> DelegateResult<koru_delegate_api::wire::RateRunResponse> {
        unreachable!()
    }

    fn stream(
        &self,
        _: RequestContext,
        _: koru_delegate_core::RunId,
        _: Option<koru_delegate_core::WorkspaceId>,
        _: Option<u64>,
    ) -> DelegateResult<Box<dyn koru_delegate_api::RunStream>> {
        unreachable!()
    }

    fn create_schedule(
        &self,
        _: RequestContext,
        _: CreateScheduleRequest,
    ) -> DelegateResult<CreateScheduleResponse> {
        unreachable!()
    }

    fn list_schedules(
        &self,
        _: RequestContext,
        _: koru_delegate_core::WorkspaceId,
    ) -> DelegateResult<ListSchedulesResponse> {
        unreachable!()
    }

    fn watch_schedules(
        &self,
        _: RequestContext,
        _: koru_delegate_core::WorkspaceId,
        _: Option<ScheduleChange>,
    ) -> DelegateResult<Box<dyn ScheduleStream>> {
        match &self.stream {
            ScheduleStreamFixture::OneChange => Ok(Box::new(OneChange(Some(ScheduleChange {
                instance: uuid::Uuid::from_u128(1),
                generation: 7,
            })))),
            ScheduleStreamFixture::Idle { opened, exited } => {
                let state = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
                let stream = IdleStream {
                    state,
                    exited: exited.clone(),
                };
                opened.send(()).unwrap();
                Ok(Box::new(stream))
            }
        }
    }
}

#[tokio::test]
async fn real_router_gates_and_serves_a_content_free_schedule_event() {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt as _;

    let root = tempfile::tempdir().unwrap();
    let worker = koru_store_server::StoreWorker::new(koru_store_server::WorkerConfig::embedded(
        root.path().to_path_buf(),
        "internal-debug",
    ))
    .spawn()
    .unwrap();
    let state = GatewayState::new("internal-debug".into(), worker);
    assert!(state.set_delegate_port(Arc::new(ScheduleOnlyDelegate {
        stream: ScheduleStreamFixture::OneChange,
    })));
    let workspace = koru_delegate_core::WorkspaceId::from_uuid(uuid::Uuid::from_u128(2));
    let uri = format!("/v1/schedules/stream?workspace={workspace}");

    let missing = crate::router(Arc::clone(&state))
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let (write_only, _) = state.sessions.mint(
        "test",
        &[DelegateScope::Write.as_str().to_owned()],
        crate::now_millis(),
    );
    let insufficient = crate::router(Arc::clone(&state))
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header(header::AUTHORIZATION, format!("Bearer {write_only}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(insufficient.status(), StatusCode::FORBIDDEN);

    let (reader, _) = state.sessions.mint(
        "test",
        &[DelegateScope::Read.as_str().to_owned()],
        crate::now_millis(),
    );
    let response = crate::router(Arc::clone(&state))
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header(header::AUTHORIZATION, format!("Bearer {reader}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("event: schedule_change"));
    assert!(body.contains("\"generation\":7"));
    assert!(!body.contains(&workspace.to_string()));
}

#[tokio::test]
async fn repeated_real_route_body_drops_release_every_idle_bridge() {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt as _;

    let root = tempfile::tempdir().unwrap();
    let worker = koru_store_server::StoreWorker::new(koru_store_server::WorkerConfig::embedded(
        root.path().to_path_buf(),
        "internal-debug",
    ))
    .spawn()
    .unwrap();
    let state = GatewayState::new("internal-debug".into(), worker);
    let (opened, wait_opened) = std::sync::mpsc::channel();
    let (exited, wait_exited) = std::sync::mpsc::channel();
    assert!(state.set_delegate_port(Arc::new(ScheduleOnlyDelegate {
        stream: ScheduleStreamFixture::Idle { opened, exited },
    })));
    let (reader, _) = state.sessions.mint(
        "test",
        &[DelegateScope::Read.as_str().to_owned()],
        crate::now_millis(),
    );
    let workspace = koru_delegate_core::WorkspaceId::from_uuid(uuid::Uuid::from_u128(3));
    let uri = format!("/v1/schedules/stream?workspace={workspace}");

    for _ in 0..8 {
        let response = crate::router(Arc::clone(&state))
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(header::AUTHORIZATION, format!("Bearer {reader}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        wait_opened
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        drop(response);

        wait_exited
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("dropping the route body left its idle bridge blocked");
    }
}
