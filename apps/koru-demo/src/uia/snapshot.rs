// SPDX-License-Identifier: MPL-2.0

//! Explicit prepared-fixture content reads, isolated from ordinary diagnostics.
use super::*;
use crate::{
    inspect::ContentSnapshot,
    locator::{LocatorPattern, Object},
};
use windows::Win32::UI::Accessibility::{
    IUIAutomationTextPattern, IUIAutomationValuePattern, UIA_TextPatternId, UIA_ValuePatternId,
};

fn refused(code: &str) -> InspectError {
    InspectError::Unavailable(code.into())
}

impl Client {
    fn content_owner(&self, element: &IUIAutomationElement) -> Result<(), InspectError> {
        let owner = self
            .owner
            .as_ref()
            .ok_or_else(|| refused("instance-owner-unproved"))?;
        if !owner.is_alive()
            || read(unsafe { element.CurrentProcessId() })? != owner.process_id() as i32
        {
            return Err(refused("instance-owner-mismatch"));
        }
        Ok(())
    }

    fn in_bound_window(&self, element: &IUIAutomationElement) -> Result<(), InspectError> {
        let root = self.element(self.bound_root.ok_or_else(|| refused("window-unbound"))?)?;
        self.content_owner(root)?;
        self.content_owner(element)?;
        let walker = read(unsafe { self.client.automation().RawViewWalker() })?;
        let mut current = element.clone();
        for _ in 0..256 {
            if read(unsafe { self.client.automation().CompareElements(&current, root) })?.as_bool()
            {
                return Ok(());
            }
            current = read(unsafe { walker.GetParentElement(&current) })?;
        }
        Err(refused("snapshot-outside-window"))
    }

    pub(super) fn snapshot(
        &mut self,
        root: Handle,
        handle: Handle,
        max_utf16: u32,
    ) -> Result<ContentSnapshot, InspectError> {
        if self.bound_root != Some(root) || !(1..=16384).contains(&max_utf16) {
            return Err(refused("snapshot-scope-invalid"));
        }
        let element = self.element(handle)?.clone();
        self.in_bound_window(&element)?;
        if read(unsafe { element.CurrentIsPassword() })?.as_bool() {
            return Err(refused("protected-target"));
        }
        let facts = self.facts(element.clone())?;
        if !model_part(&facts, "text") {
            return Err(refused("snapshot-not-fixture-paragraph"));
        }
        let text = unsafe {
            if let Ok(pattern) =
                element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            {
                read(pattern.CurrentValue())?.to_string()
            } else {
                let pattern: IUIAutomationTextPattern =
                    read(element.GetCurrentPatternAs(UIA_TextPatternId))?;
                read(read(pattern.DocumentRange())?.GetText(max_utf16 as i32))?.to_string()
            }
        };
        // Provider work can block, so reprove scope/lifetime before returning content.
        self.in_bound_window(&element)?;
        ContentSnapshot::bounded(facts, text, max_utf16)
    }

    pub(super) fn fixture_children(
        &mut self,
        handle: Handle,
        part: &str,
        max_nodes: u32,
    ) -> Result<Vec<ElementFacts>, InspectError> {
        if !(1..=256).contains(&max_nodes) || !matches!(part, "text" | "root") {
            return Err(refused("fixture-discovery-limit-invalid"));
        }
        let root = self.element(handle)?.clone();
        self.in_bound_window(&root)?;
        let walker = read(unsafe { self.client.automation().ControlViewWalker() })?;
        let mut pending = vec![root.clone()];
        let mut scanned = 0usize;
        let mut out = Vec::new();
        while let Some(element) = pending.pop() {
            scanned += 1;
            if scanned > 4096 {
                return Err(refused("fixture-discovery-too-large"));
            }
            self.content_owner(&element)?;
            let facts = self.facts(element.clone())?;
            if model_part(&facts, part) {
                if out.len() >= max_nodes as usize {
                    return Err(refused("fixture-discovery-too-large"));
                }
                out.push(facts);
            }
            // The pinned binding maps a null interface to Error::empty (code 0).
            // Other failures must stay failures, not a truncated successful manifest.
            if let Some(child) = optional(unsafe { walker.GetFirstChildElement(&element) })? {
                let mut children = vec![child];
                loop {
                    if scanned + pending.len() + children.len() > 4096 {
                        return Err(refused("fixture-discovery-too-large"));
                    }
                    let Some(next) = optional(unsafe {
                        walker.GetNextSiblingElement(children.last().unwrap())
                    })?
                    else {
                        break;
                    };
                    children.push(next);
                }
                pending.extend(children.into_iter().rev());
            }
        }
        self.in_bound_window(&root)?;
        Ok(out)
    }
}

fn optional(
    result: windows::core::Result<IUIAutomationElement>,
) -> Result<Option<IUIAutomationElement>, InspectError> {
    match result {
        Ok(element) => Ok(Some(element)),
        Err(error) if error.code() == windows::core::HRESULT(0) => Ok(None),
        Err(error) => Err(classify(&error)),
    }
}

fn model_part(facts: &ElementFacts, part: &str) -> bool {
    facts
        .locator
        .as_deref()
        .and_then(|id| LocatorPattern::parse(id).ok())
        .is_some_and(|id| {
            id.exact().is_some() && matches!(id.object, Object::Model(_)) && id.part == part
        })
}

#[cfg(test)]
mod tests;
