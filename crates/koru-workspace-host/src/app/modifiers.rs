// SPDX-License-Identifier: MPL-2.0

//! Event-time keyboard modifiers and current-state pointer recovery.
//! Winit 0.30.13 captures queue state before keyboard dispatch and preserves
//! buffered event order. GetAsyncKeyState describes a later instant; it cannot
//! override that stream. See guide/contributing/deps/WINIT.md.
use std::collections::HashSet;
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};

/// Reconcile winit's modifier snapshot with modifier keys that this window has
/// already seen go down.
///
/// `ModifiersChanged` is normally authoritative, but Windows can deliver it
/// late across a focus transition. The physical-key set is updated from the
/// same keyboard stream before this runs, so it closes that one-event gap
/// without manufacturing a modifier that was not actually pressed.
pub(super) fn modifiers_with_pressed_keys(
    mut mods: ModifiersState,
    down_keys: &HashSet<PhysicalKey>,
) -> ModifiersState {
    let held = |left, right| down_keys.contains(&left) || down_keys.contains(&right);
    if held(
        PhysicalKey::Code(KeyCode::ControlLeft),
        PhysicalKey::Code(KeyCode::ControlRight),
    ) {
        mods.insert(ModifiersState::CONTROL);
    }
    if held(
        PhysicalKey::Code(KeyCode::ShiftLeft),
        PhysicalKey::Code(KeyCode::ShiftRight),
    ) {
        mods.insert(ModifiersState::SHIFT);
    }
    if held(
        PhysicalKey::Code(KeyCode::AltLeft),
        PhysicalKey::Code(KeyCode::AltRight),
    ) {
        mods.insert(ModifiersState::ALT);
    }
    if held(
        PhysicalKey::Code(KeyCode::SuperLeft),
        PhysicalKey::Code(KeyCode::SuperRight),
    ) {
        mods.insert(ModifiersState::SUPER);
    }
    mods
}

/// Clear a cached modifier immediately when its physical key is released.
///
/// Windows may delay or omit the matching `ModifiersChanged`, especially
/// around focus changes. The key stream is unambiguous, so retaining the old
/// bit until another modifier event turns one Shift+Arrow into every later
/// Arrow extending the selection. The opposite side keeps the bit alive.
pub(super) fn modifiers_after_key_release(
    mut mods: ModifiersState,
    released: PhysicalKey,
    down_keys: &HashSet<PhysicalKey>,
) -> ModifiersState {
    let neither_held = |left, right| !down_keys.contains(&left) && !down_keys.contains(&right);
    match released {
        PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight)
            if neither_held(
                PhysicalKey::Code(KeyCode::ShiftLeft),
                PhysicalKey::Code(KeyCode::ShiftRight),
            ) =>
        {
            mods.remove(ModifiersState::SHIFT);
        }
        PhysicalKey::Code(KeyCode::ControlLeft | KeyCode::ControlRight)
            if neither_held(
                PhysicalKey::Code(KeyCode::ControlLeft),
                PhysicalKey::Code(KeyCode::ControlRight),
            ) =>
        {
            mods.remove(ModifiersState::CONTROL);
        }
        PhysicalKey::Code(KeyCode::AltLeft | KeyCode::AltRight)
            if neither_held(
                PhysicalKey::Code(KeyCode::AltLeft),
                PhysicalKey::Code(KeyCode::AltRight),
            ) =>
        {
            mods.remove(ModifiersState::ALT);
        }
        PhysicalKey::Code(KeyCode::SuperLeft | KeyCode::SuperRight)
            if neither_held(
                PhysicalKey::Code(KeyCode::SuperLeft),
                PhysicalKey::Code(KeyCode::SuperRight),
            ) =>
        {
            mods.remove(ModifiersState::SUPER);
        }
        _ => {}
    }
    mods
}

/// Repair pointer modifiers from the current physical snapshot.
/// This must not rewrite a queued keyboard event: its releases may be later
/// in the stream even when every key is already physically up.
#[cfg(any(target_os = "windows", test))]
pub(super) fn reconcile_physical_modifiers(
    mut reported: ModifiersState,
    physical: ModifiersState,
) -> ModifiersState {
    for flag in [
        ModifiersState::CONTROL,
        ModifiersState::SHIFT,
        ModifiersState::ALT,
        ModifiersState::SUPER,
    ] {
        reported.set(flag, physical.contains(flag));
    }
    reported
}

/// Current physical modifiers for pointer/focus recovery only.
/// Queued keyboard events use `accept_report` and `press` instead. In a complete
/// Ctrl+A batch the physical Ctrl can already be up while A is being handled.
/// Other platforms retain winit's report.
pub(super) fn current_modifier_state(reported: ModifiersState) -> ModifiersState {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        };

        let down = |key| (unsafe { GetAsyncKeyState(key as i32) } as u16 & 0x8000) != 0;
        let mut physical = ModifiersState::empty();
        physical.set(ModifiersState::CONTROL, down(VK_CONTROL));
        physical.set(ModifiersState::SHIFT, down(VK_SHIFT));
        physical.set(ModifiersState::ALT, down(VK_MENU));
        physical.set(ModifiersState::SUPER, down(VK_LWIN) || down(VK_RWIN));
        return reconcile_physical_modifiers(reported, physical);
    }

    #[cfg(not(target_os = "windows"))]
    reported
}

/// Accept winit's event-time snapshot, including releases consumed by another
/// owner. The next key must not resurrect them from the pressed-key supplement.
pub(super) fn accept_report(
    reported: ModifiersState,
    down_keys: &mut HashSet<PhysicalKey>,
) -> ModifiersState {
    forget_released(reported, down_keys);
    reported
}

/// Advance the same keyboard stream before deriving this press's modifiers.
/// Never consult live physical state here: winit may be replaying buffered keys.
pub(super) fn press(
    reported: ModifiersState,
    down_keys: &mut HashSet<PhysicalKey>,
    key: PhysicalKey,
) -> ModifiersState {
    down_keys.insert(key);
    modifiers_with_pressed_keys(reported, down_keys)
}

pub(super) fn forget_released(mods: ModifiersState, down_keys: &mut HashSet<PhysicalKey>) {
    use winit::keyboard::KeyCode;
    for (held, left, right) in [
        (
            mods.control_key(),
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
        ),
        (mods.shift_key(), KeyCode::ShiftLeft, KeyCode::ShiftRight),
        (mods.alt_key(), KeyCode::AltLeft, KeyCode::AltRight),
        (mods.super_key(), KeyCode::SuperLeft, KeyCode::SuperRight),
    ] {
        if !held {
            down_keys.remove(&PhysicalKey::Code(left));
            down_keys.remove(&PhysicalKey::Code(right));
        }
    }
}

#[cfg(test)]
mod tests;
