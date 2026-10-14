// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use super::*;

// ---------------------------------------------------------------- semantics
//
// Every test here runs against the injected in-memory backend: the suite
// touches no disk. One `MemoryStorage` shared by two logical "channels"
// models the machine-wide file both would read.

#[test]
fn the_default_is_relay_on_when_nothing_was_ever_saved() {
    let storage = MemoryStorage::default();
    assert_eq!(
        load(&storage),
        RelayPreference {
            relay_enabled: true
        }
    );
}

#[test]
fn a_saved_change_loads_back() {
    let storage = MemoryStorage::default();
    save(
        &storage,
        RelayPreference {
            relay_enabled: false,
        },
    )
    .unwrap();
    assert!(!load(&storage).relay_enabled);
    save(
        &storage,
        RelayPreference {
            relay_enabled: true,
        },
    )
    .unwrap();
    assert!(load(&storage).relay_enabled);
}

/// Two channels write concurrently; each write is an atomic whole, so the
/// outcome is whichever write **completed** last — here modeled by both
/// values being encoded first (both writers past their decision) and the
/// completions landing in each order.
#[test]
fn the_last_completed_write_wins_under_concurrent_channel_writers() {
    let off = RelayPreference {
        relay_enabled: false,
    }
    .encode();
    let on = RelayPreference {
        relay_enabled: true,
    }
    .encode();

    let storage = MemoryStorage::default();
    storage.write(&off).unwrap();
    storage.write(&on).unwrap();
    assert!(load(&storage).relay_enabled, "the on write completed last");

    let storage = MemoryStorage::default();
    storage.write(&on).unwrap();
    storage.write(&off).unwrap();
    assert!(
        !load(&storage).relay_enabled,
        "the off write completed last"
    );
}

/// No live cross-channel propagation: the other channel simply reads the
/// shared value on its next start, and sees what was last saved.
#[test]
fn another_channel_reads_what_the_last_writer_saved_on_its_next_start() {
    let shared = MemoryStorage::default();
    // Channel A (the clicked one) turns the relay off and persists it.
    save(
        &shared,
        RelayPreference {
            relay_enabled: false,
        },
    )
    .unwrap();
    // Channel B starts later and performs its startup read of the shared
    // value — a fresh load, no state carried over.
    assert!(!load(&shared).relay_enabled);
}

#[test]
fn a_damaged_or_foreign_file_reads_as_the_default() {
    let storage = MemoryStorage::default();
    storage.write(b"not json at all").unwrap();
    assert!(load(&storage).relay_enabled);
    storage.write(b"{\"some_other_key\": 3}").unwrap();
    assert!(load(&storage).relay_enabled);
}

#[test]
fn unknown_fields_beside_the_value_do_not_hide_it() {
    let storage = MemoryStorage::default();
    storage
        .write(b"{\"relay_enabled\": false, \"added_by_a_newer_build\": 1}")
        .unwrap();
    assert!(!load(&storage).relay_enabled);
}

// ------------------------------------------------------------------- paths
//
// Pure path math: nothing here creates, checks, or opens anything.

#[test]
fn an_explicit_root_selects_the_rooted_location_without_creating_it() {
    let root = Path::new("X:/never-created/stack");
    assert_eq!(
        preference_path(Some(root)),
        Some(root.join("agent").join("network-relay-v1.json"))
    );
}

#[test]
fn the_shared_location_sits_beside_the_channel_segments_under_none_of_them() {
    let base = Path::new("X:/never-created/Koru");
    let path = shared_preference_path_under(base);
    assert_eq!(path, base.join("network-relay-v1.json"));
    for segment in koru_channel::SEGMENTS {
        assert!(
            !path
                .components()
                .any(|component| component.as_os_str() == *segment),
            "the shared value must not live under the {segment} channel"
        );
    }
}

#[test]
fn the_rooted_location_matches_the_agents_other_rooted_files() {
    assert_eq!(
        rooted_preference_path(Path::new("X:/r")),
        Path::new("X:/r").join("agent").join(PREFERENCE_FILE)
    );
}

// -------------------------------------------------- manual smoke check only
//
// The real filesystem adapter's atomic-replacement path is deliberately not
// part of the repository suite (repository tests touch no disk). Its smoke
// check is run by hand:
//
//     cargo test -p koru-relay-preference -- --ignored
//
// which exercises FileStorage end to end in a temporary directory: write,
// read-back, overwrite, and staging-litter cleanup.
#[test]
#[ignore = "manual smoke check: touches a real temporary directory"]
fn file_storage_atomic_replacement_smoke_check() {
    let dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(dir.path().join("deep").join(PREFERENCE_FILE));
    assert_eq!(storage.read(), None);
    save(
        &storage,
        RelayPreference {
            relay_enabled: false,
        },
    )
    .unwrap();
    assert!(!load(&storage).relay_enabled);
    save(
        &storage,
        RelayPreference {
            relay_enabled: true,
        },
    )
    .unwrap();
    assert!(load(&storage).relay_enabled);
    // No staging litter: exactly the destination file remains.
    let entries: Vec<_> = std::fs::read_dir(dir.path().join("deep"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from(PREFERENCE_FILE)]);
}
