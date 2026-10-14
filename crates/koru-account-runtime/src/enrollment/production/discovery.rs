// SPDX-License-Identifier: MPL-2.0

use super::*;

impl ProductionPort {
    pub(super) fn discover_profiles(
        &mut self,
        profile: ProfileId,
        credential: &EnrollmentAuthorization,
        previous: Option<&ProductionAttempt>,
        cancel: &AtomicBool,
    ) -> Result<Discovery<EnrollmentCandidateV1, ProductionAttempt>, Failure> {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Unavailable);
        }
        // Discovery observes current authority, never a lane's preceding
        // selection response snapshot retained for its Load validation.
        if !self
            .authorized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allows(credential, profile)
        {
            return Err(Failure::AccessRevoked);
        }
        let authorization = AccountAuthorization::Bearer(&credential.access);
        // Refresh the roster before issuing evidence: a failed roster request
        // must not strand a newly reserved proof that the member never receives.
        let candidates = self
            .client
            .enrollment_candidates(profile, &authorization)
            .map_err(map_client)?
            .candidates;
        let only_older_peers = if candidates.is_empty() {
            self.client
                .devices(&authorization)
                .map_err(map_client)?
                .devices
                .iter()
                .any(|device| {
                    !device.revoked
                        && device.device_id != self.local.device_id
                        && device.profile_states.iter().any(|binding| {
                            binding.profile_id == profile
                                && binding.state == DeviceProfileState::Provisioned
                        })
                        && (device.enrollment_protocol_version != Some(ENROLLMENT_VERSION)
                            || device.peer_protocol_version != Some(PEER_PROTOCOL_VERSION))
                })
        } else {
            false
        };
        let now = self.clock.now_millis().ok_or(Failure::Unavailable)?;
        let attempt = match previous
            .filter(|attempt| attempt.usable_for(profile, credential, &self.local, now))
        {
            Some(attempt) => attempt.clone(),
            None => self.issue_attempt(profile, credential)?,
        };
        Ok(Discovery {
            candidates,
            only_older_peers,
            attempt,
        })
    }

    fn issue_attempt(
        &mut self,
        profile: ProfileId,
        credential: &EnrollmentAuthorization,
    ) -> Result<ProductionAttempt, Failure> {
        let (evidence, verified) = request_attestation(
            &self.client,
            &self.issuer,
            profile,
            &AccountAuthorization::Bearer(&credential.access),
            &mut self.jwks,
            self.clock.as_ref(),
        )?;
        let mut joining_challenge = [0; 32];
        self.entropy.fill_bytes(&mut joining_challenge);
        let attempt = ProductionAttempt {
            evidence,
            verified,
            joining_challenge,
        };
        let now = self.clock.now_millis().ok_or(Failure::Unavailable)?;
        if !attempt.usable_for(profile, credential, &self.local, now) {
            return Err(Failure::BadAttestation);
        }
        Ok(attempt)
    }
}

impl ProductionAttempt {
    pub(super) fn usable_for(
        &self,
        profile: ProfileId,
        credential: &EnrollmentAuthorization,
        local: &DeviceDescriptorV1,
        now: i64,
    ) -> bool {
        let claims = self.verified.claims();
        self.verified.is_current_at(now)
            && claims.session_id.to_string() == credential.session_id
            && claims.koru_identity_id == credential.owner.account
            && claims.device_id == local.device_id
            && claims.signing_public_key == local.signing_public_key
            && claims.endpoint_id == local.endpoint_id
            && claims.authorized_profile_ids.contains(&profile)
    }
}

fn request_attestation(
    client: &AccountClient,
    issuer: &str,
    profile: ProfileId,
    authorization: &AccountAuthorization<'_>,
    jwks: &mut Option<JwksResponseV1>,
    clock: &dyn Clock,
) -> Result<(SignedDeviceAttestationV1, VerifiedDeviceAttestationV1), Failure> {
    if jwks.is_none() {
        *jwks = Some(client.jwks().map_err(map_client)?);
    }
    let issued = client
        .issue_device_attestation(
            profile,
            &DeviceAttestationRequestV1 { version: Version1 },
            authorization,
        )
        .map_err(map_client)?;
    let evidence = SignedDeviceAttestationV1 {
        compact: issued.attestation.compact,
    };
    // The server creates this proof during the request. A time sampled before
    // that request is stale, even when both clocks agree exactly.
    let now = clock.now_millis().ok_or(Failure::Unavailable)?;
    let verified = client
        .verify_device_attestation(
            &evidence,
            issuer,
            now,
            jwks.as_mut().expect("JWKS initialized above"),
        )
        .map_err(map_attestation)?;
    // An unknown signing key can cause a further JWKS request. Expiry remains
    // strict after that round trip too.
    if !verified.is_current_at(clock.now_millis().ok_or(Failure::Unavailable)?) {
        return Err(Failure::BadAttestation);
    }
    Ok((evidence, verified))
}

#[cfg(test)]
mod tests;
