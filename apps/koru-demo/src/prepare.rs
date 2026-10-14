// SPDX-License-Identifier: MPL-2.0

//! Scoped UIA reads and visible, non-mutating fixture preparation.
//! Final diagnostic snapshots remain read-only.
use crate::{
    cases::{Operation, Step, Target},
    execute::Executor,
    fixtures::{self, Manifest, Paragraph},
    input::Condition,
    inspect::{ContentSnapshot, ElementFacts, Handle, Request, Response},
    locator::LocatorPattern,
    select::{self, Requirements, Selection},
    supervise::{CancelToken, Outcome, Supervisor},
    viewport::{center_in_document_viewport, in_document_viewport},
    wait::{Clock, Patience},
};

/// Only capture receives navigation authority; scoped diagnostic reads do not.
pub struct Navigation<'a> {
    pub window_handle: i64,
    pub patience: Patience,
    pub clock: &'a dyn Clock,
}

pub struct Reader<'a> {
    pub supervisor: &'a mut Supervisor,
    pub cancel: &'a CancelToken,
    pub root: Handle,
    pub process_id: u32,
    pub window_slot: u64,
}
impl Reader<'_> {
    fn call(&mut self, request: Request) -> Result<Response, &'static str> {
        match self.supervisor.call(request, self.cancel) {
            Outcome::Answered(Response::Failed(_)) => Err("fixture-provider-failed"),
            Outcome::Answered(response) => Ok(response),
            Outcome::Cancelled => Err("cancelled"),
            _ => Err("fixture-provider-unavailable"),
        }
    }
    pub fn one(&mut self, pattern: LocatorPattern) -> Result<ElementFacts, &'static str> {
        let Response::Found(found) = self.call(Request::Matches {
            root: self.root,
            pattern: pattern.bound_to_window(self.window_slot),
        })?
        else {
            return Err("fixture-provider-response");
        };
        match select::select(found, Some(self.process_id), Requirements::INSPECT) {
            Selection::One(facts) => Ok(facts),
            Selection::NotFound => Err("fixture-target-missing"),
            _ => Err("fixture-target-ambiguous-or-unusable"),
        }
    }
    fn children(&mut self, root: Handle, part: &str) -> Result<Vec<ElementFacts>, &'static str> {
        let Response::Found(found) = self.call(Request::FixtureChildren {
            root,
            part: part.into(),
            max_nodes: 256,
        })?
        else {
            return Err("fixture-provider-response");
        };
        if found.len() > 256
            || found
                .iter()
                .any(|facts| facts.process_id != Some(self.process_id))
        {
            return Err("fixture-children-invalid");
        }
        Ok(found)
    }
    pub fn snapshot(&mut self, element: Handle) -> Result<ContentSnapshot, &'static str> {
        let Response::Snapshot(snapshot) = self.call(Request::Snapshot {
            root: self.root,
            element,
            max_utf16: 4096,
        })?
        else {
            return Err("fixture-provider-response");
        };
        if snapshot.truncated {
            return Err("fixture-content-truncated");
        }
        Ok(snapshot)
    }
    fn selected(&mut self, selected: &ElementFacts, title: &str) -> Result<(), &'static str> {
        for (condition, code) in [
            (
                Condition::Selected(true),
                "fixture-selected-document-changed",
            ),
            (
                Condition::Name(title.into()),
                "fixture-selected-title-changed",
            ),
        ] {
            if !matches!(
                self.call(Request::Check {
                    element: selected.handle,
                    condition
                })?,
                Response::Checked(true)
            ) {
                return Err(code);
            }
        }
        Ok(())
    }
    fn navigate(
        &mut self,
        facts: &ElementFacts,
        operation: Operation,
        navigation: &Navigation<'_>,
    ) -> Result<(), &'static str> {
        // This private boundary admits only focus and page navigation. It cannot
        // become a hidden content repair path as fixture validation evolves.
        if !matches!(
            &operation,
            Operation::Input(crate::input::Action::Focus)
                | Operation::ReachByPageDown
                | Operation::ReachByPageUp
        ) {
            return Err("fixture-navigation-invalid");
        }
        let result = Executor {
            supervisor: self.supervisor,
            cancel: self.cancel,
            root: self.root,
            window_slot: self.window_slot,
            window_handle: navigation.window_handle,
            process_id: self.process_id,
            patience: navigation.patience,
            clock: navigation.clock,
        }
        .step(&Step {
            id: "prepare-navigation",
            target: Target::Locator(pattern(facts)?),
            operation,
        });
        match result.failure {
            Some(code) => Err(code),
            None => Ok(()),
        }
    }
    pub fn capture(
        &mut self,
        manifest: &mut Manifest,
        name: &str,
        date: Option<&str>,
        time: Option<&str>,
        navigation: &Navigation<'_>,
    ) -> Result<(), &'static str> {
        let tree = self.one(LocatorPattern::parse("koru:w*/v*/s:explore#tree").unwrap())?;
        let mut selected = Vec::new();
        for facts in self.children(tree.handle, "root")? {
            match self.call(Request::Check {
                element: facts.handle,
                condition: Condition::Selected(true),
            })? {
                Response::Checked(true) => selected.push(facts),
                Response::Checked(false) => {}
                _ => return Err("fixture-provider-response"),
            }
        }
        if selected.len() != 1 {
            return Err("fixture-selected-document-ambiguous-or-missing");
        }
        let document = fixtures::model_id(
            selected[0]
                .locator
                .as_deref()
                .ok_or("fixture-locator-missing")?,
            "root",
        )?;
        let expected_title = fixtures::title(name)?;
        if !matches!(
            self.call(Request::Check {
                element: selected[0].handle,
                condition: Condition::Name(expected_title.into())
            })?,
            Response::Checked(true)
        ) {
            return Err("fixture-selected-title-mismatch");
        }
        let body = self.one(LocatorPattern::parse("koru:w*/v*/s:body#root").unwrap())?;
        let initial = self.children(body.handle, "text")?;
        if initial.len() < 2 {
            return Err("fixture-owning-title-missing");
        }
        let count = if name == "document-edit" {
            initial.len().saturating_sub(3)
        } else {
            fixtures::FILLERS
        };
        let content = fixtures::document_text_with_count(name, count)?;
        let expected: Vec<_> = std::iter::once(expected_title)
            .chain(content.split('\n'))
            .collect();
        if initial.len() != expected.len() {
            return Err("fixture-content-or-order-mismatch");
        }
        let mut ids = std::collections::HashSet::new();
        for facts in &initial {
            let id = fixtures::model_id(
                facts.locator.as_deref().ok_or("fixture-locator-missing")?,
                "text",
            )?;
            if !ids.insert(id) {
                return Err("fixture-duplicate-paragraph");
            }
        }
        // Establish the visible owning title and entry before any navigation.
        // The rest may legitimately have neither geometry nor a TextPattern yet.
        for facts in &initial[..2] {
            if !in_document_viewport(facts.bounds, body.bounds)? {
                return Err("fixture-entry-not-visible");
            }
        }
        if !center_in_document_viewport(initial[1].bounds, body.bounds)? {
            return Err("fixture-entry-not-visible");
        }
        let mut paragraphs = Vec::new();
        let mut navigated = false;
        for (index, original) in initial.iter().enumerate() {
            let mut facts = self.one(pattern(original)?)?;
            if index >= 2 && facts.bounds.is_none_or(|bounds| bounds.is_empty()) {
                self.selected(&selected[0], expected_title)?;
                if !navigated {
                    let entry = self.one(pattern(&initial[1])?)?;
                    let viewport =
                        self.one(LocatorPattern::parse("koru:w*/v*/s:body#root").unwrap())?;
                    if !center_in_document_viewport(entry.bounds, viewport.bounds)? {
                        return Err("fixture-entry-not-visible");
                    }
                    self.navigate(
                        &initial[1],
                        Operation::Input(crate::input::Action::Focus),
                        navigation,
                    )?;
                    navigated = true;
                }
                self.navigate(original, Operation::ReachByPageDown, navigation)?;
                facts = self.one(pattern(original)?)?;
            }
            let snapshot = self.snapshot(facts.handle)?;
            if snapshot.facts.locator != original.locator
                || snapshot.facts.process_id != Some(self.process_id)
            {
                return Err("fixture-target-changed");
            }
            if snapshot.text != expected[index] {
                return Err(if index == 0 {
                    "fixture-title-mismatch"
                } else {
                    "fixture-content-or-order-mismatch"
                });
            }
            paragraphs.push(Paragraph {
                id: fixtures::model_id(original.locator.as_deref().unwrap(), "text")?,
                text: snapshot.text,
                // Filled from fresh geometry only after the entry is restored.
                in_viewport: false,
            });
        }
        if navigated {
            self.selected(&selected[0], expected_title)?;
            self.navigate(&initial[1], Operation::ReachByPageUp, navigation)?;
        }
        let restored_body = self.one(LocatorPattern::parse("koru:w*/v*/s:body#root").unwrap())?;
        if restored_body.locator != body.locator {
            return Err("fixture-target-changed");
        }
        let restored = self.children(restored_body.handle, "text")?;
        if restored.len() != initial.len()
            || restored
                .iter()
                .zip(&initial)
                .any(|(now, before)| now.locator != before.locator)
        {
            return Err("fixture-paragraph-order-changed");
        }
        for (paragraph, facts) in paragraphs.iter_mut().zip(restored) {
            paragraph.in_viewport = in_document_viewport(facts.bounds, restored_body.bounds)?;
        }
        if !matches!(
            self.call(Request::Check {
                element: selected[0].handle,
                condition: Condition::Selected(true)
            })?,
            Response::Checked(true)
        ) {
            return Err("fixture-selected-document-changed");
        }
        if !matches!(
            self.call(Request::Check {
                element: selected[0].handle,
                condition: Condition::Name(expected_title.into())
            })?,
            Response::Checked(true)
        ) {
            return Err("fixture-selected-title-changed");
        }
        fixtures::capture(manifest, name, document, paragraphs, date, time)
    }
}

fn pattern(facts: &ElementFacts) -> Result<LocatorPattern, &'static str> {
    LocatorPattern::parse(facts.locator.as_deref().ok_or("fixture-locator-missing")?)
        .map_err(|_| "fixture-locator-invalid")
}

#[cfg(test)]
mod tests;
