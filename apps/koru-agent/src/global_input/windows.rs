// SPDX-License-Identifier: MPL-2.0

//! Agent hotkey adapters and the policy-facing native insertion adapter.
//! Consent, restoration decisions and refusal presentation remain in Agent.

use std::marker::PhantomData;
use std::rc::Rc;

use koru_windows_input::{self as native, OrdinaryDesktop};
pub use native::{WindowsRestoreHint, WindowsTarget};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};

use super::{
    DirectInsertion, InsertBackend, KeyState, KeyStateSource, PressError, PressRegistrar, Refusal,
    TargetDisplay, TargetState, VirtualKey,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowsChord {
    pub modifiers: HOT_KEY_MODIFIERS,
    pub key: VirtualKey,
}

#[derive(Default)]
pub struct WindowsPressRegistrar;

/// A RegisterHotKey token is intentionally not Send: registration and release
/// remain on the same message-pump thread.
pub struct WindowsPressRegistration {
    action: i32,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Drop for WindowsPressRegistration {
    fn drop(&mut self) {
        let _ = unsafe { UnregisterHotKey(None, self.action) };
    }
}

impl PressRegistrar for WindowsPressRegistrar {
    type Chord = WindowsChord;
    type Registration = WindowsPressRegistration;

    fn register(
        &mut self,
        action: i32,
        chord: Self::Chord,
    ) -> Result<Self::Registration, PressError> {
        unsafe {
            RegisterHotKey(
                None,
                action,
                chord.modifiers | MOD_NOREPEAT,
                chord.key.raw() as u32,
            )
        }
        .map_err(|_| PressError::Unavailable)?;
        Ok(WindowsPressRegistration {
            action,
            _thread_bound: PhantomData,
        })
    }
}

/// Captures Agent's ordinary desktop name once. Every active sample proves the
/// current input desktop still has that name before interpreting a zero key bit.
pub struct WindowsKeyState {
    ordinary_desktop: OrdinaryDesktop,
}

impl WindowsKeyState {
    pub fn new() -> Result<Self, ()> {
        Ok(Self {
            ordinary_desktop: OrdinaryDesktop::capture()?,
        })
    }

    fn on_ordinary_desktop(&self) -> bool {
        self.ordinary_desktop.is_current()
    }
}

impl KeyStateSource for WindowsKeyState {
    fn state(&mut self, key: VirtualKey) -> KeyState {
        if !self.on_ordinary_desktop() {
            return KeyState::Unavailable;
        }
        let down = (unsafe { GetAsyncKeyState(key.raw() as i32) } as u16 & 0x8000) != 0;
        if down { KeyState::Down } else { KeyState::Up }
    }
}

/// Thin adapter: product refusal vocabulary and policy remain in Agent.
pub struct WindowsInsertBackend(native::WindowsInsertBackend);

impl WindowsInsertBackend {
    pub fn new() -> Result<Self, ()> {
        native::WindowsInsertBackend::new().map(Self)
    }
}

impl InsertBackend for WindowsInsertBackend {
    type Target = WindowsTarget;
    type RestoreHint = WindowsRestoreHint;
    fn capture_restore_hint(&mut self) -> Result<Self::RestoreHint, Refusal> {
        self.0.capture_restore_hint().map_err(Into::into)
    }

    fn restore_hint(&mut self, hint: &Self::RestoreHint) -> Result<(), Refusal> {
        self.0.restore_hint(hint).map_err(Into::into)
    }

    fn capture_target(&mut self) -> Result<Self::Target, Refusal> {
        self.0.capture_target().map_err(Into::into)
    }

    fn capture_target_for_hint(
        &mut self,
        hint: &Self::RestoreHint,
    ) -> Result<Self::Target, Refusal> {
        self.0.capture_target_for_hint(hint).map_err(Into::into)
    }

    fn target_state(&mut self, target: &Self::Target) -> TargetState {
        self.0.target_state(target).into()
    }

    fn restore_target(&mut self, target: &Self::Target) -> TargetState {
        self.0.restore_target(target).into()
    }

    fn current_display(&mut self) -> Option<TargetDisplay> {
        self.0.current_display().map(Into::into)
    }

    fn target_display(&mut self, target: &Self::Target) -> Option<TargetDisplay> {
        self.0.target_display(target).map(Into::into)
    }

    fn insert_unicode(&mut self, target: &Self::Target, text: &str) -> DirectInsertion {
        self.0.insert_unicode(target, text).into()
    }
}

impl From<native::Refusal> for Refusal {
    fn from(value: native::Refusal) -> Self {
        match value {
            native::Refusal::NoTarget => Self::NoTarget,
            native::Refusal::ProtectedTarget => Self::ProtectedTarget,
            native::Refusal::TargetChanged => Self::TargetChanged,
            native::Refusal::TargetDestroyed => Self::TargetDestroyed,
            native::Refusal::TargetUnavailable => Self::TargetUnavailable,
        }
    }
}

impl From<native::TargetState> for TargetState {
    fn from(value: native::TargetState) -> Self {
        match value {
            native::TargetState::Editable => Self::Editable,
            native::TargetState::Protected => Self::Protected,
            native::TargetState::Changed => Self::Changed,
            native::TargetState::Destroyed => Self::Destroyed,
            native::TargetState::Unavailable => Self::Unavailable,
        }
    }
}

impl From<native::DirectInsertion> for DirectInsertion {
    fn from(value: native::DirectInsertion) -> Self {
        match value {
            native::DirectInsertion::Complete => Self::Complete,
            native::DirectInsertion::Rejected => Self::Rejected,
            native::DirectInsertion::Partial => Self::Partial,
            native::DirectInsertion::TargetUnavailable => Self::TargetUnavailable,
        }
    }
}

impl From<native::TargetDisplay> for TargetDisplay {
    fn from(value: native::TargetDisplay) -> Self {
        Self {
            work_area: value.work_area,
            dpi: value.dpi,
        }
    }
}

#[cfg(test)]
mod tests;
