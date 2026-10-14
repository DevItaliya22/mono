// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::inspect::{Handle, Response, Role};

fn facts() -> ElementFacts {
    ElementFacts {
        handle: Handle {
            generation: 1,
            slot: 2,
        },
        locator: None,
        role: Role::Edit,
        enabled: true,
        offscreen: true,
        bounds: None,
        process_id: Some(1),
    }
}

#[test]
fn snapshot_cannot_split_a_surrogate_pair_or_leak_through_debug() {
    let snapshot = ContentSnapshot::bounded(facts(), "a😀private".into(), 2).unwrap();
    assert_eq!(snapshot.text, "a");
    assert!(snapshot.truncated);
    let full = ContentSnapshot::bounded(facts(), "secret sentinel".into(), 100).unwrap();
    assert!(!format!("{:?}", Response::Snapshot(full)).contains("secret sentinel"));
}

#[test]
fn complete_unicode_is_exact_and_invalid_limits_are_refused() {
    let text = "ü漢😀e\u{301}";
    let snapshot =
        ContentSnapshot::bounded(facts(), text.into(), text.encode_utf16().count() as u32).unwrap();
    assert_eq!(snapshot.text, text);
    assert!(!snapshot.truncated);
    assert!(ContentSnapshot::bounded(facts(), String::new(), 0).is_err());
    assert!(ContentSnapshot::bounded(facts(), String::new(), 16385).is_err());
}
