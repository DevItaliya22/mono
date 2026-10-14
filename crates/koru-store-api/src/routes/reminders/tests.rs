// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::error::status_for;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use koru_store_core::service::Store;
use koru_store_core::text::Locale;
use koru_store_core::{
    EffectiveState, FloatingDateTime, OccurrenceKey, OccurrenceMoment, OccurrenceSource,
    OneTimeSchedule, ReminderReductionInput, ReminderSettings, RuleRevisionId, TargetState,
    Timestamp, ZonedDateTime,
};
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};
use koru_store_wire::reminders::{
    ReminderQuery, ReminderQueryResponse, ReminderSnapshot, ReminderSnapshotRow, ReminderTarget,
};
use tower::ServiceExt as _;

#[test]
fn ids_require_canonical_profile_and_reminder_values() {
    let profile = ProfileId::new();
    let reminder = ReminderId::new();
    assert_eq!(
        ids(&profile.to_storage(), &reminder.to_storage()).unwrap(),
        (profile, reminder)
    );
    assert_eq!(
        ids("not-an-id", &reminder.to_storage())
            .unwrap_err()
            .0
            .code(),
        Code::InvalidId
    );
    assert_eq!(
        ids(&profile.to_storage(), "NOT-AN-ID")
            .unwrap_err()
            .0
            .code(),
        Code::InvalidId
    );
}

#[test]
fn snooze_and_handle_endpoints_reject_the_other_action_kind() {
    assert!(validate_action_route(&ActionKind::SnoozedUntil { instant_ms: 1 }, true).is_ok());
    assert!(validate_action_route(&ActionKind::Handled, false).is_ok());
    assert_eq!(
        validate_action_route(&ActionKind::Handled, true)
            .unwrap_err()
            .0
            .code(),
        Code::InvalidRequest
    );
    assert_eq!(
        validate_action_route(&ActionKind::SnoozedUntil { instant_ms: 1 }, false)
            .unwrap_err()
            .0
            .code(),
        Code::InvalidRequest
    );
}

#[test]
fn a_missing_entity_query_maps_to_the_stable_invalid_request() {
    let query = EntityQuery { entity: None };
    let error = query
        .entity
        .as_deref()
        .and_then(EntityRef::parse)
        .ok_or_else(invalid_entity)
        .unwrap_err();
    assert_eq!(error.0.code(), Code::InvalidRequest);
    assert_eq!(
        status_for(error.0.class()),
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[test]
fn query_validation_and_projection_failure_keep_distinct_http_classes() {
    let invalid = Error::new(Code::InvalidRequest, "now_ms and device_zone are required");
    let unavailable = Error::new(Code::ProjectionUnavailable, "projection is unavailable");
    assert_eq!(
        status_for(invalid.class()),
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for(unavailable.class()),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn projection_work_runs_after_the_callers_lock_is_released() {
    let lock = Arc::new(std::sync::Mutex::new(()));
    {
        let held = lock.lock().unwrap();
        drop(held);
    }
    let worker_lock = lock.clone();
    let answer = blocking_projection(move || {
        let _guard = worker_lock
            .try_lock()
            .expect("projection work must not inherit the caller's Store guard");
        Ok::<_, Error>(7)
    })
    .await
    .unwrap();
    assert_eq!(answer.0, 7);
}

fn api_fixture(scopes: Vec<Scope>) -> (Arc<AppState>, ProfileId, String) {
    use koru_store_core::ports::Catalog;
    use koru_store_core::profile_units::{IdentityManifest, PrivateManifest};
    let mut catalog = SqliteCatalog::in_memory().unwrap();
    let profile =
        koru_store_core::Profile::new("Owner", Locale::default(), Timestamp::from_millis(1))
            .unwrap();
    catalog
        .initialize_profile_units(
            &profile,
            &IdentityManifest::create(&profile)
                .unwrap()
                .export_snapshot()
                .unwrap(),
            &PrivateManifest::create(profile.id)
                .unwrap()
                .export_snapshot()
                .unwrap(),
        )
        .unwrap();
    let mut workspace =
        koru_store_core::Workspace::new(profile.id, "Closed", Timestamp::from_millis(1)).unwrap();
    workspace.id = fixture_workspace();
    catalog.put_workspace(&workspace).unwrap();
    // Only catalog metadata exists: no Workspace or document backend is opened.
    let store = Store::open(
        catalog,
        SqliteBackend::new(std::path::Path::new("unused-reminder-test-root")),
    )
    .unwrap();
    let state = Arc::new(AppState::new(
        store,
        "<memory>".into(),
        "reminder-test".into(),
        1,
    ));
    let (token, _) = state.mint_session("reminder-test", scopes);
    (state, profile.id, token)
}

fn request(
    method: &str,
    uri: String,
    token: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    request
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn write_rule(
    state: Arc<AppState>,
    method: &str,
    uri: String,
    token: &str,
    rule: koru_store_core::ReminderRule,
) -> axum::response::Response {
    crate::router(state)
        .oneshot(request(
            method,
            uri,
            Some(token),
            serde_json::to_value(RuleRequest { rule }).unwrap(),
        ))
        .await
        .unwrap()
}

async fn failure(response: axum::response::Response) -> koru_store_wire::Failure {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn fixture_workspace() -> koru_store_core::WorkspaceId {
    koru_store_core::WorkspaceId::parse("10000000-0000-4000-8000-000000000031").unwrap()
}

fn api_rule(id: ReminderId, hour: u8) -> koru_store_core::ReminderRule {
    koru_store_core::ReminderRule {
        id,
        target: EntityRef::new(
            "node",
            fixture_workspace(),
            koru_store_core::DocumentId::new(),
            uuid::Uuid::new_v4(),
        )
        .unwrap(),
        revision: RuleRevisionId::new(),
        status: RuleStatus::Enabled,
        schedule: OneTimeSchedule::Floating {
            wall: FloatingDateTime::parse(&format!("2026-09-02T{hour:02}:00:00")).unwrap(),
        },
        tone_override: None,
    }
}

fn zoned_api_rule(id: ReminderId, zone: &str) -> koru_store_core::ReminderRule {
    let mut rule = api_rule(id, 9);
    rule.schedule = OneTimeSchedule::Zoned {
        value: ZonedDateTime::new(1_788_264_000_000, zone).unwrap(),
    };
    rule
}

struct ReducingProjection;

impl crate::state::ReminderProjectionService for ReducingProjection {
    fn snapshot(
        &self,
        reminders: &[ReminderReductionInput],
        settings: &ReminderSettings,
    ) -> koru_store_core::Result<ReminderSnapshot> {
        Ok(ReminderSnapshot {
            reminders: reminders
                .iter()
                .map(|input| ReminderSnapshotRow {
                    stored: input.stored().clone(),
                    target: ReminderTarget {
                        target: input.stored().rule.target.clone(),
                        title: "Target".into(),
                        source_location: "Acme / Notes".into(),
                        state: TargetState::Active,
                        due: None,
                        projected: true,
                    },
                    occurrence_source: None,
                    reduced: input.reduce(TargetState::Active, None),
                })
                .collect(),
            settings: settings.clone(),
            updating_workspaces: Vec::new(),
        })
    }

    fn query(
        &self,
        reminders: &[ReminderReductionInput],
        settings: &ReminderSettings,
        _query: &ReminderQuery,
    ) -> koru_store_core::Result<ReminderQueryResponse> {
        let snapshot = self.snapshot(reminders, settings)?;
        Ok(ReminderQueryResponse {
            reminders: snapshot.reminders,
            updating_workspaces: snapshot.updating_workspaces,
        })
    }
}

#[tokio::test]
async fn public_reminder_http_contract_returns_authority_and_stable_failures() {
    let (state, profile, token) = api_fixture(Scope::ALL.to_vec());
    let id = ReminderId::new();
    let original = api_rule(id, 9);
    let collection = format!("/v1/profiles/{}/reminders", profile.to_storage());

    let response = crate::router(state.clone())
        .oneshot(request(
            "POST",
            collection.clone(),
            Some(&token),
            serde_json::to_value(RuleRequest {
                rule: original.clone(),
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: StoredReminder = serde_json::from_slice(&body).unwrap();
    assert_eq!(created.rule, original);

    let conflict = crate::router(state.clone())
        .oneshot(request(
            "POST",
            collection.clone(),
            Some(&token),
            serde_json::to_value(RuleRequest {
                rule: api_rule(id, 10),
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(failure(conflict).await.code, "idempotency_conflict");

    let missing = crate::router(state.clone())
        .oneshot(request(
            "GET",
            format!("{collection}/{}", ReminderId::new().to_storage()),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(failure(missing).await.code, "reminder_not_found");

    let missing_query = crate::router(state.clone())
        .oneshot(request(
            "GET",
            collection.clone(),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(missing_query.status(), StatusCode::BAD_REQUEST);
    assert_eq!(failure(missing_query).await.code, "invalid_request");

    let unavailable = crate::router(state)
        .oneshot(request(
            "GET",
            format!(
                "/v1/profiles/{}/reminders/reconciliation",
                profile.to_storage()
            ),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure(unavailable).await.code, "projection_unavailable");
}

#[tokio::test]
async fn create_requires_database_known_canonical_iana_zone() {
    let (state, profile, token) = api_fixture(Scope::ALL.to_vec());
    let uri = format!("/v1/profiles/{}/reminders", profile.to_storage());
    let response = write_rule(
        state.clone(),
        "POST",
        uri.clone(),
        token.as_str(),
        zoned_api_rule(ReminderId::new(), "US/Eastern"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(failure(response).await.code, "invalid_request");
    let response = write_rule(
        state.clone(),
        "POST",
        uri.clone(),
        token.as_str(),
        zoned_api_rule(ReminderId::new(), "Mars/Olympus"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(failure(response).await.code, "invalid_request");
    let response = write_rule(
        state.clone(),
        "POST",
        uri.clone(),
        token.as_str(),
        zoned_api_rule(ReminderId::new(), "America/New_York"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = write_rule(
        state,
        "POST",
        uri,
        token.as_str(),
        zoned_api_rule(ReminderId::new(), "UTC"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn update_requires_database_known_canonical_iana_zone() {
    let (state, profile, token) = api_fixture(Scope::ALL.to_vec());
    let collection = format!("/v1/profiles/{}/reminders", profile.to_storage());
    let id = ReminderId::new();
    let created = zoned_api_rule(id, "Europe/Zurich");
    let response = write_rule(
        state.clone(),
        "POST",
        collection.clone(),
        token.as_str(),
        created.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let uri = format!("{collection}/{}", id.to_storage());
    let mut utc = zoned_api_rule(id, "UTC");
    utc.revision = RuleRevisionId::new();
    let response = write_rule(
        state.clone(),
        "PUT",
        uri.clone(),
        token.as_str(),
        utc.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut alias = zoned_api_rule(id, "US/Eastern");
    alias.revision = RuleRevisionId::new();
    let response = write_rule(state.clone(), "PUT", uri.clone(), token.as_str(), alias).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(failure(response).await.code, "invalid_request");
    let mut unknown = zoned_api_rule(id, "Mars/Olympus");
    unknown.revision = RuleRevisionId::new();
    let response = write_rule(state.clone(), "PUT", uri.clone(), token.as_str(), unknown).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(failure(response).await.code, "invalid_request");
    let stored = lock(&state).unwrap().reminder(profile, id).unwrap();
    assert_eq!(stored.rule, utc, "rejected updates must not persist");
}

#[tokio::test]
async fn reconciliation_keeps_cleared_handled_occurrences_terminal_without_exposing_history() {
    let (state, profile, token) = api_fixture(Scope::ALL.to_vec());
    assert!(state.set_reminder_projection_service(Arc::new(ReducingProjection)));
    let id = ReminderId::new();
    let rule = api_rule(id, 9);
    let source = match &rule.schedule {
        OneTimeSchedule::Floating { wall } => OccurrenceSource::OneTime {
            moment: OccurrenceMoment::Floating { wall: wall.clone() },
        },
        _ => unreachable!(),
    };
    let occurrence = OccurrenceKey::derive(id, rule.revision, &source);
    let collection = format!("/v1/profiles/{}/reminders", profile.to_storage());

    let created = crate::router(state.clone())
        .oneshot(request(
            "POST",
            collection.clone(),
            Some(&token),
            serde_json::to_value(RuleRequest { rule }).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);

    let handled = crate::router(state.clone())
        .oneshot(request(
            "POST",
            format!("{collection}/{}/handle", id.to_storage()),
            Some(&token),
            serde_json::to_value(ActionRequest {
                action: koru_store_core::OccurrenceAction {
                    id: koru_store_core::ActionId::new(),
                    occurrence,
                    kind: ActionKind::Handled,
                },
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(handled.status(), StatusCode::OK);

    let cleared = crate::router(state.clone())
        .oneshot(request(
            "DELETE",
            format!("/v1/profiles/{}/reminder-history", profile.to_storage()),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(cleared.status(), StatusCode::OK);

    let response = crate::router(state)
        .oneshot(request(
            "GET",
            format!("{collection}/reconciliation"),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let snapshot: ReminderSnapshot = serde_json::from_slice(&body).unwrap();
    let row = &snapshot.reminders[0];
    assert!(row.stored.actions.is_empty());
    assert!(row.reduced.handled_history.is_empty());
    assert_eq!(
        row.reduced.effective,
        EffectiveState::Handled { occurrence }
    );
}

#[tokio::test]
async fn history_delete_without_query_is_profile_wide_and_entity_query_is_scoped() {
    let (state, profile, token) = api_fixture(Scope::ALL.to_vec());
    let first = api_rule(ReminderId::new(), 9);
    let second = api_rule(ReminderId::new(), 10);
    let handled = |rule: &koru_store_core::ReminderRule| {
        let source = match &rule.schedule {
            OneTimeSchedule::Floating { wall } => OccurrenceSource::OneTime {
                moment: OccurrenceMoment::Floating { wall: wall.clone() },
            },
            _ => unreachable!(),
        };
        koru_store_core::OccurrenceAction {
            id: koru_store_core::ActionId::new(),
            occurrence: OccurrenceKey::derive(rule.id, rule.revision, &source),
            kind: ActionKind::Handled,
        }
    };
    {
        let mut store = lock(&state).unwrap();
        store.create_reminder(profile, &first).unwrap();
        store.create_reminder(profile, &second).unwrap();
        store
            .append_reminder_action(profile, first.id, &handled(&first))
            .unwrap();
        store
            .append_reminder_action(profile, second.id, &handled(&second))
            .unwrap();
    }

    let scoped = crate::router(state.clone())
        .oneshot(request(
            "DELETE",
            format!(
                "/v1/profiles/{}/reminder-history?entity={}",
                profile.to_storage(),
                first.target.to_storage()
            ),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(scoped.status(), StatusCode::OK);
    {
        let mut store = lock(&state).unwrap();
        assert!(
            store
                .reminder(profile, first.id)
                .unwrap()
                .actions
                .is_empty()
        );
        assert_eq!(store.reminder(profile, second.id).unwrap().actions.len(), 1);
    }

    let all = crate::router(state.clone())
        .oneshot(request(
            "DELETE",
            format!("/v1/profiles/{}/reminder-history", profile.to_storage()),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(all.status(), StatusCode::OK);
    assert!(
        lock(&state)
            .unwrap()
            .reminder(profile, second.id)
            .unwrap()
            .actions
            .is_empty()
    );
}

#[tokio::test]
async fn reminder_http_authorization_is_enforced_before_the_handler() {
    let (state, profile, read_token) = api_fixture(vec![Scope::ReadDocuments]);
    let response = crate::router(state.clone())
        .oneshot(request(
            "POST",
            format!("/v1/profiles/{}/reminders", profile.to_storage()),
            Some(&read_token),
            serde_json::to_value(RuleRequest {
                rule: api_rule(ReminderId::new(), 9),
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(failure(response).await.code, "forbidden");

    let unauthenticated = crate::router(state)
        .oneshot(request(
            "GET",
            format!(
                "/v1/profiles/{}/reminders/{}",
                profile.to_storage(),
                ReminderId::new().to_storage()
            ),
            None,
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(failure(unauthenticated).await.code, "unauthenticated");
}

#[tokio::test]
async fn unit_scoped_reminder_read_and_mutation_never_fall_back_to_another_workspace() {
    let (state, profile, token) = api_fixture(Scope::ALL.to_vec());
    let rule = api_rule(ReminderId::new(), 9);
    state
        .store
        .lock()
        .unwrap()
        .create_reminder(profile, &rule)
        .unwrap();
    let path = format!(
        "/v1/profiles/{}/workspaces/{}/reminders/{}",
        profile,
        fixture_workspace(),
        rule.id
    );
    let response = crate::router(state.clone())
        .oneshot(request(
            "GET",
            path.clone(),
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let wrong = format!(
        "/v1/profiles/{}/workspaces/{}/reminders/{}",
        profile,
        koru_store_core::WorkspaceId::new(),
        rule.id
    );
    let response = crate::router(state.clone())
        .oneshot(request(
            "DELETE",
            wrong,
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        !state
            .store
            .lock()
            .unwrap()
            .reminder(profile, rule.id)
            .unwrap()
            .deleted
    );
    let response = crate::router(state.clone())
        .oneshot(request(
            "DELETE",
            path,
            Some(&token),
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        state
            .store
            .lock()
            .unwrap()
            .reminder(profile, rule.id)
            .unwrap()
            .deleted
    );
}
