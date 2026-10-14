// SPDX-License-Identifier: MPL-2.0

//! The live ceremony protocol over an endpoint-authenticated frame stream.
//! Tests inject frames here without constructing an iroh endpoint or socket.
use super::*;
use std::pin::Pin;

pub(super) type IoFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, EnrollmentTransportError>> + Send + 'a>>;
pub(super) trait CeremonySession: Send {
    fn remote_endpoint(&self) -> ReplicationEndpointId;
    fn send_frame<'a>(&'a mut self, frame: &'a EnrollmentFrameV1) -> IoFuture<'a, ()>;
    fn receive_frame(&mut self) -> IoFuture<'_, EnrollmentFrameV1>;
}
impl CeremonySession for EnrollmentSession {
    fn remote_endpoint(&self) -> ReplicationEndpointId {
        EnrollmentSession::remote_endpoint(self)
    }
    fn send_frame<'a>(&'a mut self, frame: &'a EnrollmentFrameV1) -> IoFuture<'a, ()> {
        Box::pin(EnrollmentSession::send_frame(self, frame))
    }
    fn receive_frame(&mut self) -> IoFuture<'_, EnrollmentFrameV1> {
        Box::pin(EnrollmentSession::receive_frame(self))
    }
}
