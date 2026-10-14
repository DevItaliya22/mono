// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn text_and_expected_content_cannot_reach_debug_output() {
    for output in [
        format!("{:?}", Action::Text("private phrase".into())),
        format!("{:?}", Condition::Text("private phrase".into())),
        format!("{:?}", Condition::Name("private phrase".into())),
    ] {
        assert!(!output.contains("private"));
    }
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn reminder_and_checklist_chords_come_from_the_registry() {
    assert_eq!(
        shortcut(koru_keys::ShortcutId::Reminders),
        Ok(Action::Chord(vec![0x11, 0x10, 0x52]))
    );
    assert_eq!(
        shortcut(koru_keys::ShortcutId::Checklist),
        Ok(Action::Chord(vec![0x11, 0x10, 0x39]))
    );
}
