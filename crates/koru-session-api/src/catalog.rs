// SPDX-License-Identifier: MPL-2.0

use schemars::{Schema, SchemaGenerator};

use crate::{
    ATTACHMENT_SCOPE, CONTROL_SCOPE, GatewayError, METADATA_SCOPE, OUTPUT_SCOPE, TERMINATE_SCOPE,
};

pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }

    pub const fn lower(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Mutation,
    Stream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestLocation {
    None,
    Query,
    Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RouteId {
    Create,
    List,
    Get,
    Attach,
    Detach,
    Snapshot,
    Scrollback,
    WriteBytes,
    WriteText,
    SendKey,
    Paste,
    UploadAttachment,
    ListAttachments,
    RemoveAttachment,
    SendPendingInput,
    Interrupt,
    Resize,
    Terminate,
    Subscribe,
}

impl RouteId {
    pub const fn operation_id(self) -> &'static str {
        match self {
            Self::Create => "sessions.create",
            Self::List => "sessions.list",
            Self::Get => "sessions.get",
            Self::Attach => "sessions.attach",
            Self::Detach => "sessions.detach",
            Self::Snapshot => "sessions.snapshot",
            Self::Scrollback => "sessions.scrollback",
            Self::WriteBytes => "sessions.write_bytes",
            Self::WriteText => "sessions.write_text",
            Self::SendKey => "sessions.send_key",
            Self::Paste => "sessions.paste",
            Self::UploadAttachment => "sessions.upload_attachment",
            Self::ListAttachments => "sessions.list_attachments",
            Self::RemoveAttachment => "sessions.remove_attachment",
            Self::SendPendingInput => "sessions.send_pending_input",
            Self::Interrupt => "sessions.interrupt",
            Self::Resize => "sessions.resize",
            Self::Terminate => "sessions.terminate",
            Self::Subscribe => "sessions.subscribe",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Route {
    pub id: RouteId,
    pub method: Method,
    pub path: &'static str,
    pub access: Access,
    pub scopes: &'static [&'static str],
    pub request: SchemaFn,
    pub response: SchemaFn,
    pub request_location: RequestLocation,
    pub replay_sensitive: bool,
    pub controller_only: bool,
    pub summary: &'static str,
}

impl Route {
    /// Whether a client may repeat this operation after the connection failed
    /// before a response arrived. Reads and streams have no mutation to
    /// duplicate; replay-sensitive commands carry an idempotency key; uploads
    /// carry a caller-minted attachment id. Every other mutation must surface
    /// its unknown outcome instead of guessing.
    pub const fn retry_safe_after_unknown_outcome(self) -> bool {
        match self.access {
            Access::Read | Access::Stream => true,
            Access::Mutation => {
                self.replay_sensitive || matches!(self.id, RouteId::UploadAttachment)
            }
        }
    }
}

macro_rules! schema_fn {
    ($name:ident, $type:ty) => {
        fn $name(generator: &mut SchemaGenerator) -> Schema {
            generator.subschema_for::<$type>()
        }
    };
}

schema_fn!(empty_request, crate::EmptyRequest);
schema_fn!(empty_response, crate::EmptyResponse);
schema_fn!(create_request, crate::CreateSessionRequest);
schema_fn!(create_response, crate::CreateSessionResponse);
schema_fn!(list_request, crate::ListSessionsRequest);
schema_fn!(list_response, crate::SessionList);
schema_fn!(get_response, crate::GetSessionResponse);
schema_fn!(attach_request, crate::AttachSessionRequest);
schema_fn!(attach_response, crate::AttachSessionResponse);
schema_fn!(detach_request, crate::DetachSessionRequest);
schema_fn!(snapshot_request, crate::SnapshotRequest);
schema_fn!(snapshot_response, crate::SnapshotResponse);
schema_fn!(scrollback_request, crate::ScrollbackRequest);
schema_fn!(scrollback_response, crate::ScrollbackResponse);
schema_fn!(bytes_request, crate::WriteBytesRequest);
schema_fn!(text_request, crate::WriteTextRequest);
schema_fn!(key_request, crate::SendKeyRequest);
schema_fn!(paste_request, crate::PasteTextRequest);
schema_fn!(command_response, crate::CommandReceipt);
schema_fn!(upload_request, crate::UploadAttachmentRequest);
schema_fn!(upload_response, crate::UploadAttachmentResponse);
schema_fn!(attachment_list, crate::AttachmentList);
schema_fn!(remove_request, crate::RemoveAttachmentRequest);
schema_fn!(remove_response, crate::RemoveAttachmentResponse);
schema_fn!(pending_request, crate::SendPendingInputRequest);
schema_fn!(pending_response, crate::SendPendingInputResponse);
schema_fn!(interrupt_request, crate::InterruptRequest);
schema_fn!(resize_request, crate::ResizeRequest);
schema_fn!(terminate_request, crate::TerminateRequest);
schema_fn!(subscribe_request, crate::SubscribeRequest);
schema_fn!(stream_response, crate::SessionStreamItem);

const METADATA: &[&str] = &[METADATA_SCOPE];
const OUTPUT: &[&str] = &[OUTPUT_SCOPE];
const CONTROL: &[&str] = &[CONTROL_SCOPE];
const ATTACHMENT: &[&str] = &[ATTACHMENT_SCOPE];
const CONTROL_ATTACHMENT: &[&str] = &[CONTROL_SCOPE, ATTACHMENT_SCOPE];
const CONTROL_TERMINATE: &[&str] = &[CONTROL_SCOPE, TERMINATE_SCOPE];

macro_rules! route {
    ($id:ident, $method:ident, $path:literal, $access:ident, $scopes:ident,
     $request:ident, $response:ident, $location:ident, $replay:literal,
     $controller:literal, $summary:literal) => {
        Route {
            id: RouteId::$id,
            method: Method::$method,
            path: $path,
            access: Access::$access,
            scopes: $scopes,
            request: $request,
            response: $response,
            request_location: RequestLocation::$location,
            replay_sensitive: $replay,
            controller_only: $controller,
            summary: $summary,
        }
    };
}

pub const ROUTES: &[Route] = &[
    route!(
        Create,
        Post,
        "/v1/sessions",
        Mutation,
        CONTROL,
        create_request,
        create_response,
        Body,
        true,
        false,
        "Create a rooted process session"
    ),
    route!(
        List,
        Get,
        "/v1/sessions",
        Read,
        METADATA,
        list_request,
        list_response,
        Query,
        false,
        false,
        "List session metadata"
    ),
    route!(
        Get,
        Get,
        "/v1/sessions/{session_id}",
        Read,
        METADATA,
        empty_request,
        get_response,
        None,
        false,
        false,
        "Get one session's metadata"
    ),
    route!(
        Attach,
        Post,
        "/v1/sessions/{session_id}/controller",
        Mutation,
        CONTROL,
        attach_request,
        attach_response,
        Body,
        false,
        false,
        "Acquire the exclusive controller lease"
    ),
    route!(
        Detach,
        Delete,
        "/v1/sessions/{session_id}/controller",
        Mutation,
        CONTROL,
        detach_request,
        empty_response,
        Body,
        false,
        true,
        "Release the controller lease without ending the process"
    ),
    route!(
        Snapshot,
        Post,
        "/v1/sessions/{session_id}/snapshot",
        Read,
        OUTPUT,
        snapshot_request,
        snapshot_response,
        Body,
        false,
        false,
        "Read a bounded terminal snapshot"
    ),
    route!(
        Scrollback,
        Post,
        "/v1/sessions/{session_id}/scrollback",
        Read,
        OUTPUT,
        scrollback_request,
        scrollback_response,
        Body,
        false,
        false,
        "Read one bounded scrollback page"
    ),
    route!(
        WriteBytes,
        Post,
        "/v1/sessions/{session_id}/input/bytes",
        Mutation,
        CONTROL,
        bytes_request,
        command_response,
        Body,
        true,
        true,
        "Write base64-encoded bytes to the process"
    ),
    route!(
        WriteText,
        Post,
        "/v1/sessions/{session_id}/input/text",
        Mutation,
        CONTROL,
        text_request,
        command_response,
        Body,
        true,
        true,
        "Write text to the process"
    ),
    route!(
        SendKey,
        Post,
        "/v1/sessions/{session_id}/input/key",
        Mutation,
        CONTROL,
        key_request,
        command_response,
        Body,
        true,
        true,
        "Send one terminal key"
    ),
    route!(
        Paste,
        Post,
        "/v1/sessions/{session_id}/input/paste",
        Mutation,
        CONTROL,
        paste_request,
        command_response,
        Body,
        true,
        true,
        "Paste text using terminal paste semantics"
    ),
    route!(
        UploadAttachment,
        Post,
        "/v1/sessions/{session_id}/attachments",
        Mutation,
        CONTROL_ATTACHMENT,
        upload_request,
        upload_response,
        Body,
        false,
        true,
        "Upload and attach verified image bytes"
    ),
    route!(
        ListAttachments,
        Get,
        "/v1/sessions/{session_id}/attachments",
        Read,
        ATTACHMENT,
        empty_request,
        attachment_list,
        None,
        false,
        false,
        "List session-owned attachments"
    ),
    route!(
        RemoveAttachment,
        Delete,
        "/v1/sessions/{session_id}/attachments/{attachment_id}",
        Mutation,
        CONTROL_ATTACHMENT,
        remove_request,
        remove_response,
        Body,
        false,
        true,
        "Remove one pending attachment"
    ),
    route!(
        SendPendingInput,
        Post,
        "/v1/sessions/{session_id}/pending-input",
        Mutation,
        CONTROL_ATTACHMENT,
        pending_request,
        pending_response,
        Body,
        true,
        true,
        "Deliver one logical text-and-attachment input"
    ),
    route!(
        Interrupt,
        Post,
        "/v1/sessions/{session_id}/interrupt",
        Mutation,
        CONTROL,
        interrupt_request,
        command_response,
        Body,
        true,
        true,
        "Interrupt the active process"
    ),
    route!(
        Resize,
        Put,
        "/v1/sessions/{session_id}/size",
        Mutation,
        CONTROL,
        resize_request,
        command_response,
        Body,
        true,
        true,
        "Set the process terminal size"
    ),
    route!(
        Terminate,
        Delete,
        "/v1/sessions/{session_id}",
        Mutation,
        CONTROL_TERMINATE,
        terminate_request,
        command_response,
        Body,
        true,
        true,
        "Terminate the complete process tree"
    ),
    route!(
        Subscribe,
        Get,
        "/v1/sessions/{session_id}/subscribe",
        Stream,
        OUTPUT,
        subscribe_request,
        stream_response,
        Query,
        false,
        false,
        "Subscribe to ordered output and lifecycle events"
    ),
];

pub fn route_by_id(id: RouteId) -> &'static Route {
    &ROUTES[id as usize]
}

pub fn route_for(method: Method, path: &str) -> Option<&'static Route> {
    ROUTES
        .iter()
        .find(|route| route.method == method && route.path == path)
}

/// Scope admission is intentionally pure so the HTTP adapter and proof matrix
/// cannot disagree. All listed scopes are required.
pub fn authorize(route: &Route, held: &[String]) -> Result<(), GatewayError> {
    route
        .scopes
        .iter()
        .all(|required| held.iter().any(|scope| scope == required))
        .then_some(())
        .ok_or(GatewayError::PermissionDenied)
}
