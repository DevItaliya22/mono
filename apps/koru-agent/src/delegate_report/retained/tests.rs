// SPDX-License-Identifier: MPL-2.0

//! A fixture tree, measured. Everything here lives under a temporary
//! directory named for this process; nothing reaches a real root.

use std::fs;

use super::*;

fn temp(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "koru-retained-{}-{label}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_missing_folder_holds_nothing_and_is_not_an_error() {
    let root = temp("missing");
    let held = measure(&root.join(".artifacts").join("nobody"));
    assert_eq!(held, Retained::default());
    assert!(!held.present);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_nested_tree_is_counted_whole() {
    let root = temp("nested");
    let folder = root.join(".artifacts").join("run");
    fs::create_dir_all(folder.join("a").join("b").join("c")).unwrap();
    fs::write(folder.join("one.txt"), vec![b'x'; 10]).unwrap();
    fs::write(folder.join("a").join("two.bin"), vec![b'y'; 100]).unwrap();
    fs::write(
        folder.join("a").join("b").join("c").join("deep"),
        vec![b'z'; 1_000],
    )
    .unwrap();
    fs::create_dir_all(folder.join("empty")).unwrap();

    let held = measure(&folder);
    assert!(held.present);
    assert_eq!(held.files, 3);
    assert_eq!(held.bytes, 1_110);
    assert!(!held.truncated);
    assert_eq!(held.unreadable, 0);

    // A file beside the folder is not this run's disk.
    fs::write(root.join(".artifacts").join("stray"), vec![b'q'; 500]).unwrap();
    assert_eq!(measure(&folder).bytes, 1_110);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_file_where_a_folder_should_be_holds_nothing() {
    let root = temp("notdir");
    let path = root.join("not-a-folder");
    fs::write(&path, b"x").unwrap();
    assert!(!measure(&path).present);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn totals_add_and_carry_the_flags() {
    let mut total = Retained::default();
    total.add(Retained {
        present: true,
        files: 2,
        bytes: 30,
        truncated: false,
        unreadable: 1,
    });
    total.add(Retained {
        present: true,
        files: 3,
        bytes: 12,
        truncated: true,
        unreadable: 0,
    });
    assert_eq!(total.files, 5);
    assert_eq!(total.bytes, 42);
    assert!(total.truncated, "a truncated walk taints the total");
    assert_eq!(total.unreadable, 1);
}
