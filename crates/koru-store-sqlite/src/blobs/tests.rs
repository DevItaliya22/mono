// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::fixtures::{BIDI_LINE, SAMPLES, UNICODE_FILENAME};

fn store() -> (tempfile::TempDir, SqliteBlobs) {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SqliteBlobs::new(dir.path().join("blobs"));
    (dir, blobs)
}

// ------------------------------------------------------------------ identity

#[test]
fn the_same_bytes_are_the_same_blob() {
    let (_dir, blobs) = store();
    let one = blobs.put(b"hello").unwrap();
    let two = blobs.put(b"hello").unwrap();
    assert_eq!(one, two);
    assert_eq!(blobs.get(&one).unwrap(), b"hello");
}

#[test]
fn different_bytes_are_different_blobs() {
    let (_dir, blobs) = store();
    assert_ne!(blobs.put(b"hello").unwrap(), blobs.put(b"hellp").unwrap());
}

#[test]
fn a_hash_says_which_algorithm_produced_it() {
    // The reason the algorithm is in the value: adopting a second one has to be
    // additive, or every document already written is orphaned.
    let (_dir, blobs) = store();
    let hash = blobs.put(b"hello").unwrap();
    assert_eq!(hash.algorithm(), "b3");
    assert_eq!(hash.digest().len(), 64);
    assert!(hash.as_str().starts_with("b3:"));
}

#[test]
fn the_hash_is_blake3_and_not_the_legacy_fnv() {
    let (_dir, blobs) = store();
    let hash = blobs.put(b"hello").unwrap();
    assert_eq!(hash.digest(), blake3::hash(b"hello").to_hex().to_string());
}

#[test]
fn an_empty_blob_is_a_blob() {
    let (_dir, blobs) = store();
    let hash = blobs.put(b"").unwrap();
    assert!(blobs.has(&hash).unwrap());
    assert_eq!(blobs.get(&hash).unwrap(), Vec::<u8>::new());
}

// ------------------------------------------------------------------ on disk

#[test]
fn the_on_disk_name_is_ascii_and_derived_from_the_content() {
    // Nothing a person typed reaches a path component.
    let (_dir, blobs) = store();
    let hash = blobs.put(b"hello").unwrap();
    let name = std::fs::read_dir(blobs.objects_dir())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .to_string();

    assert!(name.is_ascii(), "{name}");
    assert!(
        !name.contains(':'),
        "a colon is illegal in a Windows path: {name}"
    );
    assert_eq!(name, hash.file_name());
    assert!(name.starts_with("b3-"), "{name}");
}

#[test]
fn a_unicode_filename_never_becomes_a_path() {
    // The filename is metadata. This asserts the store's directory holds
    // nothing named after one — including one carrying a separator or a bidi
    // override.
    let (_dir, blobs) = store();
    for name in [UNICODE_FILENAME, BIDI_LINE, "../escape.txt", "C:\\absolute"] {
        blobs.put(name.as_bytes()).unwrap();
    }
    for entry in std::fs::read_dir(blobs.objects_dir()).unwrap() {
        let found = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(found.is_ascii(), "{found}");
        assert!(found.starts_with("b3-"), "{found}");
    }
}

#[test]
fn nothing_partial_is_left_behind() {
    // Written beside and renamed, so a reader never sees a partial blob under a
    // name that promises those exact bytes.
    let (_dir, blobs) = store();
    blobs.put(b"hello").unwrap();
    let leftovers: Vec<String> = std::fs::read_dir(blobs.objects_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_chunked_upload_matches_put() {
    let (_dir, blobs) = store();
    let mut pending = blobs.begin().unwrap();
    pending.write_chunk(b"chunk ").unwrap();
    pending.write_chunk(b"by chunk").unwrap();
    let chunked = pending.finish().unwrap();
    let one_shot = blobs.put(b"chunk by chunk").unwrap();

    assert_eq!(chunked.hash, one_shot);
    assert_eq!(chunked.size, 14);
    assert_eq!(blobs.get(&chunked.hash).unwrap(), b"chunk by chunk");
}

#[test]
fn an_over_limit_or_cancelled_upload_leaves_nothing_published() {
    let (_dir, blobs) = store();
    let mut over_limit = blobs.begin_with_limit(4).unwrap();
    over_limit.write_chunk(b"four").unwrap();
    assert_eq!(
        over_limit.write_chunk(b"!").unwrap_err().code(),
        Code::PayloadTooLarge
    );
    drop(over_limit);

    let mut cancelled = blobs.begin().unwrap();
    cancelled.write_chunk(b"unfinished").unwrap();
    drop(cancelled);

    assert!(
        std::fs::read_dir(blobs.objects_dir())
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn overlapping_equal_uploads_deduplicate_and_remove_both_parts() {
    let (_dir, blobs) = store();
    let mut first = blobs.begin().unwrap();
    let mut second = blobs.begin().unwrap();
    for pending in [&mut first, &mut second] {
        pending.write_chunk(b"same ").unwrap();
        pending.write_chunk(b"content").unwrap();
    }

    let first = first.finish().unwrap();
    let second = second.finish().unwrap();
    assert_eq!(first.hash, second.hash);

    let entries = std::fs::read_dir(blobs.objects_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![first.hash.file_name()]);
    assert!(!entries.iter().any(|name| name.ends_with(".part")));
}

#[test]
fn blobs_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let hash = {
        let blobs = SqliteBlobs::new(dir.path().join("blobs"));
        blobs.put("四半期計画".as_bytes()).unwrap()
    };
    let blobs = SqliteBlobs::new(dir.path().join("blobs"));
    assert_eq!(blobs.get(&hash).unwrap(), "四半期計画".as_bytes());
}

// ------------------------------------------------------------------ absence

#[test]
fn a_blob_that_is_not_there_says_so() {
    let (_dir, blobs) = store();
    let absent = SqliteBlobs::hash_of(b"never stored");
    assert!(!blobs.has(&absent).unwrap());
    assert_eq!(blobs.get(&absent).unwrap_err().code(), Code::BlobNotFound);
    assert_eq!(
        blobs.quarantine(&absent).unwrap_err().code(),
        Code::BlobNotFound
    );
}

// ------------------------------------------------------------------ verifying

#[test]
fn an_intact_blob_verifies() {
    let (_dir, blobs) = store();
    let hash = blobs.put(b"intact").unwrap();
    assert!(blobs.verify(&hash).unwrap());
}

#[test]
fn a_rotted_blob_fails_verification() {
    // A blob's identity *is* its checksum, so this needs no separate column.
    let (_dir, blobs) = store();
    let hash = blobs.put(b"intact").unwrap();
    std::fs::write(blobs.objects_dir().join(hash.file_name()), b"rotted").unwrap();
    assert!(!blobs.verify(&hash).unwrap());
}

#[test]
fn every_supported_language_round_trips_as_bytes() {
    let (_dir, blobs) = store();
    for sample in SAMPLES {
        let hash = blobs.put(sample.body.as_bytes()).unwrap();
        assert_eq!(
            blobs.get(&hash).unwrap(),
            sample.body.as_bytes(),
            "{}",
            sample.tag
        );
        assert!(blobs.verify(&hash).unwrap(), "{}", sample.tag);
    }
}

// ------------------------------------------------------------------ quarantine

#[test]
fn quarantine_copies_and_never_removes() {
    // Recovery must not be able to destroy the thing it was called to
    // preserve. A reference to nothing is worse than a reference to something
    // broken.
    let (_dir, blobs) = store();
    let hash = blobs.put(b"damaged").unwrap();
    std::fs::write(blobs.objects_dir().join(hash.file_name()), b"rotted").unwrap();

    let location = blobs.quarantine(&hash).unwrap();
    assert!(
        blobs.has(&hash).unwrap(),
        "the damaged blob was moved rather than copied"
    );
    assert!(location.starts_with("quarantine/"), "{location}");

    // The copy is what somebody diagnoses, and it holds the damaged bytes.
    let copied = blobs.dir().join(&location);
    assert!(copied.exists(), "{location}");
    assert_eq!(std::fs::read(copied).unwrap(), b"rotted");
}

#[test]
fn a_quarantine_location_is_opaque_and_not_an_absolute_path() {
    // It goes into a report, which a client may read. It must not hand out the
    // store's physical layout.
    let (_dir, blobs) = store();
    let hash = blobs.put(b"x").unwrap();
    let location = blobs.quarantine(&hash).unwrap();
    assert!(!location.contains(':'), "{location}");
    assert!(!location.starts_with('/'), "{location}");
    assert!(!location.contains(".."), "{location}");
}

// --------------------------------------------------------------- ranged read

#[test]
fn a_range_reads_only_its_own_window() {
    let (_dir, blobs) = store();
    let hash = blobs.put(b"0123456789").unwrap();
    assert_eq!(blobs.read_range(&hash, 0, 4).unwrap(), b"0123");
    assert_eq!(blobs.read_range(&hash, 4, 4).unwrap(), b"4567");
    assert_eq!(blobs.read_range(&hash, 0, 100).unwrap(), b"0123456789");
}

#[test]
fn a_range_at_or_past_the_end_is_empty_rather_than_an_error() {
    // The reader walking a blob learns its length from the blob's metadata, so
    // a request landing at the end is the ordinary last step of a loop, not a
    // caller mistake to distinguish.
    let (_dir, blobs) = store();
    let hash = blobs.put(b"0123456789").unwrap();
    assert_eq!(blobs.read_range(&hash, 10, 4).unwrap(), b"");
    assert_eq!(blobs.read_range(&hash, 4_000, 4).unwrap(), b"");
    assert_eq!(blobs.read_range(&hash, 8, 4).unwrap(), b"89");
}

#[test]
fn a_range_of_an_absent_blob_says_so() {
    let (_dir, blobs) = store();
    let absent = blobs.put(b"present").unwrap();
    let (_other_dir, empty) = store();
    assert_eq!(
        empty.read_range(&absent, 0, 4).unwrap_err().code(),
        Code::BlobNotFound
    );
}

#[test]
fn a_ranged_walk_reassembles_the_original_bytes() {
    // What a peer transfer does, in miniature: the whole payload arrives as
    // fixed windows and must come back byte-identical.
    let (_dir, blobs) = store();
    let payload: Vec<u8> = (0..5_000).map(|index| (index % 251) as u8).collect();
    let hash = blobs.put(&payload).unwrap();
    let mut rebuilt = Vec::new();
    let mut offset = 0u64;
    loop {
        let chunk = blobs.read_range(&hash, offset, 512).unwrap();
        if chunk.is_empty() {
            break;
        }
        offset += chunk.len() as u64;
        rebuilt.extend_from_slice(&chunk);
    }
    assert_eq!(rebuilt, payload);
}

// ------------------------------------------------------------------ promotion

/// A file in the layout an older build wrote, under the name it claims.
fn legacy(dir: &std::path::Path, hash: &BlobHash, bytes: &[u8]) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(hash.file_name());
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn promotion_copies_the_candidate_and_leaves_it_where_it_is() {
    let (dir, blobs) = store();
    let legacy_dir = dir.path().join("workspaces").join("one").join("blobs");
    let hash = SqliteBlobs::hash_of(b"a picture");
    let candidate = legacy(&legacy_dir, &hash, b"a picture");

    assert_eq!(
        blobs.promote(&candidate, &hash).unwrap(),
        LegacyPromotion::Installed
    );
    assert_eq!(blobs.get(&hash).unwrap(), b"a picture");
    assert!(candidate.exists(), "the candidate was moved, not copied");

    // Idempotent, and cheap the second time: the object is already there, so
    // the candidate is not even opened.
    assert_eq!(
        blobs.promote(&candidate, &hash).unwrap(),
        LegacyPromotion::Installed
    );
    assert_eq!(
        std::fs::read_dir(blobs.objects_dir()).unwrap().count(),
        1,
        "one object, however many times it is promoted"
    );
}

#[test]
fn a_candidate_that_does_not_hash_to_its_name_is_refused_and_untouched() {
    let (dir, blobs) = store();
    let legacy_dir = dir.path().join("workspaces").join("one").join("blobs");
    let hash = SqliteBlobs::hash_of(b"what the name promises");
    let candidate = legacy(&legacy_dir, &hash, b"what the file holds");

    let outcome = blobs.promote(&candidate, &hash).unwrap();
    assert_eq!(
        outcome,
        LegacyPromotion::Mismatch {
            found: SqliteBlobs::hash_of(b"what the file holds")
        },
        "the refusal names what was actually there"
    );
    assert!(!blobs.has(&hash).unwrap(), "nothing was installed");
    assert_eq!(std::fs::read(&candidate).unwrap(), b"what the file holds");
    assert_eq!(
        std::fs::read_dir(blobs.staging_dir()).unwrap().count(),
        0,
        "the rejected copy did not stay in staging"
    );
}

#[test]
fn a_missing_candidate_is_not_an_error() {
    let (dir, blobs) = store();
    let hash = SqliteBlobs::hash_of(b"never stored anywhere");
    assert_eq!(
        blobs
            .promote(&dir.path().join("nowhere").join(hash.file_name()), &hash)
            .unwrap(),
        LegacyPromotion::NoCandidate
    );
}

// --------------------------------------------------------------- crash sweeps

/// Backdate a file so a sweep sees it as abandoned. `filetime` is not a
/// dependency here, so the clock moves instead of the file.
fn long_ago() -> Timestamp {
    Timestamp::from_millis(Timestamp::now().millis() + 10 * PENDING_MAX_AGE_MS)
}

#[test]
fn a_sweep_leaves_fresh_pending_work_alone() {
    let (_dir, blobs) = store();
    let mut pending = blobs.begin().unwrap();
    pending.write_chunk(b"still going").unwrap();

    let swept = blobs
        .sweep_staging(Timestamp::now(), PENDING_MAX_AGE_MS)
        .unwrap();
    assert_eq!(swept, PendingSweep::default());
    assert_eq!(std::fs::read_dir(blobs.staging_dir()).unwrap().count(), 1);

    // And it still completes, which is the point of leaving it alone.
    let done = pending.finish().unwrap();
    assert_eq!(blobs.get(&done.hash).unwrap(), b"still going");
}

#[test]
fn a_sweep_removes_an_abandoned_upload_that_nothing_can_name() {
    let (_dir, blobs) = store();
    let mut pending = blobs.begin().unwrap();
    pending.write_chunk(b"a process that died").unwrap();
    // A crash: the pending file outlives the value that would have removed it.
    std::mem::forget(pending);

    let swept = blobs.sweep_staging(long_ago(), PENDING_MAX_AGE_MS).unwrap();
    assert_eq!(swept.removed, 1);
    assert_eq!(swept.recovered, 0);
    assert_eq!(swept.retained, 0);
    assert_eq!(std::fs::read_dir(blobs.staging_dir()).unwrap().count(), 0);
}

#[test]
fn a_sweep_finishes_an_abandoned_promotion_rather_than_discarding_it() {
    // The rule is "the final object path is checked first": a promotion whose
    // object never landed holds bytes, and bytes are the one thing this
    // codebase does not throw away to tidy up.
    let (_dir, blobs) = store();
    let hash = SqliteBlobs::hash_of(b"interrupted");
    std::fs::create_dir_all(blobs.staging_dir()).unwrap();
    let stranded = blobs
        .staging_dir()
        .join(format!(".promote.{}.999.0.part", hash.file_name()));
    std::fs::write(&stranded, b"interrupted").unwrap();

    let swept = blobs.sweep_staging(long_ago(), PENDING_MAX_AGE_MS).unwrap();
    assert_eq!(swept.recovered, 1);
    assert_eq!(blobs.get(&hash).unwrap(), b"interrupted");
    assert!(!stranded.exists());
}

#[test]
fn a_sweep_keeps_a_stale_promotion_it_cannot_account_for() {
    let (_dir, blobs) = store();
    let hash = SqliteBlobs::hash_of(b"what the name says");
    std::fs::create_dir_all(blobs.staging_dir()).unwrap();
    let stranded = blobs
        .staging_dir()
        .join(format!(".promote.{}.999.1.part", hash.file_name()));
    std::fs::write(&stranded, b"a truncated copy").unwrap();

    let swept = blobs.sweep_staging(long_ago(), PENDING_MAX_AGE_MS).unwrap();
    assert_eq!(swept.retained, 1);
    assert_eq!(swept.removed, 0);
    assert!(
        !blobs.has(&hash).unwrap(),
        "a partial copy is not the object"
    );
    assert!(
        stranded.exists(),
        "bytes nobody can explain are still bytes"
    );
}

#[test]
fn a_sweep_touches_nothing_it_did_not_write() {
    let (_dir, blobs) = store();
    std::fs::create_dir_all(blobs.staging_dir()).unwrap();
    let theirs = blobs.staging_dir().join("someone-elses-notes.txt");
    std::fs::write(&theirs, b"not ours").unwrap();

    let swept = blobs.sweep_staging(long_ago(), PENDING_MAX_AGE_MS).unwrap();
    assert_eq!(swept, PendingSweep::default());
    assert!(theirs.exists());
}

#[test]
fn a_pending_file_is_never_at_the_object_address() {
    // "Pending files are never renderable" as a path assertion: staging and
    // the objects directory are disjoint, so a reader walking objects cannot
    // see work in progress at any age.
    let (_dir, blobs) = store();
    let mut pending = blobs.begin().unwrap();
    pending.write_chunk(b"half a picture").unwrap();
    assert_eq!(std::fs::read_dir(blobs.objects_dir()).unwrap().count(), 0);
    assert_ne!(blobs.objects_dir(), blobs.staging_dir());
    let done = pending.finish().unwrap();
    assert_eq!(std::fs::read_dir(blobs.objects_dir()).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(blobs.staging_dir()).unwrap().count(), 0);
    assert_eq!(blobs.get(&done.hash).unwrap(), b"half a picture");
}

// ------------------------------------------------------------------- release

/// **The one destructive method on this port, doing exactly what it says.**
/// Everything else here refuses to lose bytes — `quarantine` copies rather
/// than moves — so the boundary of this one is worth pinning.
#[test]
fn release_removes_the_bytes_and_nothing_else() {
    let (_dir, blobs) = store();
    let kept = blobs.put(b"the one that stays").unwrap();
    let released = blobs.put(b"the one that goes").unwrap();

    blobs.release(&released).unwrap();

    assert!(!blobs.has(&released).unwrap(), "the bytes are gone");
    assert_eq!(
        blobs.get(&released).unwrap_err().code(),
        Code::BlobNotFound,
        "the same answer a device that never held them gives",
    );
    // A device that never held these bytes reports the same thing, which is
    // what makes them fetchable again rather than lost.
    assert!(blobs.open(&released).is_err());
    assert!(blobs.has(&kept).unwrap(), "its neighbour is untouched");
    assert_eq!(blobs.get(&kept).unwrap(), b"the one that stays");
}

/// Releasing what is already absent is the state the caller asked for, so it
/// succeeds. A retry after a partial sweep must not fail on the work already
/// done.
#[test]
fn releasing_an_absent_blob_is_success() {
    let (_dir, blobs) = store();
    let absent = SqliteBlobs::hash_of(b"never stored");
    blobs.release(&absent).unwrap();
    blobs.release(&absent).unwrap();
}

/// The same bytes can be stored again afterwards, under the same address.
/// Reclaiming space must not poison the content address for the re-download.
#[test]
fn released_bytes_can_be_stored_again() {
    let (_dir, blobs) = store();
    let bytes = SAMPLES[0].body.as_bytes();
    let hash = blobs.put(bytes).unwrap();
    blobs.release(&hash).unwrap();
    assert!(!blobs.has(&hash).unwrap());

    let again = blobs.put(bytes).unwrap();
    assert_eq!(again, hash, "content addressing survives a release");
    assert!(blobs.has(&hash).unwrap());
    assert_eq!(blobs.get(&hash).unwrap(), bytes);
    assert!(blobs.verify(&hash).unwrap());
}
