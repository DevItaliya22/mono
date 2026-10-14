// SPDX-License-Identifier: MPL-2.0

use super::*;

fn id(n: u8) -> ProfileId {
    ProfileId::parse(&format!("6f0e5b2c-1d4a-4b2e-9c11-8a1f2e3d4c{n:02}")).unwrap()
}

fn rec(n: u8, name: &str, dir: &str) -> ProfileRecord {
    ProfileRecord {
        id: id(n),
        name: name.into(),
        email: Some(format!("{}@example.com", name.to_lowercase())),
        last_opened: Some(format!("2026080{n}-120000")),
        dir: PathBuf::from(dir),
    }
}

#[test]
fn a_registry_round_trips() {
    let records = vec![
        rec(1, "Work", r"C:\Users\sam\Koru\Work"),
        rec(2, "Personal", r"D:\Dropbox\Koru\Personal"),
    ];
    assert_eq!(parse(&body(&records, None)), records);
}

/// The file is meant to be opened and edited when something has gone wrong, so
/// the parser has to survive what a person's editor does to it.
#[test]
fn a_hand_edited_registry_still_parses() {
    let text = "
        # a comment

        6f0e5b2c-1d4a-4b2e-9c11-8a1f2e3d4c01|Work|sam@acme.com|20260801-120000|C:\\Koru\\Work
           6f0e5b2c-1d4a-4b2e-9c11-8a1f2e3d4c02  |  Personal  |  |  | D:\\Koru\\Personal
    ";
    let got = parse(text);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].name, "Work", "no padding needed");
    assert_eq!(got[1].name, "Personal", "and padding is fine too");
    assert_eq!(got[1].email, None, "an empty field is absent, not blank");
    assert_eq!(got[1].last_opened, None);
    assert_eq!(got[1].dir, PathBuf::from(r"D:\Koru\Personal"));
}

/// Every Windows editor writes a byte-order mark. A workspace was once opened
/// in a directory named after its own comment header because of one, so the
/// same guard is asserted here rather than assumed.
#[test]
fn a_byte_order_mark_does_not_become_a_profile() {
    let text = format!("\u{feff}{}", body(&[rec(1, "Work", r"C:\Koru\Work")], None));
    let got = parse(&text);
    assert_eq!(got.len(), 1, "the mark did not hide the record");
    assert_eq!(got[0].name, "Work");
}

/// One unreadable line must not cost somebody the profiles either side of it.
#[test]
fn a_line_without_a_folder_drops_without_taking_its_neighbours() {
    let text = "\
6f0e5b2c-1d4a-4b2e-9c11-8a1f2e3d4c01|Work|||C:\\Koru\\Work
not-a-uuid|Broken|||C:\\Koru\\Broken
6f0e5b2c-1d4a-4b2e-9c11-8a1f2e3d4c02|NoFolder|||
6f0e5b2c-1d4a-4b2e-9c11-8a1f2e3d4c03|Personal|||D:\\Koru\\Personal
";
    let names: Vec<String> = parse(text).into_iter().map(|r| r.name).collect();
    assert_eq!(names, vec!["Work", "Personal"]);
}

/// A `|` in a display name would shift every field after it, so the row would
/// name a folder the person never chose. The writer reserves the separator; the
/// folder, being last and being the field that must never be altered, does not
/// need it reserved.
#[test]
fn a_stray_separator_in_a_name_cannot_shift_the_folder() {
    let mut r = ProfileRecord::new(
        id(1),
        "Work | School",
        Some("a|b@example.com"),
        // A POSIX path may legitimately contain one.
        Path::new("/home/sam/Koru|Work"),
    );
    r.last_opened = Some("20260801-120000".into());

    let got = parse(&body(std::slice::from_ref(&r), None));

    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "Work   School", "the separator is reserved");
    assert_eq!(got[0].email.as_deref(), Some("a b@example.com"));
    assert_eq!(
        got[0].dir,
        PathBuf::from("/home/sam/Koru|Work"),
        "the folder is untouched — an altered path is a profile that will not open"
    );
    assert_eq!(got[0].id, r.id);
}

/// An unplugged drive, an unsynced cloud folder and a deleted one are
/// indistinguishable from here. A registry that pruned itself on a bad day is
/// how a profile disappears from the switcher forever.
#[test]
fn a_missing_folder_keeps_its_entry() {
    let missing = ProfileRecord::new(
        id(1),
        "Work",
        None,
        Path::new(r"Z:\definitely\not\here\at\all"),
    );
    let text = body(std::slice::from_ref(&missing), None);
    assert!(!missing.dir.exists(), "the premise of the test");
    assert_eq!(parse(&text).len(), 1, "and it is still listed");
}

/// A profile folder copied to a second path is a second *replica* of one
/// profile, not a second profile. The switcher must show one row.
#[test]
fn two_folders_with_one_profile_id_are_one_entry_in_the_switcher() {
    let mut records = vec![ProfileRecord::new(
        id(1),
        "Work",
        None,
        Path::new(r"C:\Koru\Work"),
    )];
    let mut moved = ProfileRecord::new(id(1), "Work", None, Path::new(r"D:\Koru\Work"));
    moved.last_opened = Some("20260801-120000".into());

    upsert(&mut records, moved.clone());

    assert_eq!(records.len(), 1, "one profile, however many folders");
    assert_eq!(records[0].dir, moved.dir, "the one opened most recently");
}

#[test]
fn upsert_adds_a_genuinely_new_profile() {
    let mut records = vec![rec(1, "Work", r"C:\Koru\Work")];
    upsert(&mut records, rec(2, "Personal", r"D:\Koru\Personal"));
    assert_eq!(records.len(), 2);
}

#[test]
fn a_profile_is_found_by_name_by_case_and_by_prefix() {
    let records = vec![
        rec(1, "Work", r"C:\Koru\Work"),
        rec(2, "Personal", r"D:\Koru\Personal"),
    ];
    assert_eq!(find_by_name(&records, "Work").unwrap().id, id(1));
    assert_eq!(find_by_name(&records, "work").unwrap().id, id(1));
    assert_eq!(find_by_name(&records, "  pers ").unwrap().id, id(2));
    assert_eq!(find_by_id(&records, id(2)).unwrap().name, "Personal");
}

/// Choosing between two profiles the user might have meant is the same failure
/// as guessing a directory. Answer nothing and let the caller refuse.
#[test]
fn an_ambiguous_name_matches_nothing() {
    let records = vec![
        rec(1, "Work", r"C:\Koru\Work"),
        rec(2, "Workshop", r"D:\Koru\Workshop"),
    ];
    assert_eq!(find_by_name(&records, "Wor"), None, "a shared prefix");
    assert_eq!(find_by_name(&records, ""), None);
    assert_eq!(find_by_name(&records, "nothing like it"), None);
    // An exact name still wins over a longer one that shares its prefix.
    assert_eq!(find_by_name(&records, "Work").unwrap().id, id(1));
}

#[test]
fn the_most_recently_opened_profile_is_the_one_a_plain_launch_reopens() {
    let mut records = vec![
        rec(1, "Work", r"C:\Koru\Work"),
        rec(2, "Personal", r"D:\Koru\Personal"),
    ];
    // rec(2)'s stamp is later, by construction.
    assert_eq!(most_recent(&records).unwrap().id, id(2));
    assert_eq!(by_recency(&records)[0].id, id(2));

    // A profile that has never been opened does not outrank one that has.
    records.push(ProfileRecord::new(
        id(3),
        "School",
        None,
        Path::new(r"E:\Koru\School"),
    ));
    assert_eq!(most_recent(&records).unwrap().id, id(2));
    assert_eq!(by_recency(&records).last().unwrap().id, id(3));
}

#[test]
fn an_empty_registry_is_not_an_error() {
    assert!(parse("").is_empty());
    assert!(parse("# nothing but a comment\n").is_empty());
    assert_eq!(most_recent(&[]), None);
}

/// The marker round-trips through `body`, and an older `parse` ignores it —
/// so a new file does not confuse a build from before the marker existed.
#[test]
fn the_selected_marker_round_trips_and_is_forward_compatible() {
    let records = vec![rec(1, "Work", r"C:\Koru\Work")];
    let text = body(&records, Some(id(1)));
    assert_eq!(selected_id(&text), Some(id(1)));
    // The directive line carries no `|`, so `parse` drops it: the records come
    // back unchanged, and an older build would do the same.
    assert_eq!(parse(&text), records);
}

/// `None` when no marker is present — a fresh list, or one an older build wrote.
#[test]
fn a_silent_registry_has_no_selection() {
    let records = vec![rec(1, "Work", r"C:\Koru\Work")];
    assert_eq!(selected_id(&body(&records, None)), None);
    assert_eq!(selected_id("# only comments\n"), None);
    assert_eq!(selected_id(""), None);
}

/// A malformed marker is ignored rather than guessed at.
#[test]
fn a_marker_with_a_bad_id_is_ignored() {
    let text =
        "last-selected not-a-uuid\n6f0e5b2c-1d4a-4b2e-9c11-8a1f2e3d4c01|Work|||C:\\Koru\\Work\n";
    assert_eq!(selected_id(text), None);
    assert_eq!(parse(text).len(), 1, "the row still parses");
}
