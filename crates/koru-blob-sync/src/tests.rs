// SPDX-License-Identifier: MPL-2.0

//! The few claims that belong to the crate rather than to one of its modules.

use crate::fakes::Harness;

/// The two schemes are different words, and this crate uses the right one.
///
/// A misrouted credential must fail as a malformed credential rather than be
/// tried as the other kind: at rung 7 one of these travels to a third party.
#[test]
fn the_byte_plane_never_borrows_the_account_scheme() {
    assert_eq!(koru_cloud_blob::CAPABILITY_SCHEME, "Capability");
    assert_ne!(koru_cloud_blob::CAPABILITY_SCHEME, "Bearer");
}

/// The chunk a client uses is the contract's, and a client never invents one.
#[test]
fn the_chunk_is_the_contracts() {
    let harness = Harness::new();
    assert_eq!(harness.engine.walks(), 0);
    assert_eq!(koru_cloud_blob::CHUNK, 2 * 1024 * 1024);
}
