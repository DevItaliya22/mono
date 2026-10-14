// SPDX-License-Identifier: MPL-2.0

//! Refresh credentials through their issuer, keeping permits and routes from
//! the same admission. No credential is extended or patched in place.
use super::*;

impl<A: Api, H: Host> Coordinator<A, H> {
    pub(super) fn authorized<T>(
        &mut self,
        call: impl Fn(&mut A, &EnrollmentAuthorization) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let authority = self.current_authority()?;
        let result = call(&mut self.api, &authority);
        if result.as_ref().err() == Some(&Failure::SessionExpired) {
            // An account refresh racing an HTTP request gets one retry with
            // the new bearer. A 401 with the same bearer waits for renewal;
            // it is never retried as though revocation were a network error.
            let current = self.current_authority()?;
            if current.access.0 != authority.access.0 {
                return call(&mut self.api, &current);
            }
        }
        result
    }

    fn renew_admission(&mut self, admission: &mut Admission) -> Result<(), Failure> {
        let authority = self.current_authority()?;
        tracing::info!(workspace = ?admission.candidate.workspace_id,
            "cloud session: renewing worker admission");
        let replacement = self
            .admit(&authority, admission.candidate.clone())?
            .ok_or(Failure::SessionExpired)?;
        self.current_authority()?;
        let old = [
            admission.workspace_permit.permit_id,
            admission.profile_permit.permit_id,
        ];
        self.host.fence(&old)?;
        self.live.retain(|permit| !old.contains(&permit.permit_id));
        self.sequence = self.sequence.saturating_add(1);
        let permits = [
            replacement.workspace_permit.clone(),
            replacement.profile_permit.clone(),
        ];
        // Retain ownership even if the host applied the context but its reply
        // was lost. The next pass must fence these ids before admitting more.
        self.live.extend(permits.iter().cloned());
        for (previous, permit) in old.into_iter().zip(&permits) {
            if let Some(mut record) = self.records.remove(&previous) {
                record.permit_id = permit.permit_id;
                record.dataset = permit.dataset;
                self.records.insert(permit.permit_id, record);
            }
        }
        *admission = replacement;
        // Retain the replacement in the lane before a fallible acknowledgement.
        // Retrying with its old IDs would leak a pair per uncertain renewal.
        self.host.replace_context(
            self.sequence,
            authority.owner.account,
            self.account_generation,
            &permits,
        )?;
        Ok(())
    }

    pub(super) fn prepare_session(
        &mut self,
        admission: &mut Admission,
        workspace: bool,
        attempt: u8,
        clock: &dyn Clock,
    ) -> Result<(), Failure> {
        self.current_authority()?;
        let now = clock.now_millis().ok_or(Failure::Unavailable)? / 1000;
        if attempt > 0
            || admission.token.expires_at_seconds().saturating_sub(now)
                <= SESSION_RENEW_BEFORE_SECONDS
        {
            self.renew_admission(admission)?;
        }
        let now = clock.now_millis().ok_or(Failure::Unavailable)? / 1000;
        if admission.token.expires_at_seconds().saturating_sub(now) <= SESSION_RENEW_BEFORE_SECONDS
        {
            return Err(Failure::CredentialExpired);
        }
        let permit = if workspace {
            &admission.workspace_permit
        } else {
            &admission.profile_permit
        };
        if let Some(record) = self.records.get_mut(&permit.permit_id) {
            record.stage = Stage::Reconciling;
            record.error_code = None;
        }
        self.stage = Stage::Reconciling;
        let authority = self.current_authority()?;
        if let Err(error) = self.host.replace_blob_access(
            authority.owner.account,
            self.account_generation,
            &authority.access,
        ) {
            tracing::warn!(
                code = error.code(),
                "cloud session: file access renewal failed"
            );
        }
        self.publish_progress();
        Ok(())
    }
}
