// SPDX-License-Identifier: MPL-2.0

//! These three moved here from `koru-shell-chrome`'s `branch` module with the
//! policy they cover: which branches are worth naming is this app's judgement,
//! so this is where it is pinned.

use super::*;

/// The gate is the whole feature: a pill on `dev` would be permanent furniture.
#[test]
fn only_a_branch_that_is_not_the_usual_one_is_named() {
    assert_eq!(
        worth_naming(Some("import-google-docs")),
        Some("import-google-docs")
    );
    assert_eq!(worth_naming(Some("bug-fixes")), Some("bug-fixes"));
    assert_eq!(worth_naming(Some("dev")), None);
    assert_eq!(worth_naming(Some("main")), None);
}

/// A released build, a source tarball with no git, and a detached HEAD all
/// arrive as nothing to say — and an unstamped build must not draw an empty
/// pill, which is the arm a release is the worst place to find.
#[test]
fn a_build_with_no_branch_says_nothing() {
    assert_eq!(worth_naming(None), None);
    assert_eq!(worth_naming(Some("")), None);
    assert_eq!(worth_naming(Some("   ")), None);
}

/// `dev ` and `dev` are the same branch; the stamp is trimmed here so no caller
/// has to remember to — including one that does not come through
/// [`branch`](super::branch), which trims already.
#[test]
fn the_name_is_trimmed_before_it_is_judged() {
    assert_eq!(worth_naming(Some("  dev  ")), None);
    assert_eq!(worth_naming(Some("  kimi  ")), Some("kimi"));
}
