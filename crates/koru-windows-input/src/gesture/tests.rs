// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn every_partial_chord_releases_only_accepted_downs() {
    let events = chord(&[0x11, 0x10, 0x5a]);
    assert!(pending_releases(&events, 0).is_empty());
    assert_eq!(
        pending_releases(&events, 2),
        vec![
            Event {
                key: Key::Virtual(0x10),
                up: true
            },
            Event {
                key: Key::Virtual(0x11),
                up: true
            }
        ]
    );
    for accepted in 0..=events.len() {
        let mut completed = events[..accepted].to_vec();
        completed.extend(pending_releases(&events, accepted));
        assert!(pending_releases(&completed, completed.len()).is_empty());
    }
}

#[test]
fn unicode_preserves_surrogate_pairs_combining_marks_and_bidi() {
    let phrase = "Aé\u{301}漢😀\u{200f}";
    let events = text(phrase);
    let units: Vec<_> = events
        .iter()
        .filter_map(|e| match (e.key, e.up) {
            (Key::Unicode(u), false) => Some(u),
            _ => None,
        })
        .collect();
    assert_eq!(String::from_utf16(&units).unwrap(), phrase);
    for accepted in 0..=events.len() {
        let cleanup = pending_releases(&events, accepted);
        assert_eq!(cleanup.len(), accepted % 2);
    }
}
