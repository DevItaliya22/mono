// SPDX-License-Identifier: MPL-2.0

//! Runner consequences. Traversal, locators, cases and product policy stay outside.
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Accessibility::{IUIAutomation, IUIAutomationElement};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId,
    IsIconic, IsWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, WindowFromPoint,
};

use crate::{
    DeliveryPermit, OrdinaryDesktop,
    gesture::{self, Event, Key},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Cancelled,
    TargetChanged,
    TargetDestroyed,
    ProtectedTarget,
    Unavailable,
    Offscreen,
    StaleBounds,
    HeldInput,
    Rejected,
    Partial,
    CleanupFailed,
    InvalidBatch,
}

/// No text is retained in a diagnostic type.
pub enum Action<'a> {
    Focus,
    Click { right: bool },
    Text(&'a str),
    Chord(&'a [u16]),
}

pub struct Context<'a> {
    pub automation: &'a IUIAutomation,
    pub root: HWND,
    pub pid: u32,
    pub desktop: &'a OrdinaryDesktop,
    pub permit: &'a DeliveryPermit,
    pub owner_alive: &'a dyn Fn() -> bool,
}

impl Context<'_> {
    fn native_guard(&self) -> Result<(), Failure> {
        if !self.permit.is_live() {
            return Err(Failure::Cancelled);
        }
        self.native_identity()
    }

    // Only non-provider calls. Safe to repeat while holding the consequence permit.
    fn native_identity(&self) -> Result<(), Failure> {
        if !(self.owner_alive)() {
            return Err(Failure::TargetDestroyed);
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(self.root, Some(&mut pid)) };
        crate::windows::classify_restore_hint(
            unsafe { IsWindow(Some(self.root)) }.as_bool(),
            (pid != 0).then_some(pid),
            self.pid,
        )
        .map_err(refusal)?;
        if unsafe { GetAncestor(GetForegroundWindow(), GA_ROOT) } != self.root {
            return Err(Failure::TargetChanged);
        }
        if unsafe { IsIconic(self.root) }.as_bool() || !self.desktop.is_current() {
            return Err(Failure::Unavailable);
        }
        Ok(())
    }

    fn selected(&self, element: &IUIAutomationElement) -> Result<(), Failure> {
        self.native_guard()?;
        if unsafe { element.CurrentProcessId() }.map_err(provider)? != self.pid as i32 {
            return Err(Failure::TargetChanged);
        }
        if unsafe { element.CurrentIsPassword() }
            .map_err(provider)?
            .as_bool()
        {
            return Err(Failure::ProtectedTarget);
        }
        if !unsafe { element.CurrentIsEnabled() }
            .map_err(provider)?
            .as_bool()
        {
            return Err(Failure::Unavailable);
        }
        let root = unsafe { self.automation.ElementFromHandle(self.root) }.map_err(provider)?;
        if !self.descends_from(element, &root)? {
            return Err(Failure::TargetChanged);
        }
        self.native_guard()
    }

    fn descends_from(
        &self,
        element: &IUIAutomationElement,
        ancestor: &IUIAutomationElement,
    ) -> Result<bool, Failure> {
        let walker = unsafe { self.automation.RawViewWalker() }.map_err(provider)?;
        let mut at = element.clone();
        for _ in 0..128 {
            if unsafe { self.automation.CompareElements(&at, ancestor) }
                .map_err(provider)?
                .as_bool()
            {
                return Ok(true);
            }
            if !self.permit.is_live() {
                return Err(Failure::Cancelled);
            }
            match unsafe { walker.GetParentElement(&at) } {
                Ok(parent) => at = parent,
                Err(_) => return Ok(false),
            }
        }
        Ok(false)
    }

    pub fn execute(
        &self,
        element: &IUIAutomationElement,
        action: Action<'_>,
    ) -> Result<(), Failure> {
        let _dpi = crate::dpi::PhysicalCoordinates::enter().map_err(|_| Failure::Unavailable)?;
        self.selected(element)?;
        if matches!(action, Action::Focus)
            && unsafe { element.CurrentControlType() }.map_err(provider)?.0 != 50004
        {
            return Err(Failure::InvalidBatch);
        }
        let action = if matches!(action, Action::Focus) {
            Action::Click { right: false }
        } else {
            action
        };
        match &action {
            Action::Text(text) if text.encode_utf16().take(4097).count() > 4096 => {
                return Err(Failure::InvalidBatch);
            }
            Action::Chord(keys)
                if keys.is_empty()
                    || keys.len() > 8
                    || keys
                        .iter()
                        .enumerate()
                        .any(|(i, k)| *k == 0 || *k > 255 || keys[..i].contains(k)) =>
            {
                return Err(Failure::InvalidBatch);
            }
            _ => {}
        }
        let mut point = None;
        let events = match action {
            Action::Click { right } => {
                if unsafe { element.CurrentIsOffscreen() }
                    .map_err(provider)?
                    .as_bool()
                {
                    return Err(Failure::Offscreen);
                }
                let bounds = unsafe { element.CurrentBoundingRectangle() }.map_err(provider)?;
                if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
                    return Err(Failure::Offscreen);
                }
                let center = POINT {
                    x: (i64::from(bounds.left) + i64::from(bounds.right)).div_euclid(2) as i32,
                    y: (i64::from(bounds.top) + i64::from(bounds.bottom)).div_euclid(2) as i32,
                };
                let hit = unsafe { self.automation.ElementFromPoint(center) }.map_err(provider)?;
                if !self.descends_from(&hit, element)? {
                    return Err(Failure::TargetChanged);
                }
                self.selected(element)?;
                let now = unsafe { element.CurrentBoundingRectangle() }.map_err(provider)?;
                if now != bounds {
                    return Err(Failure::StaleBounds);
                }
                point = Some(center);
                gesture::click(right)
            }
            Action::Text(text) => {
                self.focused(element)?;
                gesture::text(text)
            }
            Action::Chord(keys) => {
                self.focused(element)?;
                gesture::chord(keys)
            }
            Action::Focus => unreachable!(),
        };
        // No UIA call below this line. Cancellation serializes with the finite
        // batch and its cleanup, never with a potentially wedged provider.
        self.permit
            .consequence(|| {
                self.native_identity()?;
                if any_held(&events) {
                    return Err(Failure::HeldInput);
                }
                if let Some(point) = point {
                    if unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) } != self.root {
                        return Err(Failure::TargetChanged);
                    }
                }
                send(&events, point)
            })
            .ok_or(Failure::Cancelled)?
    }

    fn focused(&self, element: &IUIAutomationElement) -> Result<(), Failure> {
        let focused = unsafe { self.automation.GetFocusedElement() }.map_err(provider)?;
        if !unsafe { self.automation.CompareElements(element, &focused) }
            .map_err(provider)?
            .as_bool()
        {
            return Err(Failure::TargetChanged);
        }
        Ok(())
    }
}

fn provider(error: windows::core::Error) -> Failure {
    refusal(crate::windows::element_refusal(error))
}

fn refusal(error: crate::Refusal) -> Failure {
    match error {
        crate::Refusal::TargetDestroyed => Failure::TargetDestroyed,
        crate::Refusal::TargetChanged => Failure::TargetChanged,
        crate::Refusal::ProtectedTarget => Failure::ProtectedTarget,
        _ => Failure::Unavailable,
    }
}

fn any_held(events: &[Event]) -> bool {
    // Refuse user-held modifiers even for text, to avoid Ctrl+Unicode becoming
    // a command. Never release a key the runner did not press.
    [0x10, 0x11, 0x12, 0x5b, 0x5c, 0x01, 0x02]
        .into_iter()
        .chain(events.iter().filter_map(|e| {
            if let Key::Virtual(key) = e.key {
                Some(i32::from(key))
            } else {
                None
            }
        }))
        .any(|key| unsafe { GetAsyncKeyState(key) } < 0)
}

fn input(event: &Event) -> INPUT {
    let up = event.up;
    match event.key {
        Key::Virtual(key) | Key::Unicode(key) => {
            let unicode = matches!(event.key, Key::Unicode(_));
            let extended = !unicode && matches!(key, 0x21..=0x28 | 0x2d..=0x2e);
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if up {
                flags |= KEYEVENTF_KEYUP;
            }
            if unicode {
                flags |= KEYEVENTF_UNICODE;
            }
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(if unicode { 0 } else { key }),
                        wScan: if unicode { key } else { 0 },
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            }
        }
        Key::LeftMouse | Key::RightMouse => {
            let flags = match (event.key, up) {
                (Key::LeftMouse, false) => MOUSEEVENTF_LEFTDOWN,
                (Key::LeftMouse, true) => MOUSEEVENTF_LEFTUP,
                (_, false) => MOUSEEVENTF_RIGHTDOWN,
                (_, true) => MOUSEEVENTF_RIGHTUP,
            };
            INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            }
        }
    }
}

fn send(events: &[Event], point: Option<POINT>) -> Result<(), Failure> {
    if events.is_empty() {
        return Ok(());
    }
    let mut inputs: Vec<_> = Vec::new();
    if let Some(point) = point {
        let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        if width <= 1 || height <= 1 {
            return Err(Failure::Unavailable);
        }
        let x = ((i64::from(point.x) - i64::from(left)) * 65535 / i64::from(width - 1)) as i32;
        let y = ((i64::from(point.y) - i64::from(top)) * 65535 / i64::from(height - 1)) as i32;
        if !(0..=65535).contains(&x) || !(0..=65535).contains(&y) {
            return Err(Failure::Offscreen);
        }
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: x,
                    dy: y,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    ..Default::default()
                },
            },
        });
    }
    inputs.extend(events.iter().map(input));
    let accepted = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
    let cleanup: Vec<_> = gesture::pending_releases(
        events,
        accepted.saturating_sub(usize::from(point.is_some())),
    )
    .iter()
    .map(input)
    .collect();
    if !cleanup.is_empty()
        && unsafe { SendInput(&cleanup, std::mem::size_of::<INPUT>() as i32) } as usize
            != cleanup.len()
    {
        return Err(Failure::CleanupFailed);
    }
    if accepted == inputs.len() {
        Ok(())
    } else if accepted == 0 {
        Err(Failure::Rejected)
    } else {
        Err(Failure::Partial)
    }
}
#[cfg(test)]
mod native;
