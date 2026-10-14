// SPDX-License-Identifier: MPL-2.0

//! The hosted half of the coordinator: the typed native account client.
//!
//! It is deliberately thin. Every route already exists and is already
//! strict; the only judgment here is which refusal is a reason to retry, and
//! that a 503 `server_unavailable` from the descriptor read means "not ready
//! yet", never "make something up".

use koru_account_client::{AccountAuthorization, AccountClient, ClientError};
use koru_account_wire::cloud_sync::{SyncTokenRequestV1, SyncTokenResponseV1};
use koru_account_wire::{
    CloudProfileSelectionRequestV1, CloudWorkspaceCursorV1, CloudWorkspaceDescriptorV1,
    CloudWorkspacePageV1, ErrorCode, JwksResponseV1, Version1,
};

use super::*;

/// Wraps the shared typed client. Cloning the client is cheap and the worker
/// owns its own handle, so no lock is held across an HTTP call.
pub struct HostedApi {
    client: AccountClient,
}

impl HostedApi {
    pub fn new(client: AccountClient) -> Self {
        Self { client }
    }
}

impl Api for HostedApi {
    fn discover(
        &mut self,
        authority: &EnrollmentAuthorization,
        after: Option<CloudWorkspaceCursorV1>,
    ) -> Result<CloudWorkspacePageV1, Failure> {
        self.client
            .cloud_workspaces(
                after.as_ref(),
                &AccountAuthorization::Bearer(&authority.access),
            )
            .map_err(failure)
    }

    fn select(
        &mut self,
        authority: &EnrollmentAuthorization,
        profile: ProfileId,
        workspace: WorkspaceId,
    ) -> Result<(), Failure> {
        self.client
            .select_profile_cloud(
                profile,
                &CloudProfileSelectionRequestV1 {
                    version: Version1,
                    workspace_id: workspace,
                },
                &AccountAuthorization::Bearer(&authority.access),
            )
            .map(|_| ())
            .map_err(failure)
    }

    fn describe(
        &mut self,
        authority: &EnrollmentAuthorization,
        workspace: WorkspaceId,
        profile: ProfileId,
    ) -> Result<CloudWorkspaceDescriptorV1, Failure> {
        self.client
            .cloud_workspace_descriptor(
                workspace,
                profile,
                &AccountAuthorization::Bearer(&authority.access),
            )
            .map_err(failure)
    }

    fn issue_token(
        &mut self,
        authority: &EnrollmentAuthorization,
        workspace: WorkspaceId,
        profile: ProfileId,
    ) -> Result<SyncTokenResponseV1, Failure> {
        self.client
            .issue_cloud_sync_token(
                workspace,
                &SyncTokenRequestV1 {
                    version: Version1,
                    profile_id: profile,
                },
                &AccountAuthorization::Bearer(&authority.access),
            )
            .map_err(failure)
    }

    fn jwks(&mut self) -> Result<JwksResponseV1, Failure> {
        self.client.jwks().map_err(failure)
    }
}

/// A transport problem is offline and retried; a revoked session or a denied
/// principal is not, because only a new authorization can change it.
fn failure(error: ClientError) -> Failure {
    match error {
        ClientError::Transport(_) => Failure::Offline,
        ClientError::RateLimited { retry_after_millis } => {
            Failure::RateLimited(Duration::from_millis(retry_after_millis))
        }
        ClientError::Api { status, code } => match (status, code.as_str()) {
            (401, _) => Failure::SessionExpired,
            (403, _) => Failure::Denied,
            (_, ErrorCode::SERVER_UNAVAILABLE) => Failure::NotReady,
            _ => Failure::Unavailable,
        },
        ClientError::InvalidRequest
        | ClientError::MissingBrowserOrigin
        | ClientError::InvalidResponse
        | ClientError::ContractMismatch => Failure::Unavailable,
    }
}

/// Build the credential verifier a host dials with, from the JWKS the
/// coordinator read under the current account generation. Only the
/// capability trust domain is admitted: access and attestation keys are never
/// unioned in, and an empty capability set is a deployment without Cloud sync
/// rather than a reason to try another domain.
pub fn capability_verifier(
    region: koru_account_core::workspace_authorization::Region,
    issuer: &str,
    keys: &JwksResponseV1,
) -> Result<koru_account_session::TokenCapabilityVerifier, Failure> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    if keys.capability_keys.is_empty() || keys.capability_keys.len() > 16 {
        return Err(Failure::Unavailable);
    }
    let mut verifying = Vec::with_capacity(keys.capability_keys.len());
    for key in &keys.capability_keys {
        if key.kty != "OKP"
            || key.crv != "Ed25519"
            || key.key_use != "sig"
            || key.alg != "EdDSA"
            || key.x.len() != 43
        {
            return Err(Failure::Unavailable);
        }
        let bytes: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&key.x)
            .map_err(|_| Failure::Unavailable)?
            .try_into()
            .map_err(|_| Failure::Unavailable)?;
        if URL_SAFE_NO_PAD.encode(bytes) != key.x {
            return Err(Failure::Unavailable);
        }
        let public =
            ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| Failure::Unavailable)?;
        verifying.push((key.kid.clone(), public));
    }
    koru_account_session::TokenCapabilityVerifier::new(region, issuer.to_owned(), verifying)
        .map_err(|_| Failure::Unavailable)
}
