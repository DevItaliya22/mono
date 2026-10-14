// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The whole point of the crate, asserted in both directions so that a
/// definition which was `CONTROL` everywhere could not pass it.
#[test]
fn primary_is_command_on_mac_and_control_elsewhere() {
    if cfg!(target_os = "macos") {
        assert_eq!(PRIMARY, ModifiersState::SUPER);
        assert!(has_primary(ModifiersState::SUPER));
        assert!(!has_primary(ModifiersState::CONTROL));
    } else {
        assert_eq!(PRIMARY, ModifiersState::CONTROL);
        assert!(has_primary(ModifiersState::CONTROL));
        assert!(!has_primary(ModifiersState::SUPER));
    }
}

/// The two are exactly opposite — there is no platform where a modifier is both,
/// and none where a modifier is neither.
#[test]
fn the_foreign_primary_is_the_other_one() {
    for mods in [ModifiersState::CONTROL, ModifiersState::SUPER] {
        assert_ne!(
            has_primary(mods),
            has_foreign_primary(mods),
            "{mods:?} must be exactly one of primary and foreign"
        );
    }
}

/// **A chord must not fire under the other platform's primary.** ⌘+B means
/// nothing on Windows and Ctrl+B means nothing on macOS; a resolver that tests
/// only its own modifier fires on both.
#[test]
fn the_foreign_primary_alone_is_not_the_primary() {
    let foreign = if cfg!(target_os = "macos") {
        ModifiersState::CONTROL
    } else {
        ModifiersState::SUPER
    };
    assert!(!primary_only(foreign));
    assert!(has_foreign_primary(foreign));
}

/// `primary_only` is the four-condition guard the hand-written copies kept
/// getting wrong — most often by forgetting Alt, which is how `Ctrl+Alt+N`
/// opened a window *and* set a document Narrow.
#[test]
fn primary_only_excludes_shift_alt_and_the_foreign_primary() {
    assert!(primary_only(PRIMARY));
    assert!(!primary_only(NONE));
    assert!(!primary_only(PRIMARY_SHIFT));
    assert!(!primary_only(PRIMARY_ALT));

    let both = ModifiersState::from_bits_truncate(
        PRIMARY.bits() | ModifiersState::CONTROL.bits() | ModifiersState::SUPER.bits(),
    );
    assert!(!primary_only(both), "the foreign primary must veto it");
}

#[test]
fn the_compound_constants_are_their_parts() {
    assert!(has_primary(PRIMARY_SHIFT) && PRIMARY_SHIFT.shift_key());
    assert!(!PRIMARY_SHIFT.alt_key());
    assert!(has_primary(PRIMARY_ALT) && PRIMARY_ALT.alt_key());
    assert!(!PRIMARY_ALT.shift_key());
}

#[test]
fn without_primary_leaves_the_rest_alone() {
    assert_eq!(without_primary(PRIMARY), NONE);
    assert_eq!(without_primary(PRIMARY_SHIFT), SHIFT);
    assert_eq!(without_primary(NONE), NONE);
}
