// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::errors::Code;

fn caller(scopes: &[Scope]) -> Caller {
    Caller {
        scopes: scopes.to_vec(),
        label: "test".into(),
    }
}

#[test]
fn a_caller_holds_only_what_it_was_given() {
    let c = caller(&[Scope::ReadDocuments]);
    assert!(c.allows(Scope::ReadDocuments));
    assert!(!c.allows(Scope::EditDocuments));
    assert!(!c.allows(Scope::ManageWorkspaces));
}

#[test]
fn requiring_a_missing_scope_is_forbidden_and_names_it() {
    // The scope is named because a developer integrating against the API has
    // to know which one to ask for. It reveals nothing — the caller already
    // knows what they attempted.
    let e = caller(&[Scope::ReadDocuments])
        .require(Scope::ManageWorkspaces)
        .unwrap_err();
    assert_eq!(e.0.code(), Code::Forbidden);
    assert!(
        e.0.message().contains("manage_workspaces"),
        "{}",
        e.0.message()
    );
}

#[test]
fn requiring_a_held_scope_passes() {
    assert!(caller(Scope::ALL).require(Scope::EditDocuments).is_ok());
}

#[test]
fn a_caller_with_no_scopes_can_do_nothing() {
    let c = caller(&[]);
    for s in Scope::ALL {
        assert!(c.require(*s).is_err(), "{s} was allowed");
    }
}

// ------------------------------------------------------------------ comparison

#[test]
fn identical_tokens_compare_equal() {
    let token = "f".repeat(64);
    assert!(constant_time_eq(&token, &token));
    assert!(constant_time_eq("", ""));
}

#[test]
fn different_tokens_do_not() {
    assert!(!constant_time_eq("abc", "abd"));
    assert!(!constant_time_eq("abc", "ABC"), "comparison must be exact");
}

#[test]
fn a_length_difference_is_not_equal() {
    assert!(!constant_time_eq("abc", "abcd"));
    assert!(!constant_time_eq("abcd", "abc"));
    assert!(!constant_time_eq("", "a"));
}

#[test]
fn a_wrong_first_byte_and_a_wrong_last_byte_are_both_simply_false() {
    // A short-circuiting compare tells an attacker how many leading bytes of a
    // guess were right, turning a search of the whole space into a search of
    // one byte at a time. This cannot observe timing, but it pins the
    // behaviour that makes the timing uniform.
    let real = "0123456789abcdef".repeat(4);
    let mut first = real.clone();
    first.replace_range(0..1, "z");
    let mut last = real.clone();
    let end = real.len();
    last.replace_range(end - 1..end, "z");
    assert!(!constant_time_eq(&real, &first));
    assert!(!constant_time_eq(&real, &last));
}

#[test]
fn comparison_is_over_bytes_and_handles_non_ascii_without_panicking() {
    // Tokens are hex today, but this must not become a slicing panic if one
    // ever is not.
    assert!(constant_time_eq("مفتاح", "مفتاح"));
    assert!(!constant_time_eq("مفتاح", "مفتاج"));
    assert!(!constant_time_eq("鍵", "key"));
}
