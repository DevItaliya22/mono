// SPDX-License-Identifier: MPL-2.0

//! The hosted completion and local Profile commit are separate authorities.
//! Replaying the same completion repairs an interrupted local commit without
//! widening the peer's grant to descriptive Profile metadata or Profile CRDTs.
use super::*;

pub(super) fn provision(
    client: &AccountClient,
    local_profiles: &dyn LocalProfileInventory,
    profile: ProfileId,
    local: koru_device::DeviceId,
    completion: &PreparedEnrollmentV1,
    credential: &EnrollmentAuthorization,
    now: i64,
    check_current: impl Fn() -> Result<(), Failure>,
) -> Result<(), Failure> {
    if completion.transcript.profile_id != profile
        || completion.transcript.joining_device.device_id != local
    {
        return Err(Failure::BadAttestation);
    }
    let authorization = AccountAuthorization::Bearer(&credential.access);
    let session = client.session(&authorization).map_err(map_client)?;
    let profiles = client
        .profiles(&authorization)
        .map_err(map_client)?
        .profiles;
    let inventory = local_profiles
        .inventory()
        .map_err(|_| Failure::Unavailable)?;
    let authorized =
        ownership::AuthorizedProfiles::verify(credential, local, &session, &profiles, &inventory)?;
    if !authorized.allows(credential, profile) {
        return Err(Failure::AccessRevoked);
    }
    let summary = profiles
        .iter()
        .find(|row| row.profile_id == profile)
        .ok_or(Failure::AccessRevoked)?;
    if inventory
        .profiles
        .iter()
        .any(|row| row.profile_id == profile && row.removed_at_millis.is_some())
    {
        return Err(Failure::AccessRevoked);
    }
    check_current()?;
    let devices = client.devices(&authorization).map_err(map_client)?.devices;
    let already_provisioned = devices.iter().any(|device| {
        device.device_id == local
            && !device.revoked
            && device.endpoint_id_hex
                == completion
                    .transcript
                    .joining_device
                    .endpoint_id
                    .to_storage()
            && device.profile_states.iter().any(|state| {
                state.profile_id == profile && state.state == DeviceProfileState::Provisioned
            })
    });
    if !already_provisioned {
        // A new completion still requires its exact session-bound receipt.
        provision_completion(client, profile, local, completion, &authorization)?;
    }
    // A fresh session may repair an older completed target only because its
    // current account's hosted Device/Profile row independently proves the
    // completion. Current effective local trust is rechecked on both paths;
    // neither an account discovery row nor a historical receipt alone suffices.
    check_current()?;
    local_profiles
        .provision(
            profile,
            &koru_store_wire::ProvisionAccountProfile {
                identity_id: credential.owner.account.to_string(),
                // Seeding a local row that does not exist yet is the account
                // copy's remaining job; the identity document replaces it on
                // arrival. See `AccountProfileRow::name`.
                name: summary
                    .name_snapshot
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "Profile".into()),
                locale: summary.locale_snapshot.clone(),
                created_at_millis: now,
            },
        )
        .map_err(|error| {
            if error.code == "profile_not_authorized" {
                Failure::AccessRevoked
            } else {
                Failure::Persistence
            }
        })
}
