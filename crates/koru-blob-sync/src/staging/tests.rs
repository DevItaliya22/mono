// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::time::Duration;

use crate::fakes::hash_of;

fn area() -> (tempfile::TempDir, StagingArea) {
    let root = tempfile::tempdir().expect("a private root");
    let area = StagingArea::new(root.path().join("blobs").join("staging"));
    area.ensure().expect("provisioned");
    (root, area)
}

/// **`b3-<digest>`, never `b3:<digest>`.** The colon is the NTFS
/// alternate-data-stream delimiter, and a path that carried one would fail on
/// exactly one of the two platforms this ships on first.
#[test]
fn a_staging_file_is_named_with_the_filename_spelling() {
    let hash = hash_of(b"a picture");
    let name = part_name(&hash);
    assert!(!name.contains(':'), "{name}");
    assert!(name.starts_with("b3-"), "{name}");
    assert!(name.ends_with(DOWNLOAD_SUFFIX), "{name}");
    assert_eq!(parse_part_name(&name).as_ref(), Some(&hash));
}

/// An unrecognised pending file is not interpreted, parsed or trusted.
#[test]
fn only_this_crates_own_names_parse() {
    for hostile in [
        "b3-not-hex.download.part",
        "../escape.download.part",
        "b3-aa.download.part.exe",
        "b3:aa.download.part",
        "b3-.download.part",
        "",
        &format!("b3-{}.download.part", "a".repeat(200)),
    ] {
        assert!(parse_part_name(hostile).is_none(), "{hostile}");
    }
}

/// The synced length is the truth, and it is read from the disk rather than
/// remembered.
#[test]
fn the_length_on_disk_is_the_resume_point() {
    let (_root, area) = area();
    let bytes = vec![7u8; 5_000];
    let hash = hash_of(&bytes);
    assert_eq!(area.synced_length(&hash).unwrap(), 0);
    assert_eq!(area.append(&hash, 0, &bytes[..2_000]).unwrap(), 2_000);
    assert_eq!(area.synced_length(&hash).unwrap(), 2_000);
    // An append at anything but the current length would leave a hole the
    // final hash would blame on the network.
    assert!(area.append(&hash, 0, &bytes[..10]).is_err());
    assert_eq!(area.append(&hash, 2_000, &bytes[2_000..]).unwrap(), 5_000);
    assert!(area.hashes_to(&hash).unwrap());
}

/// **S11's half of the staging area.** Bad bytes leave staging altogether, so
/// the next resume cannot append to them; the file is still on the disk, under
/// quarantine, because a person may want to know what arrived.
#[test]
fn a_mismatch_leaves_staging_and_survives_in_quarantine() {
    let (_root, area) = area();
    let hash = hash_of(b"what was asked for");
    area.append(&hash, 0, b"what arrived instead").unwrap();
    assert!(!area.hashes_to(&hash).unwrap());
    let location = area.quarantine(&hash).unwrap();
    assert!(
        !area.part_path(&hash).exists(),
        "bad bytes stayed in staging"
    );
    assert!(std::path::Path::new(&location).exists());
    assert_eq!(
        area.synced_length(&hash).unwrap(),
        0,
        "a resume starts over"
    );
}

/// Startup recovery: keep what is still referenced, remove what is not and has
/// been abandoned, and quarantine a name of ours this build cannot parse.
#[test]
fn a_sweep_keeps_removes_and_quarantines() {
    let (_root, area) = area();
    let wanted = hash_of(b"still referenced");
    let forgotten = hash_of(b"nothing references this");
    area.append(&wanted, 0, b"half of it").unwrap();
    area.append(&forgotten, 0, b"half of that").unwrap();
    let malformed = format!("b3-not-a-digest{}", crate::staging::DOWNLOAD_SUFFIX);
    std::fs::write(area.staging_dir().join(&malformed), b"?").unwrap();

    let referenced = std::collections::BTreeSet::from([wanted.as_str().to_owned()]);
    let (kept, removed, quarantined) = area.sweep(&referenced, Duration::ZERO).unwrap();
    assert_eq!((kept, removed, quarantined), (1, 1, 1));
    assert!(area.part_path(&wanted).exists());
    assert!(!area.part_path(&forgotten).exists());
    assert!(area.quarantine_dir().join(&malformed).exists());
}

/// **The directory is the Store's too.** Its in-flight imports stage here
/// under its own names, and Finder leaves `.DS_Store` wherever it looks; a
/// sweep that moved them would fail an import at its rename, so a name that
/// is not ours is not touched at any age.
#[test]
fn a_sweep_leaves_names_it_did_not_write_alone() {
    let (_root, area) = area();
    area.ensure().unwrap();
    let foreign = [".pending.4242.7.part", ".promote.4242.8.part", ".DS_Store"];
    for name in foreign {
        std::fs::write(area.staging_dir().join(name), b"someone else's").unwrap();
    }

    let counts = area
        .sweep(&std::collections::BTreeSet::new(), Duration::ZERO)
        .unwrap();
    assert_eq!(counts, (0, 0, 0));
    for name in foreign {
        assert!(area.staging_dir().join(name).exists(), "{name} was moved");
        assert!(!area.quarantine_dir().join(name).exists());
    }
}

/// **One Workspace's references are not the Store root's.** A part file is
/// named by hash and shared, so the file this Workspace does not reference may
/// be another Workspace's download in flight — kept until nothing has
/// written it for the abandonment window.
#[test]
fn an_unreferenced_download_still_being_written_is_kept() {
    let (_root, area) = area();
    let theirs = hash_of(b"another Workspace's picture");
    area.append(&theirs, 0, b"the first chunk").unwrap();

    let counts = area
        .sweep(
            &std::collections::BTreeSet::new(),
            crate::staging::ABANDONED_AFTER,
        )
        .unwrap();
    assert_eq!(counts, (1, 0, 0));
    assert_eq!(area.synced_length(&theirs).unwrap(), 15);
}

/// A quarantined file is not swept by the thing that sweeps staging, or a
/// recovery would destroy the evidence it was called to preserve.
#[test]
fn quarantine_is_not_inside_staging() {
    let (_root, area) = area();
    assert!(!area.quarantine_dir().starts_with(area.staging_dir()));
}

/// **The storage bound, exactly as it is derived.** Publishing a download
/// streams the finished staging file into the object store, so at that moment
/// both copies exist: what is still to be written here, plus the whole object,
/// plus one chunk of slack so the last append does not lose a race with
/// something else on the device.
///
/// Asserted against the volume's own free space rather than a number, so the
/// test says what the rule is instead of what one machine happened to have.
/// The margins are loose on purpose — free space moves while a suite runs, and
/// a test that asserts the exact byte is a test that fails on a busy machine.
#[test]
fn a_download_that_would_not_fit_with_its_published_copy_is_refused() {
    let (_root, area) = area();
    let available = koru_free_space::available(area.staging_dir()).expect("a readable volume");
    let chunk = koru_cloud_blob::CHUNK;
    assert!(available > 8 * chunk, "this volume is already full");

    // Comfortably inside it, counting the copy and the slack.
    let quarter = available / 4;
    assert!(area.will_fit(quarter, quarter).expect("a free-space read"));

    // Past the end of the volume, however it is split.
    assert!(
        !area.will_fit(available, 0).expect("a free-space read"),
        "a download the size of the whole volume leaves no room for its copy",
    );
    let half = available / 2 + chunk;
    assert!(
        !area.will_fit(half, half).expect("a free-space read"),
        "the copy `install_downloaded` makes was not counted",
    );
    // A 2 GiB file on a device that has nothing is refused rather than half
    // written, and the arithmetic saturates rather than wrapping into a yes.
    assert!(
        !area
            .will_fit(u64::MAX, u64::MAX)
            .expect("a free-space read")
    );
}
