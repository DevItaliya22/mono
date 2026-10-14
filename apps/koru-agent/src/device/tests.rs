// SPDX-License-Identifier: MPL-2.0

use super::*;

fn scratch(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("koru-agent-device-tests")
        .join(label);
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("agent").join("device.json")
}

#[test]
fn a_first_run_mints_an_identity_and_writes_it() {
    let path = scratch("first-run");
    let device = Device::open(&path, 1_700_000_000_000).unwrap();
    assert!(path.exists(), "and it is on the disk, not only in memory");
    assert_eq!(device.created_at_millis, 1_700_000_000_000);
    assert!(!device.id.is_empty());
    // A v4 uuid in its storage spelling, which is 36 ASCII characters.
    assert_eq!(device.id.len(), 36, "{}", device.id);
}

#[test]
fn the_id_survives_a_second_open() {
    // **The whole point of the file.** A device that renamed itself on every
    // launch would give phase 2's trust nothing to be about.
    let path = scratch("stable");
    let first = Device::open(&path, 1).unwrap();
    let second = Device::open(&path, 2).unwrap();
    assert_eq!(first, second, "including the stamp: it is not rewritten");
}

#[test]
fn two_devices_are_two_ids() {
    // Minted, never derived. A path- or hostname-derived id looks stable until
    // a backup is restored onto a second machine, and then two devices are one.
    let a = Device::open(&scratch("mint-a"), 1).unwrap();
    let b = Device::open(&scratch("mint-b"), 1).unwrap();
    assert_ne!(a.id, b.id);
}

#[test]
fn a_file_that_will_not_parse_is_replaced_rather_than_fatal() {
    // The opposite of the workspace rule, and deliberately. Losing a device id
    // costs this installation its name in a list; refusing to start would take
    // the hotkey, the overlay and the store's supervisor down with it.
    let path = scratch("corrupt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{ this is not json").unwrap();

    let device = Device::open(&path, 5).unwrap();
    assert!(!device.id.is_empty());
    assert_eq!(device.created_at_millis, 5);
    // ...and the replacement is on the disk, so the next launch is stable again.
    assert_eq!(Device::open(&path, 9).unwrap(), device);
}

#[test]
fn a_file_with_an_empty_id_is_replaced_too() {
    // Valid JSON of the right shape and useless — which `serde` alone would
    // accept. An empty id would reach the wire as a device that is every
    // device.
    let path = scratch("empty-id");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, br#"{"id":"","name":"x","created_at_millis":1}"#).unwrap();
    assert!(!Device::open(&path, 7).unwrap().id.is_empty());
}

#[test]
fn the_directory_is_created_on_the_way() {
    // A first run has no `agent/` yet, and the agent must not need one made for
    // it by hand.
    let path = scratch("makes-its-dir");
    assert!(!path.parent().unwrap().exists());
    Device::open(&path, 1).unwrap();
    assert!(path.parent().unwrap().exists());
}
