// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use koru_session_protocol::{AttachmentId, SessionId};
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::{
    CAPABILITY, GATEWAY_ERROR_STATUSES, ROUTES, RequestLocation, SESSION_ERROR_STATUSES,
    SESSION_STREAMS,
};

/// Adds the public session surface to the Gateway's existing OpenAPI document.
/// The Gateway remains the owner of HTTP and of the common `Failure` body.
pub fn extend_openapi(document: &mut Value) {
    let mut generator = SchemaSettings::openapi3().into_generator();
    let session_id = generator.subschema_for::<SessionId>();
    let attachment_id = generator.subschema_for::<AttachmentId>();
    let failure = json!({"$ref": "#/components/schemas/Failure"});

    let paths = document["paths"]
        .as_object_mut()
        .expect("Gateway OpenAPI has paths");
    for route in ROUTES {
        let request = (route.request)(&mut generator);
        let response = (route.response)(&mut generator);
        let mut operation = json!({
            "operationId": route.id.operation_id(),
            "summary": route.summary,
            "security": [{"bearerAuth": route.scopes}],
            "x-koru-required-capability": CAPABILITY,
            "x-koru-request-schema": request,
            "x-koru-request-location": request_location(route.request_location),
            "x-koru-controller-lease-required": route.controller_only,
            "x-koru-idempotency-required": route.replay_sensitive,
            "x-koru-unknown-outcome-retry-safe": route.retry_safe_after_unknown_outcome(),
            "x-koru-stable-error-status": stable_error_statuses(),
            "responses": {
                "200": {
                    "description": route.summary,
                    "content": {"application/json": {"schema": response}}
                }
            }
        });
        operation["parameters"] = Value::Array(path_parameters(
            route.path,
            session_id.clone().into(),
            attachment_id.clone().into(),
        ));
        if route.request_location == RequestLocation::Body {
            operation["requestBody"] = json!({
                "required": true,
                "content": {"application/json": {"schema": request}}
            });
        }
        if route.request_location == RequestLocation::Query {
            operation["description"] =
                json!("Query fields use the typed request schema in x-koru-request-schema.");
        }
        if route.access == crate::Access::Stream {
            operation["responses"]["200"]["content"] = json!({
                "text/event-stream": {"schema": response}
            });
        }
        for status in stable_status_codes() {
            operation["responses"][status.to_string()] = json!({
                "description": "A stable machine-readable Koru failure",
                "content": {"application/json": {"schema": failure}}
            });
        }

        let entry = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        let methods = entry.as_object_mut().expect("an OpenAPI path is an object");
        assert!(
            methods
                .insert(route.method.lower().to_owned(), operation)
                .is_none(),
            "session route collides with an existing Gateway route: {} {}",
            route.method.as_str(),
            route.path
        );
    }

    let generated = generator.take_definitions(true);
    let schemas = document["components"]["schemas"]
        .as_object_mut()
        .expect("Gateway OpenAPI has component schemas");
    for (name, schema) in generated {
        match schemas.get(&name) {
            Some(existing) => assert_eq!(
                existing, &schema,
                "session schema name collides with a different Gateway schema: {name}"
            ),
            None => {
                schemas.insert(name, schema);
            }
        }
    }
}

/// Adds the typed, ordered SSE channel to the Gateway stream catalog.
pub fn extend_asyncapi(document: &mut Value) {
    let mut generator = SchemaSettings::openapi3().into_generator();
    let channels = document["channels"]
        .as_object_mut()
        .expect("Gateway AsyncAPI has channels");
    for channel in SESSION_STREAMS {
        let messages = channel
            .messages
            .iter()
            .map(|message| {
                (
                    message.name.to_owned(),
                    json!({
                        "title": message.name,
                        "summary": message.description,
                        "payload": (message.payload)(&mut generator),
                        "x-direction": message.direction,
                    }),
                )
            })
            .collect::<Map<_, _>>();
        let value = json!({
            "address": channel.path,
            "messages": messages,
            "servers": [{"$ref": "#/servers/localSse"}],
            "x-protocol": channel.protocol,
            "x-version": channel.version,
            "x-ordered": channel.ordered,
            "x-backpressured": channel.backpressured,
            "x-explicit-gap": channel.explicit_gap,
            "x-required-scopes": [channel.scope],
            "x-koru-required-capability": CAPABILITY,
        });
        assert!(
            channels.insert(channel.path.to_owned(), value).is_none(),
            "session stream collides with an existing Gateway stream: {}",
            channel.path
        );
    }
    document["servers"]["localSse"] = json!({
        "host": "127.0.0.1",
        "protocol": "http",
        "security": [{"$ref": "#/components/securitySchemes/bearerAuth"}]
    });
    let generated = generator.take_definitions(true);
    let schemas = document["components"]
        .as_object_mut()
        .expect("Gateway AsyncAPI has components")
        .entry("schemas")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("AsyncAPI component schemas are an object");
    for (name, schema) in generated {
        match schemas.get(&name) {
            Some(existing) => assert_eq!(existing, &schema, "different AsyncAPI schema: {name}"),
            None => {
                schemas.insert(name, schema);
            }
        }
    }
}

const fn request_location(location: RequestLocation) -> &'static str {
    match location {
        RequestLocation::None => "none",
        RequestLocation::Query => "query",
        RequestLocation::Body => "body",
    }
}

fn stable_error_statuses() -> Value {
    let session = SESSION_ERROR_STATUSES.iter().map(|(class, status)| {
        (
            format!("session.{}", error_class_name(*class)),
            json!(status.0),
        )
    });
    let gateway = GATEWAY_ERROR_STATUSES
        .iter()
        .map(|(error, status)| (error.as_str().to_owned(), json!(status.0)));
    Value::Object(session.chain(gateway).collect())
}

const fn error_class_name(class: koru_session_protocol::ErrorClass) -> &'static str {
    match class {
        koru_session_protocol::ErrorClass::InvalidRequest => "invalid_request",
        koru_session_protocol::ErrorClass::Conflict => "conflict",
        koru_session_protocol::ErrorClass::NotFound => "not_found",
        koru_session_protocol::ErrorClass::Unavailable => "unavailable",
        koru_session_protocol::ErrorClass::Unsupported => "unsupported",
        koru_session_protocol::ErrorClass::Forbidden => "forbidden",
        koru_session_protocol::ErrorClass::Internal => "internal",
    }
}

fn stable_status_codes() -> BTreeSet<u16> {
    SESSION_ERROR_STATUSES
        .iter()
        .map(|(_, status)| status.0)
        .chain(GATEWAY_ERROR_STATUSES.iter().map(|(_, status)| status.0))
        .collect()
}

fn path_parameters(path: &str, session_id: Value, attachment_id: Value) -> Vec<Value> {
    let mut parameters = Vec::new();
    if path.contains("{session_id}") {
        parameters.push(json!({
            "name": "session_id", "in": "path", "required": true, "schema": session_id
        }));
    }
    if path.contains("{attachment_id}") {
        parameters.push(json!({
            "name": "attachment_id", "in": "path", "required": true,
            "schema": attachment_id
        }));
    }
    parameters
}
