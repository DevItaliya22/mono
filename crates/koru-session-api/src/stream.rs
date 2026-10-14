// SPDX-License-Identifier: MPL-2.0

use schemars::{Schema, SchemaGenerator};

use crate::{OUTPUT_SCOPE, SessionStreamItem};

pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

#[derive(Clone, Copy, Debug)]
pub struct StreamMessage {
    pub name: &'static str,
    pub direction: &'static str,
    pub description: &'static str,
    pub payload: SchemaFn,
}

#[derive(Clone, Copy, Debug)]
pub struct StreamChannel {
    pub path: &'static str,
    pub protocol: &'static str,
    pub version: u32,
    pub ordered: bool,
    pub backpressured: bool,
    pub explicit_gap: bool,
    pub scope: &'static str,
    pub messages: &'static [StreamMessage],
}

fn item_schema(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<SessionStreamItem>()
}

pub const SESSION_MESSAGES: &[StreamMessage] = &[StreamMessage {
    name: "session_stream_item",
    direction: "server_to_client",
    description: "Per-source sequenced event/output, explicit resnapshot gap, or terminal close",
    payload: item_schema,
}];

pub const SESSION_STREAMS: &[StreamChannel] = &[StreamChannel {
    path: "/v1/sessions/{session_id}/subscribe",
    protocol: "sse",
    version: 1,
    ordered: true,
    backpressured: true,
    explicit_gap: true,
    scope: OUTPUT_SCOPE,
    messages: SESSION_MESSAGES,
}];
