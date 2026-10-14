// SPDX-License-Identifier: MPL-2.0

use super::*;

/// §8's table, asserted as numbers rather than as prose. The two compile-time
/// assertions in `lib.rs` already refuse a contradiction; this one refuses a
/// silent *change*, which a compile-time assertion cannot see.
#[test]
fn the_derived_bounds_are_the_ones_the_decisions_took() {
    assert_eq!(SLOWEST_SUPPORTED_LINK_BYTES_PER_SECOND, 16_000);
    assert_eq!(CAPABILITY_LIFETIME_SECS, 300);
    assert_eq!(CHUNK, 2 * 1024 * 1024);
    assert_eq!(SAFETY, 2);
    assert_eq!(MAX_BLOB_SIZE, 2 * 1024 * 1024 * 1024);
    assert_eq!(RESUME_WINDOW_MILLIS, 7 * 24 * 60 * 60 * 1_000);
}

/// The derivation itself, written out: a device on the slowest link Koru
/// promises to finish on completes one chunk inside one capability with the
/// safety margin to spare, and a chunk twice this size would not.
#[test]
fn the_slowest_promised_link_finishes_a_chunk_inside_one_capability() {
    let budget = SLOWEST_SUPPORTED_LINK_BYTES_PER_SECOND * CAPABILITY_LIFETIME_SECS as u64;
    assert!(CHUNK * SAFETY <= budget);
    assert!(
        CHUNK * 2 * SAFETY > budget,
        "4 MiB would strand that device"
    );
    // A 2 GiB file becomes 1,024 requests rather than 512, which costs nothing.
    assert_eq!(MAX_BLOB_SIZE / CHUNK, 1_024);
}

/// There is one ceiling and it is the Store's. Two numbers in two files is the
/// failure D243 exists to prevent, so this reads the Store's constant rather
/// than restating it.
#[test]
fn the_cloud_ceiling_is_the_store_ceiling() {
    assert_eq!(
        MAX_BLOB_SIZE,
        koru_store_core::ports::BLOB_UPLOAD_BODY_LIMIT
    );
}

/// Staging outlives the session that owns it, so a session can never be told to
/// resume from bytes that were reclaimed underneath it.
#[test]
fn staging_is_reclaimed_strictly_after_the_session_expires() {
    assert!(STAGING_RECLAIM_MILLIS > RESUME_WINDOW_MILLIS);
}

/// Three schemes, three words, none a prefix of another. A misrouted credential
/// must fail loudly, and the cheapest way to guarantee that is for no scheme to
/// be mistakable for another by a lenient parser.
#[test]
fn the_three_authentication_schemes_are_distinct() {
    let schemes = [CAPABILITY_SCHEME, WORKER_SCHEME, "Bearer"];
    for (index, scheme) in schemes.iter().enumerate() {
        for (other_index, other) in schemes.iter().enumerate() {
            if index != other_index {
                assert!(!scheme.starts_with(other), "{scheme} vs {other}");
                assert!(
                    !scheme.eq_ignore_ascii_case(other),
                    "{scheme} vs {other} differ by more than case"
                );
            }
        }
    }
}
