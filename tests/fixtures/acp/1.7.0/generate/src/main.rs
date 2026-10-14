// SPDX-License-Identifier: MPL-2.0

use agent_client_protocol_schema::v1::*;
use schemars::{JsonSchema, schema_for};

#[derive(JsonSchema)]
#[allow(dead_code)]
struct AcpFixture {
    initialize_request: InitializeRequest,
    initialize_response: InitializeResponse,
    new_session_request: NewSessionRequest,
    new_session_response: NewSessionResponse,
    prompt_request: PromptRequest,
    prompt_response: PromptResponse,
    cancel_notification: CancelNotification,
    session_notification: SessionNotification,
    permission_request: RequestPermissionRequest,
    permission_response: RequestPermissionResponse,
}

fn main() {
    println!("{}", serde_json::to_string_pretty(&schema_for!(AcpFixture)).unwrap());
}
