// SPDX-License-Identifier: MPL-2.0

//! The blocking decision for a local Profile whose hosted lifecycle is deleted.
//!
//! Removing the local replica stays in `app::profile_removal`. Continuing
//! offline is deliberately separate: it retains every local byte and performs
//! one compare-and-clear against the exact Koru Identity binding discovered by
//! reconciliation. A delayed decision can therefore never detach a Profile
//! that was connected to a different identity meanwhile.

pub(crate) fn continue_offline(
    root: Option<&std::path::Path>,
    profile: koru_core::ProfileId,
    expected_identity_id: &str,
) -> Result<(), String> {
    let expected = koru_store_core::ProfileAccountBinding::parse_identity_id(expected_identity_id)
        .map_err(|error| error.to_string())?;
    let client = match root {
        Some(root) => koru_api_client::Client::open_at(koru_workspace::fromstore::segment(), root),
        None => koru_api_client::Client::open(koru_workspace::fromstore::segment()),
    }
    .map_err(|error| error.to_string())?;
    client
        .store()
        .clear_profile_account(
            koru_store_core::ids::ProfileId::from_uuid(profile.as_uuid()),
            expected,
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}
