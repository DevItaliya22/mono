// SPDX-License-Identifier: MPL-2.0

use std::collections::HashSet;

use schemars::generate::SchemaSettings;
use serde_json::json;

use super::*;

fn scopes(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn expanded_request(route: &Route) -> String {
    let mut generator = SchemaSettings::openapi3().into_generator();
    let root = (route.request)(&mut generator);
    serde_json::json!({"root": root, "definitions": generator.take_definitions(true)}).to_string()
}

fn expanded_response(route: &Route) -> String {
    let mut generator = SchemaSettings::openapi3().into_generator();
    let root = (route.response)(&mut generator);
    serde_json::json!({"root": root, "definitions": generator.take_definitions(true)}).to_string()
}

#[test]
fn route_catalog_is_closed_unique_and_indexed() {
    let mut methods_and_paths = HashSet::new();
    let mut operations = HashSet::new();
    for route in ROUTES {
        assert!(methods_and_paths.insert((route.method, route.path)));
        assert!(operations.insert(route.id.operation_id()));
        assert_eq!(route_by_id(route.id).id, route.id);
        assert_eq!(
            route_for(route.method, route.path).map(|found| found.id),
            Some(route.id)
        );
        assert!(!route.scopes.is_empty());
        assert!(!route.summary.is_empty());
    }
    assert_eq!(ROUTES.len(), 19);
}

#[test]
fn retry_sensitive_and_controller_calls_name_their_proofs() {
    for route in ROUTES {
        let request = expanded_request(route);
        if route.replay_sensitive {
            assert!(
                request.contains("idempotency_key"),
                "{} lacks idempotency_key",
                route.id.operation_id()
            );
        }
        if route.controller_only {
            assert!(
                request.contains("controller_lease"),
                "{} lacks controller_lease",
                route.id.operation_id()
            );
        }
    }
}

#[test]
fn read_scopes_cannot_reach_mutations() {
    for held in [scopes(&[METADATA_SCOPE]), scopes(&[OUTPUT_SCOPE])] {
        for route in ROUTES
            .iter()
            .filter(|route| route.access == Access::Mutation)
        {
            assert_eq!(authorize(route, &held), Err(GatewayError::PermissionDenied));
        }
    }
}

#[test]
fn metadata_output_attachment_and_control_are_separate() {
    assert_eq!(
        authorize(route_by_id(RouteId::Snapshot), &scopes(&[METADATA_SCOPE])),
        Err(GatewayError::PermissionDenied)
    );
    assert_eq!(
        authorize(route_by_id(RouteId::Subscribe), &scopes(&[METADATA_SCOPE])),
        Err(GatewayError::PermissionDenied)
    );
    for id in [
        RouteId::UploadAttachment,
        RouteId::ListAttachments,
        RouteId::RemoveAttachment,
    ] {
        assert_eq!(
            authorize(route_by_id(id), &scopes(&[CONTROL_SCOPE])),
            Err(GatewayError::PermissionDenied)
        );
    }
    assert_eq!(
        authorize(
            route_by_id(RouteId::SendPendingInput),
            &scopes(&[ATTACHMENT_SCOPE])
        ),
        Err(GatewayError::PermissionDenied)
    );
    assert!(
        authorize(
            route_by_id(RouteId::SendPendingInput),
            &scopes(&[CONTROL_SCOPE, ATTACHMENT_SCOPE])
        )
        .is_ok()
    );
    assert_eq!(
        authorize(
            route_by_id(RouteId::UploadAttachment),
            &scopes(&[ATTACHMENT_SCOPE])
        ),
        Err(GatewayError::PermissionDenied)
    );
    assert!(
        authorize(
            route_by_id(RouteId::UploadAttachment),
            &scopes(&[CONTROL_SCOPE, ATTACHMENT_SCOPE])
        )
        .is_ok()
    );
    assert!(
        authorize(
            route_by_id(RouteId::ListAttachments),
            &scopes(&[ATTACHMENT_SCOPE])
        )
        .is_ok()
    );
}

#[test]
fn termination_has_an_independent_scope() {
    let terminate = route_by_id(RouteId::Terminate);
    assert_eq!(
        authorize(terminate, &scopes(&[CONTROL_SCOPE])),
        Err(GatewayError::PermissionDenied)
    );
    assert_eq!(
        authorize(terminate, &scopes(&[TERMINATE_SCOPE])),
        Err(GatewayError::PermissionDenied)
    );
    assert!(authorize(terminate, &scopes(&[CONTROL_SCOPE, TERMINATE_SCOPE])).is_ok());
}

#[test]
fn every_stable_failure_has_a_public_status() {
    for code in koru_session_protocol::ErrorCode::ALL {
        assert!((400..=599).contains(&public_status(code).0), "{code}");
    }
    assert_eq!(GATEWAY_ERROR_STATUSES.len(), GatewayError::ALL.len());
    for error in GatewayError::ALL {
        assert_eq!(
            GATEWAY_ERROR_STATUSES
                .iter()
                .find(|(candidate, _)| candidate == error)
                .map(|(_, status)| *status),
            Some(error.status())
        );
    }
}

#[test]
fn openapi_has_typed_requests_responses_scopes_and_errors() {
    let mut document = json!({
        "openapi": "3.1.0",
        "paths": {},
        "components": {
            "securitySchemes": {"bearerAuth": {"type": "http", "scheme": "bearer"}},
            "schemas": {"Failure": {"type": "object"}}
        }
    });
    extend_openapi(&mut document);
    for route in ROUTES {
        let operation = &document["paths"][route.path][route.method.lower()];
        assert_eq!(operation["operationId"], route.id.operation_id());
        assert_eq!(operation["x-koru-required-capability"], CAPABILITY);
        assert_eq!(
            operation["x-koru-unknown-outcome-retry-safe"],
            route.retry_safe_after_unknown_outcome()
        );
        assert!(!operation["x-koru-request-schema"].is_null());
        assert!(!operation["responses"]["200"]["content"].is_null());
        assert_eq!(
            operation["security"][0]["bearerAuth"],
            serde_json::to_value(route.scopes).unwrap()
        );
        assert!(operation["x-koru-stable-error-status"].is_object());
        assert!(operation["responses"]["403"].is_object());
        assert!(operation["responses"]["503"].is_object());
    }
}

#[test]
fn a_projects_placement_is_readable_here_and_never_writable() {
    // Every route that returns a session summary carries the placement...
    let mut carriers = 0;
    for route in ROUTES {
        if expanded_response(route).contains("SessionSummary") {
            carriers += 1;
            assert!(
                expanded_response(route).contains("project_id"),
                "{} returns a summary without its placement",
                route.id.operation_id()
            );
        }
        // ...and no route accepts one. Grouping is device-local session-host
        // state with one authority; a public mutation would make a second.
        assert!(
            !expanded_request(route).contains("project"),
            "{} lets a public caller write a project",
            route.id.operation_id()
        );
    }
    assert!(carriers >= 3, "create, get and list all carry the summary");
    assert_eq!(
        ROUTES.len(),
        19,
        "a project verb reached the public catalog"
    );
}

#[test]
fn asyncapi_declares_a_bounded_ordered_gap_aware_sse_stream() {
    let mut document = json!({
        "asyncapi": "3.0.0",
        "servers": {},
        "channels": {},
        "components": {
            "securitySchemes": {"bearerAuth": {"type": "httpApiKey"}}
        }
    });
    extend_asyncapi(&mut document);
    let channel = &document["channels"][SESSION_STREAMS[0].path];
    assert_eq!(channel["x-protocol"], "sse");
    assert_eq!(channel["x-ordered"], true);
    assert_eq!(channel["x-backpressured"], true);
    assert_eq!(channel["x-explicit-gap"], true);
    assert_eq!(channel["x-required-scopes"][0], OUTPUT_SCOPE);
    assert!(channel["messages"]["session_stream_item"]["payload"].is_object());
}
