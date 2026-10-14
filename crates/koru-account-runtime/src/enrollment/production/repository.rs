// SPDX-License-Identifier: MPL-2.0

//! Existing inbound persistence effects, separated from the wire dialogue.
use super::*;
use std::pin::Pin;

pub(super) type RepositoryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, EnrollmentHandlerError>> + Send + 'a>>;
pub(super) struct ReplayRequest {
    pub evidence: SignedDeviceAttestationV1,
    pub jti: String,
    pub profile: ProfileId,
    pub local: DeviceDescriptorV1,
    pub joining: DeviceDescriptorV1,
    pub joining_challenge: [u8; 32],
}
impl ReplayRequest {
    pub fn validate(
        &self,
        value: Option<PreparedEnrollmentV1>,
    ) -> Result<Option<PreparedEnrollmentV1>, EnrollmentHandlerError> {
        match value {
            Some(value)
                if value.attestation == self.evidence
                    && value.transcript.profile_id == self.profile
                    && value.transcript.existing_device == self.local
                    && value.transcript.joining_device == self.joining
                    && value.transcript.joining_challenge == self.joining_challenge =>
            {
                Ok(Some(value))
            }
            Some(_) => Err(refused("attestation_invalid")),
            None => Ok(None),
        }
    }
}
pub(super) trait InboundRepository: Sync {
    fn replay(&self, request: ReplayRequest) -> RepositoryFuture<'_, Option<PreparedEnrollmentV1>>;
    fn persist(
        &self,
        prepared: PreparedEnrollmentV1,
        verified: VerifiedDeviceAttestationV1,
        local: koru_device::DeviceId,
        session: Arc<ResponderSessionGate>,
        ticket: ResponderTicket,
    ) -> RepositoryFuture<'_, (u64, bool)>;
}
pub(super) struct DiskInboundRepository(pub PathBuf);
impl InboundRepository for DiskInboundRepository {
    fn replay(&self, request: ReplayRequest) -> RepositoryFuture<'_, Option<PreparedEnrollmentV1>> {
        Box::pin(replay_completion(self.0.clone(), request))
    }
    fn persist(
        &self,
        prepared: PreparedEnrollmentV1,
        verified: VerifiedDeviceAttestationV1,
        local: koru_device::DeviceId,
        session: Arc<ResponderSessionGate>,
        ticket: ResponderTicket,
    ) -> RepositoryFuture<'_, (u64, bool)> {
        Box::pin(persist_inbound(
            self.0.clone(),
            prepared,
            verified,
            local,
            session,
            ticket,
        ))
    }
}
