// SPDX-License-Identifier: MPL-2.0

//! `client.sessions()` — the public rooted-process session namespace.
//!
//! Unary calls block on the shared Gateway transport. A subscription owns one
//! named reader thread and resumes routine deadline reconnects from its two
//! independent cursors. A reported gap is terminal for that subscription: the
//! caller must take a fresh snapshot and reconnect from `SnapshotResponse::resume`.

use std::path::Path;

use base64::Engine as _;
use koru_session_api::{Method, Route, RouteId};
use koru_store_client::RetryPolicy;

use crate::Client;

mod error;
mod stream;

pub use error::{SessionApiFailure, SessionFailureCode, SessionsError, SessionsResult};
use error::{local, local_io, map_public, validate_input};
pub use stream::SessionSubscription;

pub use koru_session_api::{
    AttachSessionRequest, AttachmentList, CommandReceipt, CreateSessionRequest,
    DetachSessionRequest, EmptyResponse, InterruptRequest, ListSessionsRequest, PasteTextRequest,
    RemoveAttachmentRequest, ResizeRequest, ScrollbackRequest, ScrollbackResponse, SendKeyRequest,
    SendPendingInputRequest, SessionList, SessionStreamItem, SessionStreamSource, SnapshotRequest,
    SnapshotResponse, SubscribeRequest, TerminateRequest, UploadAttachmentRequest,
    WriteBytesRequest, WriteTextRequest,
};
/// `SessionSummary::project_id` is readable here, so the type it names must be
/// too. The project *verbs* are not: grouping is device-local session-host
/// state, and a public client that could rename or delete a project would be a
/// second authority over it.
pub use koru_session_protocol::ProjectId;
pub use koru_session_protocol::{
    Attachment, AttachmentId, BuiltInProfileKey, ControllerLease, ControllerLeaseToken,
    DeliveryMode, ErrorCode, GridSize, IdempotencyKey, KeyStroke, RowIndex, RowRange,
    SessionFailure, SessionId, SessionMode, SessionProfileKey, SessionSummary, StreamSequence,
    TerminalGeneration, TerminalKey, ViewportGeneration,
};

pub struct SessionsClient<'a> {
    pub(crate) client: &'a Client,
}

impl SessionsClient<'_> {
    pub fn available(&self) -> bool {
        self.client
            .capabilities()
            .iter()
            .any(|capability| capability == koru_session_api::CAPABILITY)
    }

    pub fn create(&self, request: &CreateSessionRequest) -> SessionsResult<SessionSummary> {
        self.post(RouteId::Create, None, None, request)
    }

    pub fn list(&self, request: &ListSessionsRequest) -> SessionsResult<SessionList> {
        let mut path = route_path(RouteId::List, None, None);
        if let Some(parent) = request.parent_session_id {
            path.push_str("?parent_session_id=");
            path.push_str(&parent.to_string());
        }
        self.get_path(RouteId::List, &path)
    }

    pub fn get(&self, session_id: SessionId) -> SessionsResult<SessionSummary> {
        self.get_path(
            RouteId::Get,
            &route_path(RouteId::Get, Some(session_id), None),
        )
    }

    pub fn attach(
        &self,
        session_id: SessionId,
        request: &AttachSessionRequest,
    ) -> SessionsResult<ControllerLease> {
        self.post(RouteId::Attach, Some(session_id), None, request)
    }

    pub fn detach(
        &self,
        session_id: SessionId,
        controller_lease: ControllerLeaseToken,
    ) -> SessionsResult<()> {
        let _: EmptyResponse = self.delete(
            RouteId::Detach,
            session_id,
            None,
            &DetachSessionRequest { controller_lease },
        )?;
        Ok(())
    }

    pub fn snapshot(
        &self,
        session_id: SessionId,
        request: &SnapshotRequest,
    ) -> SessionsResult<SnapshotResponse> {
        self.post(RouteId::Snapshot, Some(session_id), None, request)
    }

    pub fn scrollback(
        &self,
        session_id: SessionId,
        request: &ScrollbackRequest,
    ) -> SessionsResult<ScrollbackResponse> {
        self.post(RouteId::Scrollback, Some(session_id), None, request)
    }

    pub fn write_bytes(
        &self,
        session_id: SessionId,
        idempotency_key: IdempotencyKey,
        controller_lease: ControllerLeaseToken,
        bytes: &[u8],
    ) -> SessionsResult<CommandReceipt> {
        validate_input(bytes.len(), !bytes.is_empty())?;
        self.post(
            RouteId::WriteBytes,
            Some(session_id),
            None,
            &WriteBytesRequest {
                idempotency_key,
                controller_lease,
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        )
    }

    pub fn write_text(
        &self,
        session_id: SessionId,
        request: &WriteTextRequest,
    ) -> SessionsResult<CommandReceipt> {
        validate_input(request.text.len(), !request.text.is_empty())?;
        self.post(RouteId::WriteText, Some(session_id), None, request)
    }

    pub fn send_key(
        &self,
        session_id: SessionId,
        request: &SendKeyRequest,
    ) -> SessionsResult<CommandReceipt> {
        self.post(RouteId::SendKey, Some(session_id), None, request)
    }

    pub fn paste(
        &self,
        session_id: SessionId,
        request: &PasteTextRequest,
    ) -> SessionsResult<CommandReceipt> {
        validate_input(request.text.len(), !request.text.is_empty())?;
        self.post(RouteId::Paste, Some(session_id), None, request)
    }

    pub fn upload_attachment(
        &self,
        session_id: SessionId,
        request: &UploadAttachmentRequest,
    ) -> SessionsResult<Attachment> {
        // Four encoded bytes represent at most three source bytes. This check
        // is conservative and avoids decoding content merely to count it.
        let encoded_limit = koru_session_api::MAX_ATTACHMENT_BYTES
            .div_ceil(3)
            .saturating_mul(4) as usize;
        if request.data_base64.len() > encoded_limit {
            return Err(local(
                koru_session_protocol::ErrorCode::AttachmentLimit,
                "attachment exceeds the public eight MiB limit",
            ));
        }
        self.post(RouteId::UploadAttachment, Some(session_id), None, request)
    }

    /// Read and upload an image without exposing its local path to Gateway.
    /// Metadata is checked before allocation and the public byte limit is
    /// checked again if the file changed while it was read.
    pub fn upload_image_path(
        &self,
        session_id: SessionId,
        controller_lease: ControllerLeaseToken,
        attachment_id: AttachmentId,
        path: impl AsRef<Path>,
    ) -> SessionsResult<Attachment> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).map_err(|error| local_io("inspect", error))?;
        if !metadata.is_file() {
            return Err(local(
                koru_session_protocol::ErrorCode::InvalidRequest,
                "attachment source is not a file",
            ));
        }
        if metadata.len() > koru_session_api::MAX_ATTACHMENT_BYTES {
            return Err(local(
                koru_session_protocol::ErrorCode::AttachmentLimit,
                "attachment exceeds the public eight MiB limit",
            ));
        }
        let bytes = std::fs::read(path).map_err(|error| local_io("read", error))?;
        if bytes.len() as u64 > koru_session_api::MAX_ATTACHMENT_BYTES {
            return Err(local(
                koru_session_protocol::ErrorCode::AttachmentLimit,
                "attachment exceeds the public eight MiB limit",
            ));
        }
        self.upload_attachment(
            session_id,
            &UploadAttachmentRequest {
                controller_lease,
                attachment_id,
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        )
    }

    pub fn attachments(&self, session_id: SessionId) -> SessionsResult<AttachmentList> {
        self.get_path(
            RouteId::ListAttachments,
            &route_path(RouteId::ListAttachments, Some(session_id), None),
        )
    }

    pub fn remove_attachment(
        &self,
        session_id: SessionId,
        attachment_id: AttachmentId,
        controller_lease: ControllerLeaseToken,
    ) -> SessionsResult<Attachment> {
        self.delete(
            RouteId::RemoveAttachment,
            session_id,
            Some(attachment_id),
            &RemoveAttachmentRequest { controller_lease },
        )
    }

    pub fn send_pending_input(
        &self,
        session_id: SessionId,
        request: &SendPendingInputRequest,
    ) -> SessionsResult<koru_session_protocol::SendPendingInputResult> {
        validate_input(
            request.text.len(),
            !request.text.is_empty() || !request.attachment_ids.is_empty(),
        )?;
        self.post(RouteId::SendPendingInput, Some(session_id), None, request)
    }

    pub fn interrupt(
        &self,
        session_id: SessionId,
        request: &InterruptRequest,
    ) -> SessionsResult<CommandReceipt> {
        self.post(RouteId::Interrupt, Some(session_id), None, request)
    }

    pub fn resize(
        &self,
        session_id: SessionId,
        request: &ResizeRequest,
    ) -> SessionsResult<CommandReceipt> {
        request
            .size
            .validate()
            .map_err(|code| local(code, "invalid terminal size"))?;
        self.put(RouteId::Resize, session_id, request)
    }

    pub fn terminate(
        &self,
        session_id: SessionId,
        request: &TerminateRequest,
    ) -> SessionsResult<CommandReceipt> {
        self.delete(RouteId::Terminate, session_id, None, request)
    }

    /// Start a live SSE reader. The handler runs serially on `koru-sessions`.
    /// Routine 90-second deadline reconnects are silent and cursor-resuming.
    /// Transport failures are reported before a bounded-backoff retry. A gap
    /// or closed item is delivered once and ends this subscription.
    pub fn subscribe(
        &self,
        session_id: SessionId,
        from: SubscribeRequest,
        mut handler: impl FnMut(SessionsResult<SessionStreamItem>) + Send + 'static,
    ) -> SessionsResult<SessionSubscription> {
        stream::subscribe(self.client.store().clone(), session_id, from, move |item| {
            handler(item)
        })
    }

    fn get_path<T: serde::de::DeserializeOwned>(
        &self,
        id: RouteId,
        path: &str,
    ) -> SessionsResult<T> {
        let route = route(id);
        debug_assert_eq!(route.method, Method::Get);
        self.client
            .store()
            .get_public_json(path)
            .map_err(map_public)
    }

    fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        id: RouteId,
        session_id: Option<SessionId>,
        attachment_id: Option<AttachmentId>,
        body: &B,
    ) -> SessionsResult<T> {
        let route = route(id);
        debug_assert_eq!(route.method, Method::Post);
        self.client
            .store()
            .post_public_json(
                &route_path(id, session_id, attachment_id),
                body,
                retry_policy(route),
            )
            .map_err(map_public)
    }

    fn put<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        id: RouteId,
        session_id: SessionId,
        body: &B,
    ) -> SessionsResult<T> {
        let route = route(id);
        debug_assert_eq!(route.method, Method::Put);
        self.client
            .store()
            .put_public_json(
                &route_path(id, Some(session_id), None),
                body,
                retry_policy(route),
            )
            .map_err(map_public)
    }

    fn delete<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        id: RouteId,
        session_id: SessionId,
        attachment_id: Option<AttachmentId>,
        body: &B,
    ) -> SessionsResult<T> {
        let route = route(id);
        debug_assert_eq!(route.method, Method::Delete);
        self.client
            .store()
            .delete_public_json(
                &route_path(id, Some(session_id), attachment_id),
                body,
                retry_policy(route),
            )
            .map_err(map_public)
    }
}

fn route(id: RouteId) -> Route {
    *koru_session_api::route_by_id(id)
}

fn retry_policy(route: Route) -> RetryPolicy {
    if route.retry_safe_after_unknown_outcome() {
        RetryPolicy::Safe
    } else {
        RetryPolicy::AuthenticationOnly
    }
}

fn route_path(
    id: RouteId,
    session_id: Option<SessionId>,
    attachment_id: Option<AttachmentId>,
) -> String {
    let mut path = route(id).path.to_owned();
    if path.contains("{session_id}") {
        let id = session_id.expect("catalogued session path requires a session id");
        path = path.replace("{session_id}", &id.to_string());
    }
    if path.contains("{attachment_id}") {
        let id = attachment_id.expect("catalogued attachment path requires an attachment id");
        path = path.replace("{attachment_id}", &id.to_string());
    }
    debug_assert!(!path.contains('{'));
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_come_from_the_catalog() {
        let session = SessionId::mint();
        let attachment = AttachmentId::mint();
        assert_eq!(
            route_path(RouteId::RemoveAttachment, Some(session), Some(attachment)),
            format!("/v1/sessions/{session}/attachments/{attachment}")
        );
    }

    #[test]
    fn retry_policy_distinguishes_unknown_mutation_outcomes() {
        assert_eq!(retry_policy(route(RouteId::Snapshot)), RetryPolicy::Safe);
        assert_eq!(retry_policy(route(RouteId::WriteText)), RetryPolicy::Safe);
        assert_eq!(
            retry_policy(route(RouteId::UploadAttachment)),
            RetryPolicy::Safe
        );
        assert_eq!(
            retry_policy(route(RouteId::Attach)),
            RetryPolicy::AuthenticationOnly
        );
        assert_eq!(
            retry_policy(route(RouteId::RemoveAttachment)),
            RetryPolicy::AuthenticationOnly
        );
    }
}
