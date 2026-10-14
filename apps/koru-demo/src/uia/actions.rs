// SPDX-License-Identifier: MPL-2.0

//! Provider reads compare expected content inside the apartment and return booleans.
use super::*;
use crate::input::{Action, Condition, Consequence};
use koru_windows_input::{DeliveryPermit, workflow};
use windows::Win32::UI::Accessibility::{
    IUIAutomationTextPattern, IUIAutomationValuePattern, UIA_TextPatternId, UIA_ValuePatternId,
};

impl Client {
    pub(super) fn focused_in(&mut self, root: Handle) -> Response {
        let root = match self.element(root) {
            Ok(root) => root.clone(),
            Err(error) => return Response::Failed(error),
        };
        let condition = match unsafe {
            self.client.automation().CreatePropertyCondition(
                windows::Win32::UI::Accessibility::UIA_HasKeyboardFocusPropertyId,
                &windows::Win32::System::Variant::VARIANT::from(true),
            )
        } {
            Ok(c) => c,
            Err(error) => return Response::Failed(classify(&error)),
        };
        match unsafe { root.FindAll(TreeScope_Subtree, &condition) } {
            Ok(found) => self.collect(found, |_| true),
            Err(error) => Response::Failed(classify(&error)),
        }
    }
    pub(super) fn check(&self, handle: Handle, condition: Condition) -> Result<bool, InspectError> {
        let element = self.element(handle)?;
        unsafe {
            Ok(match condition {
                Condition::Focused => read(element.CurrentHasKeyboardFocus())?.as_bool(),
                Condition::Enabled(wanted) => read(element.CurrentIsEnabled())?.as_bool() == wanted,
                Condition::Offscreen(wanted) => {
                    read(element.CurrentIsOffscreen())?.as_bool() == wanted
                }
                Condition::Name(wanted) => read(element.CurrentName())?.to_string() == wanted,
                Condition::NamePrefixWithout { prefix, excluded } => {
                    let name = read(element.CurrentName())?.to_string();
                    name.starts_with(&prefix) && !name.contains(&excluded)
                }
                Condition::Selected(wanted) => {
                    let available = read(element.GetCurrentPropertyValue(windows::Win32::UI::Accessibility::UIA_IsSelectionItemPatternAvailablePropertyId))?;
                    let available = bool::try_from(&available).map_err(|_| {
                        InspectError::Unavailable("selection-pattern-state-unavailable".into())
                    })?;
                    if !available {
                        !wanted
                    } else {
                        let pattern: windows::Win32::UI::Accessibility::IUIAutomationSelectionItemPattern =
                            read(element.GetCurrentPatternAs(windows::Win32::UI::Accessibility::UIA_SelectionItemPatternId))?;
                        read(pattern.CurrentIsSelected())?.as_bool() == wanted
                    }
                }
                Condition::ReadOnly(wanted) => {
                    let pattern: IUIAutomationValuePattern =
                        read(element.GetCurrentPatternAs(UIA_ValuePatternId))?;
                    read(pattern.CurrentIsReadOnly())?.as_bool() == wanted
                }
                Condition::Text(wanted) => {
                    if let Ok(pattern) =
                        element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    {
                        read(pattern.CurrentValue())?.to_string() == wanted
                    } else {
                        let pattern: IUIAutomationTextPattern =
                            read(element.GetCurrentPatternAs(UIA_TextPatternId))?;
                        let range = read(pattern.DocumentRange())?;
                        read(range.GetText((wanted.encode_utf16().count() + 1).min(65536) as i32))?
                            .to_string()
                            == wanted
                    }
                }
                Condition::SelectedText(wanted) => {
                    let pattern: IUIAutomationTextPattern =
                        read(element.GetCurrentPatternAs(UIA_TextPatternId))?;
                    let selection = read(pattern.GetSelection())?;
                    if read(selection.Length())? != 1 {
                        false
                    } else {
                        read(
                            read(selection.GetElement(0))?
                                .GetText((wanted.encode_utf16().count() + 1).min(65536) as i32),
                        )?
                        .to_string()
                            == wanted
                    }
                }
            })
        }
    }

    pub(super) fn input(&mut self, request: Consequence, permit: &DeliveryPermit) -> Response {
        let Some(owner) = &self.owner else {
            return input_refused("instance-owner-unproved");
        };
        if owner.process_id() != request.process_id {
            return input_refused("instance-owner-mismatch");
        }
        let Some(desktop) = &self.desktop else {
            return input_refused("desktop-unavailable");
        };
        let element = match self.element(request.element) {
            Ok(element) => element,
            Err(error) => return Response::Failed(error),
        };
        let action = match &request.action {
            Action::Focus => workflow::Action::Focus,
            Action::Click => workflow::Action::Click { right: false },
            Action::RightClick => workflow::Action::Click { right: true },
            Action::Text(text) => workflow::Action::Text(text),
            Action::Chord(keys) => workflow::Action::Chord(keys),
        };
        let context = workflow::Context {
            automation: self.client.automation(),
            root: HWND(request.root_window as *mut _),
            pid: request.process_id,
            desktop,
            permit,
            owner_alive: &|| owner.is_alive(),
        };
        match context.execute(element, action) {
            Ok(()) => Response::Input(crate::input::Result::Complete { bounds: None }),
            Err(error) => input_refused(failure_code(error)),
        }
    }

    pub(super) fn matches_part(&mut self, root: Handle, part: &str) -> Response {
        let root = match self.element(root) {
            Ok(root) => root.clone(),
            Err(error) => return Response::Failed(error),
        };
        let condition = match unsafe { self.client.automation().CreateTrueCondition() } {
            Ok(condition) => condition,
            Err(error) => return Response::Failed(classify(&error)),
        };
        let found = match unsafe { root.FindAll(TreeScope_Subtree, &condition) } {
            Ok(found) => found,
            Err(error) => return Response::Failed(classify(&error)),
        };
        let suffix = format!("#{part}");
        self.collect(found, |facts| {
            facts
                .locator
                .as_deref()
                .is_some_and(|id| id.ends_with(&suffix))
        })
    }
}

fn input_refused(code: &'static str) -> Response {
    Response::Input(crate::input::Result::Refused(code))
}

fn failure_code(error: workflow::Failure) -> &'static str {
    use workflow::Failure::*;
    match error {
        Cancelled => "cancelled",
        TargetChanged => "target-changed",
        TargetDestroyed => "target-destroyed",
        ProtectedTarget => "protected-target",
        Unavailable => "target-unavailable",
        Offscreen => "offscreen",
        StaleBounds => "stale-bounds",
        HeldInput => "held-input",
        Rejected => "input-rejected",
        Partial => "input-partial",
        CleanupFailed => "input-release-failed",
        InvalidBatch => "invalid-input-batch",
    }
}
