// SPDX-License-Identifier: MPL-2.0

//! Repair derived catalog timestamps without authoring or importing CRDT ops.

use super::*;

pub(super) fn reconcile_creation_times<C: Catalog>(catalog: &mut C) -> Result<()> {
    // Include removed Profiles without restoring them. Only catalog data is
    // read; this never opens a Workspace or walks document content on a timer.
    for profile in catalog.profiles_including_removed()? {
        let unit = ProfileUnitRef::Identity {
            profile: profile.id,
        };
        let Some(state) = catalog.profile_unit(unit)? else {
            continue; // A placeholder is not authority to invent an identity.
        };
        let identity = match retained_identity(unit, &state.snapshot) {
            Ok(identity) => identity,
            Err(error) => {
                // An unopened corrupt/newer unit must not disable unrelated
                // Profiles. Normal access still reports its original refusal.
                tracing::warn!(
                    profile = %profile.id.as_uuid(),
                    error_code = error.code.as_str(),
                    "store profile: creation timestamp projection repair skipped"
                );
                continue;
            }
        };
        if profile.created_at != identity.created_at
            && catalog.reconcile_profile_creation_time(
                profile.id,
                identity.created_at,
                &state.snapshot,
            )?
        {
            tracing::info!(
                profile = %profile.id.as_uuid(),
                previous_created_at_ms = profile.created_at.millis(),
                identity_created_at_ms = identity.created_at.millis(),
                "store profile: creation timestamp projection repaired"
            );
        }
    }
    Ok(())
}

fn retained_identity(unit: ProfileUnitRef, snapshot: &[u8]) -> Result<Profile> {
    let identity = IdentityManifest::from_snapshot(snapshot)?;
    if identity.unit() != unit {
        return Err(Error::bare(Code::ProfileUnitScopeMismatch));
    }
    if identity.is_read_only() {
        return Err(Error::bare(Code::SchemaTooNew));
    }
    identity.profile()
}
