// SPDX-License-Identifier: MPL-2.0

//! The bindings a verified capability still has to survive: the comparison
//! between what it claims and the world the request arrived in.
//!
//! The two services know different halves of that world and this is where the
//! difference is written down rather than implied. The object worker holds no
//! account database, so it can compare the capability against **the id in the
//! URL** and nothing else; the API holds the database, so it compares all four.
//! An unknown fact is [`None`] and is not compared — never assumed to match.

use koru_account_session::cloud::blob::BlobCapabilityClaimsV1;
use koru_device::cloud::CloudId;

use crate::BlobCode;

/// What the receiving service actually knows. Every field the caller leaves
/// `None` is a fact it cannot establish, which is a statement about the
/// service, not a pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct BindingWorld {
    /// The upload id or object locator in the request path. Always known: it is
    /// how the request was addressed.
    pub route_subject: Option<CloudId>,
    /// The Workspace the addressed resource belongs to, from the path on a
    /// control route or from the session row on the report seam.
    pub addressed_workspace: Option<CloudId>,
    /// What `cloud_workspaces` says the dataset incarnation is *now*.
    pub current_incarnation: Option<CloudId>,
    /// What `cloud_workspaces` says the Cloud-access epoch is *now*.
    pub current_access_epoch: Option<i64>,
    /// Whether the session has reached a terminal state — aborted, expired, or
    /// already reclaimed.
    pub session_terminal: bool,
}

/// Compare a verified capability against that world.
///
/// Addressing first: a caller that named another object is told so before
/// anything about authorization is evaluated, and a locator that simply does
/// not exist answers the same code, so a denial cannot teach an unauthorised
/// caller which locators are real (G02/G03).
pub fn check_bindings(
    claims: &BlobCapabilityClaimsV1,
    world: &BindingWorld,
) -> Result<(), BlobCode> {
    if world
        .route_subject
        .is_some_and(|subject| subject != claims.sub)
    {
        return Err(BlobCode::WrongObject);
    }
    if world
        .addressed_workspace
        .is_some_and(|workspace| workspace != claims.wsp)
    {
        return Err(BlobCode::WrongWorkspace);
    }
    if world
        .current_incarnation
        .is_some_and(|incarnation| incarnation != claims.inc)
    {
        return Err(BlobCode::StaleIncarnation);
    }
    if world
        .current_access_epoch
        .is_some_and(|epoch| epoch != claims.eph)
    {
        return Err(BlobCode::StaleEpoch);
    }
    if world.session_terminal {
        return Err(BlobCode::UploadExpired);
    }
    Ok(())
}

/// Split one `Authorization` header value into its scheme and its credential.
///
/// The scheme is compared exactly and case-sensitively. An account bearer token
/// sent to a byte route must fail as a *malformed credential* rather than be
/// tried as the other kind (C16): at rung 7 one of these two travels to a third
/// party, and the day that happens is not the day to discover that the schemes
/// were interchangeable.
pub fn credential<'a>(header: Option<&'a str>, scheme: &str) -> Result<&'a str, BlobCode> {
    let value = header.ok_or(BlobCode::CapabilityInvalid)?;
    let (found, credential) = value.split_once(' ').ok_or(BlobCode::CapabilityInvalid)?;
    if found != scheme || credential.is_empty() || credential.contains(' ') {
        return Err(BlobCode::CapabilityInvalid);
    }
    Ok(credential)
}

#[cfg(test)]
mod tests;
