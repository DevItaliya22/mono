// SPDX-License-Identifier: MPL-2.0

//! Exact foreign-process UIA targets and input consequences.
//!
//! No window lifecycle, dictation policy, consent, scenario traversal or waits.
//! The native proof beside this module moves with the mechanism.

use std::marker::PhantomData;
use std::rc::Rc;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetThreadDesktop,
    GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, UIA_E_ELEMENTNOTAVAILABLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GUITHREADINFO, GetAncestor, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
};

use crate::automation::{AutomationClient, ComApartment};
use crate::{DirectInsertion, Refusal, TargetDisplay, TargetState};

fn desktop_name(desktop: HANDLE) -> Result<Vec<u16>, ()> {
    let mut bytes = 0_u32;
    let _ = unsafe { GetUserObjectInformationW(desktop, UOI_NAME, None, 0, Some(&mut bytes)) };
    if bytes < 2 {
        return Err(());
    }
    let mut name = vec![0_u16; (bytes as usize).div_ceil(2)];
    unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            bytes,
            Some(&mut bytes),
        )
    }
    .map_err(|_| ())?;
    if let Some(end) = name.iter().position(|unit| *unit == 0) {
        name.truncate(end);
    }
    (!name.is_empty()).then_some(name).ok_or(())
}

#[derive(Clone)]
pub struct OrdinaryDesktop(Vec<u16>);

impl OrdinaryDesktop {
    pub fn capture() -> Result<Self, ()> {
        let desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()) }.map_err(|_| ())?;
        desktop_name(HANDLE(desktop.0)).map(Self)
    }

    pub fn is_current(&self) -> bool {
        let Ok(input) =
            (unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) })
        else {
            return false;
        };
        let name = desktop_name(HANDLE(input.0));
        let _ = unsafe { CloseDesktop(input) };
        name.is_ok_and(|name| name == self.0)
    }
}

#[derive(Clone)]
pub struct WindowsTarget {
    element: IUIAutomationElement,
    focus_window: HWND,
    focus_pid: u32,
    restore_window: HWND,
    restore_pid: u32,
    _apartment: Rc<ComApartment>,
}

#[derive(Clone, Copy)]
pub struct WindowsRestoreHint {
    root: HWND,
    pid: u32,
}

impl std::fmt::Debug for WindowsRestoreHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WindowsRestoreHint([REDACTED])")
    }
}

impl std::fmt::Debug for WindowsTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WindowsTarget([REDACTED])")
    }
}

/// Construct on the worker that will use it. COM and UI Automation remain on
/// that thread, and the marker prevents moving the adapter to Agent's loop.
pub struct WindowsInsertBackend {
    automation: IUIAutomation,
    ordinary_desktop: OrdinaryDesktop,
    apartment: Rc<ComApartment>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl WindowsInsertBackend {
    pub fn new() -> Result<Self, ()> {
        let ordinary_desktop = OrdinaryDesktop::capture()?;
        let client = AutomationClient::open().map_err(|_| ())?;
        Ok(Self {
            automation: client.automation,
            ordinary_desktop,
            apartment: client.apartment,
            _thread_bound: PhantomData,
        })
    }

    fn focused_window() -> Result<HWND, Refusal> {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetGUIThreadInfo(0, &mut info) }.map_err(|_| Refusal::NoTarget)?;
        if info.hwndFocus.0.is_null() {
            Err(Refusal::NoTarget)
        } else {
            Ok(info.hwndFocus)
        }
    }

    fn window_pid(window: HWND) -> Option<u32> {
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        (thread != 0 && pid != 0).then_some(pid)
    }

    fn display_for_root(root: HWND) -> Option<TargetDisplay> {
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        };
        use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;

        if root.0.is_null() {
            return None;
        }
        let monitor = unsafe { MonitorFromWindow(root.0, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return None;
        }
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return None;
        }
        let dpi = unsafe { GetDpiForWindow(root.0) };
        (dpi != 0).then_some(TargetDisplay {
            work_area: (
                info.rcWork.left,
                info.rcWork.top,
                info.rcWork.right,
                info.rcWork.bottom,
            ),
            dpi,
        })
    }

    fn target_identity_state(target: &WindowsTarget) -> TargetState {
        classify_target_identity(
            unsafe { IsWindow(Some(target.focus_window)) }.as_bool(),
            Self::window_pid(target.focus_window),
            target.focus_pid,
            unsafe { IsWindow(Some(target.restore_window)) }.as_bool(),
            Self::window_pid(target.restore_window),
            target.restore_pid,
            unsafe { GetAncestor(target.focus_window, GA_ROOT) } == target.restore_window,
        )
    }

    fn element_state(&self, target: &WindowsTarget) -> TargetState {
        if !self.ordinary_desktop.is_current() {
            return TargetState::Unavailable;
        }
        let identity = Self::target_identity_state(target);
        if identity != TargetState::Editable {
            return identity;
        }
        match Self::focused_window() {
            Ok(window) if window == target.focus_window => {}
            Ok(_) => return TargetState::Changed,
            Err(_) => return TargetState::Unavailable,
        }
        match unsafe { target.element.CurrentIsPassword() } {
            Ok(password) if password.as_bool() => return TargetState::Protected,
            Ok(_) => {}
            Err(error) if error.code().0 as u32 == UIA_E_ELEMENTNOTAVAILABLE => {
                return TargetState::Destroyed;
            }
            Err(_) => return TargetState::Unavailable,
        }
        match unsafe { target.element.CurrentIsEnabled() } {
            Ok(enabled) if enabled.as_bool() => {}
            Ok(_) | Err(_) => return TargetState::Unavailable,
        }
        let current = match unsafe { self.automation.GetFocusedElement() } {
            Ok(current) => current,
            Err(_) => return TargetState::Unavailable,
        };
        match unsafe { self.automation.CompareElements(&target.element, &current) } {
            Ok(equal) if equal.as_bool() => TargetState::Editable,
            Ok(_) => TargetState::Changed,
            Err(error) if error.code().0 as u32 == UIA_E_ELEMENTNOTAVAILABLE => {
                TargetState::Destroyed
            }
            Err(_) => TargetState::Unavailable,
        }
    }
}

fn classify_target_identity(
    focus_exists: bool,
    focus_pid: Option<u32>,
    expected_focus_pid: u32,
    root_exists: bool,
    root_pid: Option<u32>,
    expected_root_pid: u32,
    still_same_root: bool,
) -> TargetState {
    if !focus_exists || !root_exists {
        TargetState::Destroyed
    } else if focus_pid != Some(expected_focus_pid)
        || root_pid != Some(expected_root_pid)
        || !still_same_root
    {
        TargetState::Changed
    } else {
        TargetState::Editable
    }
}

pub(crate) fn classify_restore_hint(
    exists: bool,
    pid: Option<u32>,
    expected_pid: u32,
) -> Result<(), Refusal> {
    if !exists {
        Err(Refusal::TargetDestroyed)
    } else if pid != Some(expected_pid) {
        Err(Refusal::TargetChanged)
    } else {
        Ok(())
    }
}

impl WindowsInsertBackend {
    pub fn capture_restore_hint(&mut self) -> Result<WindowsRestoreHint, Refusal> {
        if !self.ordinary_desktop.is_current() {
            return Err(Refusal::TargetUnavailable);
        }
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0.is_null() {
            return Err(Refusal::NoTarget);
        }
        let root = unsafe { GetAncestor(foreground, GA_ROOT) };
        if root.0.is_null() {
            return Err(Refusal::TargetUnavailable);
        }
        let pid = Self::window_pid(root).ok_or(Refusal::TargetUnavailable)?;
        Ok(WindowsRestoreHint { root, pid })
    }

    pub fn restore_hint(&mut self, hint: &WindowsRestoreHint) -> Result<(), Refusal> {
        if !self.ordinary_desktop.is_current() {
            return Err(Refusal::TargetUnavailable);
        }
        classify_restore_hint(
            unsafe { IsWindow(Some(hint.root)) }.as_bool(),
            Self::window_pid(hint.root),
            hint.pid,
        )?;
        let _ = unsafe { SetForegroundWindow(hint.root) };
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0.is_null() || unsafe { GetAncestor(foreground, GA_ROOT) } != hint.root {
            return Err(Refusal::TargetChanged);
        }
        classify_restore_hint(
            unsafe { IsWindow(Some(hint.root)) }.as_bool(),
            Self::window_pid(hint.root),
            hint.pid,
        )
    }

    pub fn capture_target(&mut self) -> Result<WindowsTarget, Refusal> {
        if !self.ordinary_desktop.is_current() {
            return Err(Refusal::TargetUnavailable);
        }
        let focus_window = Self::focused_window()?;
        let focus_pid = Self::window_pid(focus_window).ok_or(Refusal::TargetUnavailable)?;
        let restore_window = unsafe { GetAncestor(focus_window, GA_ROOT) };
        if restore_window.0.is_null() {
            return Err(Refusal::TargetUnavailable);
        }
        let restore_pid = Self::window_pid(restore_window).ok_or(Refusal::TargetUnavailable)?;
        let element = unsafe { self.automation.GetFocusedElement() }
            .map_err(|_| Refusal::TargetUnavailable)?;
        if Self::focused_window()? != focus_window {
            return Err(Refusal::TargetChanged);
        }
        Ok(WindowsTarget {
            element,
            focus_window,
            focus_pid,
            restore_window,
            restore_pid,
            _apartment: Rc::clone(&self.apartment),
        })
    }

    pub fn capture_target_for_hint(
        &mut self,
        hint: &WindowsRestoreHint,
    ) -> Result<WindowsTarget, Refusal> {
        let target = self.capture_target()?;
        classify_restore_hint(
            unsafe { IsWindow(Some(hint.root)) }.as_bool(),
            Self::window_pid(hint.root),
            hint.pid,
        )?;
        if target.restore_window != hint.root || target.restore_pid != hint.pid {
            return Err(Refusal::TargetChanged);
        }
        Ok(target)
    }

    /// Bind the caller-selected element to a named process without taking focus.
    /// Traversal and unique-match selection belong to the caller. This proves
    /// that the selection is the exact focused control before returning it;
    /// insert_unicode still re-proves it immediately before SendInput.
    pub fn capture_named_target(
        &mut self,
        element: &IUIAutomationElement,
        process_id: u32,
    ) -> Result<WindowsTarget, Refusal> {
        if !self.ordinary_desktop.is_current() {
            return Err(Refusal::TargetUnavailable);
        }
        let element_pid = unsafe { element.CurrentProcessId() }.map_err(element_refusal)?;
        let target = self.capture_target()?;
        classify_named_process(element_pid, target.restore_pid, process_id)?;
        let same = unsafe { self.automation.CompareElements(element, &target.element) }
            .map_err(element_refusal)?;
        if !same.as_bool() {
            return Err(Refusal::TargetChanged);
        }
        match self.element_state(&target) {
            TargetState::Editable => Ok(target),
            TargetState::Protected => Err(Refusal::ProtectedTarget),
            TargetState::Changed => Err(Refusal::TargetChanged),
            TargetState::Destroyed => Err(Refusal::TargetDestroyed),
            TargetState::Unavailable => Err(Refusal::TargetUnavailable),
        }
    }

    pub fn target_state(&mut self, target: &WindowsTarget) -> TargetState {
        self.element_state(target)
    }

    pub fn restore_target(&mut self, target: &WindowsTarget) -> TargetState {
        if !self.ordinary_desktop.is_current() {
            return TargetState::Unavailable;
        }
        let identity = Self::target_identity_state(target);
        if identity != TargetState::Editable {
            return identity;
        }
        // The BOOL reports whether Windows honored this attempt, not whether
        // focus is safe now. Voice teardown may already have restored it.
        let _ = unsafe { SetForegroundWindow(target.restore_window) };
        // Hosted controls can keep their own focus HWND. Ask the captured UIA
        // element, then treat only the exact proof below as authority.
        let _ = unsafe { target.element.SetFocus() };
        self.element_state(target)
    }

    pub fn current_display(&mut self) -> Option<TargetDisplay> {
        let foreground = unsafe { GetForegroundWindow() };
        let root = unsafe { GetAncestor(foreground, GA_ROOT) };
        Self::display_for_root(root)
    }

    pub fn target_display(&mut self, target: &WindowsTarget) -> Option<TargetDisplay> {
        (Self::target_identity_state(target) == TargetState::Editable)
            .then(|| Self::display_for_root(target.restore_window))
            .flatten()
    }

    pub fn insert_unicode(&mut self, target: &WindowsTarget, text: &str) -> DirectInsertion {
        // Re-prove the exact control immediately before SendInput. The generic
        // policy verifies at commit too, but focus or desktop can change
        // between that call and this platform consequence.
        if self.element_state(target) != TargetState::Editable {
            return DirectInsertion::TargetUnavailable;
        }
        let inputs = unicode_inputs(text);
        if inputs.is_empty() {
            return DirectInsertion::Complete;
        }
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
        classify_direct_insertion(sent, inputs.len())
    }
}

pub(crate) fn element_refusal(error: windows::core::Error) -> Refusal {
    if error.code().0 as u32 == UIA_E_ELEMENTNOTAVAILABLE {
        Refusal::TargetDestroyed
    } else {
        Refusal::TargetUnavailable
    }
}

fn classify_named_process(
    element_pid: i32,
    root_pid: u32,
    expected_pid: u32,
) -> Result<(), Refusal> {
    if expected_pid == 0
        || u32::try_from(element_pid).ok() != Some(expected_pid)
        || root_pid != expected_pid
    {
        Err(Refusal::TargetChanged)
    } else {
        Ok(())
    }
}

fn unicode_inputs(text: &str) -> Vec<INPUT> {
    crate::gesture::text(text)
        .into_iter()
        .map(|event| {
            let crate::gesture::Key::Unicode(unit) = event.key else {
                unreachable!("text is UTF-16")
            };
            keyboard_input(
                unit,
                if event.up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
            )
        })
        .collect()
}

fn classify_direct_insertion(sent: usize, expected: usize) -> DirectInsertion {
    if sent == expected {
        DirectInsertion::Complete
    } else if sent == 0 {
        DirectInsertion::Rejected
    } else {
        DirectInsertion::Partial
    }
}

fn keyboard_input(
    unit: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: Default::default(),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// The interactive proof for this mechanism.
///
/// A child module rather than a sibling because it needs the private insides of
/// [`WindowsTarget`] — the captured element is what a read-back has to be asked
/// through, and an opaque target is the whole point of the type. It also means
/// the proof travels with the code: the extraction in
/// `ADR-347-windowsinput` moves this file with `windows.rs` and keeps the
/// *same* instrument before and after.
#[cfg(test)]
mod native;

#[cfg(test)]
mod tests;
