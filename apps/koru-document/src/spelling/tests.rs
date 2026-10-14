// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn address_runs_cover_emails_and_paths_but_not_prose() {
    let text = r"mail ada@example.com or C:\Users\Ada now";
    let runs = address_runs(text);
    assert_eq!(runs.len(), 2);
    assert_eq!(&text[runs[0].clone()], "ada@example.com");
    assert_eq!(&text[runs[1].clone()], r"C:\Users\Ada");

    assert!(address_runs("plain prose only").is_empty());
}

#[test]
fn an_address_at_the_end_is_still_excluded() {
    let text = "see docs/features/spell-check";
    let runs = address_runs(text);
    assert_eq!(runs.len(), 1);
    assert_eq!(&text[runs[0].clone()], "docs/features/spell-check");
}
