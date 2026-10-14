// SPDX-License-Identifier: MPL-2.0

use super::*;
#[cfg(not(target_os = "macos"))]
use winit::keyboard::{Key, NamedKey};

#[test]
#[cfg(not(target_os = "macos"))]
fn queued_ctrl_a_keeps_its_chord_after_the_physical_batch_has_finished() {
    let mut down = HashSet::new();
    // Winit emits its queue-state report before the key-down callbacks. Every
    // callback here may be replayed after SendInput has released the whole batch.
    let physical_now = ModifiersState::empty();
    let mut mods = accept_report(ModifiersState::CONTROL, &mut down);
    mods = press(mods, &mut down, PhysicalKey::Code(KeyCode::ControlLeft));
    mods = press(mods, &mut down, PhysicalKey::Code(KeyCode::KeyA));
    let select_all = koru_keys::registry::get(koru_keys::ShortcutId::SelectAll).chord;
    let a = Key::Character("a".into());
    assert!(select_all.matches(&a, Some("a"), mods));
    // This was the old host composition: it turns the same queued chord into
    // plain A. Physical recovery is still valid for pointers, never for this key.
    assert!(!select_all.matches(
        &a,
        Some("a"),
        reconcile_physical_modifiers(mods, physical_now)
    ));
    down.remove(&PhysicalKey::Code(KeyCode::KeyA));
    mods = modifiers_after_key_release(mods, PhysicalKey::Code(KeyCode::KeyA), &down);
    assert!(mods.control_key());
    mods = accept_report(ModifiersState::empty(), &mut down);
    down.remove(&PhysicalKey::Code(KeyCode::ControlLeft));
    mods = modifiers_after_key_release(mods, PhysicalKey::Code(KeyCode::ControlLeft), &down);
    let following = press(mods, &mut down, PhysicalKey::Code(KeyCode::Enter));
    assert!(
        following.is_empty(),
        "the later Enter must not become Ctrl+Enter"
    );
    assert!(!select_all.matches(&Key::Named(NamedKey::Enter), None, following));
}

#[test]
fn queued_shift_selection_survives_until_its_release_and_not_after() {
    let mut down = HashSet::new();
    let mut mods = accept_report(ModifiersState::SHIFT, &mut down);
    mods = press(mods, &mut down, PhysicalKey::Code(KeyCode::ShiftLeft));
    mods = press(mods, &mut down, PhysicalKey::Code(KeyCode::ArrowLeft));
    assert!(mods.shift_key());
    down.remove(&PhysicalKey::Code(KeyCode::ArrowLeft));
    down.remove(&PhysicalKey::Code(KeyCode::ShiftLeft));
    mods = modifiers_after_key_release(mods, PhysicalKey::Code(KeyCode::ShiftLeft), &down);
    assert!(press(mods, &mut down, PhysicalKey::Code(KeyCode::ArrowLeft)).is_empty());
}

#[test]
fn an_empty_report_recovers_consumed_hotkey_releases_before_the_next_enter() {
    let mut down = HashSet::new();
    let mods = press(
        ModifiersState::empty(),
        &mut down,
        PhysicalKey::Code(KeyCode::ControlLeft),
    );
    let mods = press(mods, &mut down, PhysicalKey::Code(KeyCode::ShiftLeft));
    let mods = press(mods, &mut down, PhysicalKey::Code(KeyCode::KeyK));
    assert!(mods.control_key() && mods.shift_key());
    // The hotkey owner consumed Ctrl/Shift-up. Winit recomputes queue state
    // before the next key message, and this authoritative report purges them.
    let mods = accept_report(ModifiersState::empty(), &mut down);
    assert!(!down.contains(&PhysicalKey::Code(KeyCode::ControlLeft)));
    assert!(!down.contains(&PhysicalKey::Code(KeyCode::ShiftLeft)));
    assert!(
        down.contains(&PhysicalKey::Code(KeyCode::KeyK)),
        "only modifier recovery is affected"
    );
    assert!(press(mods, &mut down, PhysicalKey::Code(KeyCode::Enter)).is_empty());
}

#[test]
fn pointer_recovery_still_repairs_a_missed_release_without_resurrecting_it() {
    let mut down = HashSet::from([PhysicalKey::Code(KeyCode::ControlLeft)]);
    let repaired = reconcile_physical_modifiers(ModifiersState::CONTROL, ModifiersState::empty());
    forget_released(repaired, &mut down);
    assert!(press(repaired, &mut down, PhysicalKey::Code(KeyCode::Enter)).is_empty());
    // Conversely, Ctrl-click after a focus transition recovers a still-held Ctrl.
    assert!(
        reconcile_physical_modifiers(ModifiersState::empty(), ModifiersState::CONTROL)
            .control_key()
    );
}
