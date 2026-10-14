// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_name_with_a_path_separator_cannot_escape_the_filename() {
    let n = suggested_name("Q3/Q4 planning", "20260804-120000");
    assert!(!n.contains('/') && !n.contains('\\'), "{n}");
    assert!(n.ends_with(".koru"));
    assert!(n.starts_with("Q3-Q4 planning"), "{n}");
}

#[test]
fn a_name_with_nothing_usable_in_it_still_makes_a_filename() {
    assert_eq!(
        suggested_name("///", "20260804-120000"),
        "workspace 20260804-120000.koru"
    );
    assert_eq!(
        suggested_name("", "20260804-120000"),
        "workspace 20260804-120000.koru"
    );
}

#[test]
fn a_complete_bundle_replaces_the_destination_and_leaves_no_part() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("Roadmap.koru");
    fs::write(&out, b"old").unwrap();

    write_bundle(&out, b"complete store bundle").unwrap();

    assert_eq!(fs::read(&out).unwrap(), b"complete store bundle");
    assert!(!part_path(&out).exists());
}
