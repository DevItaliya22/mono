// SPDX-License-Identifier: MPL-2.0

//! Paging and focus establishment inside the exact current document viewport.
use super::*;

#[derive(Clone, PartialEq, Eq)]
struct PageProgress {
    subject: Option<crate::inspect::Rect>,
    body: Option<crate::inspect::Rect>,
    focused: Option<(Option<String>, Option<crate::inspect::Rect>)>,
}

#[derive(Default)]
pub(super) struct Paging {
    observed: Option<PageProgress>,
    sent: Option<PageProgress>,
    focus_sent: bool,
    saw_document_focus: bool,
    bootstrap: Option<Bootstrap>,
}

enum Bootstrap {
    AwaitingFocus(String),
    Focused,
}

impl Paging {
    pub(super) fn clear_observation(&mut self) {
        self.observed = None;
    }

    /// Called once per outer poll: equal samples are separated by Clock::rest.
    /// This observes settling; native input still re-proves current geometry.
    fn settled(&mut self, current: &PageProgress) -> bool {
        let settled = self.observed.as_ref() == Some(current);
        self.observed = Some(current.clone());
        settled
    }
}

impl Executor<'_> {
    pub(super) fn document_body(&mut self) -> Result<ElementFacts, &'static str> {
        let body = Target::Locator(
            crate::locator::LocatorPattern::parse("koru:w*/v*/s:body#root").expect("body locator"),
        );
        let Selection::One(body) = self.find(&body)? else {
            return Err("document-body-unavailable");
        };
        // Validate before any focus query or native navigation consequence.
        crate::viewport::in_document_viewport(None, body.bounds)?;
        Ok(body)
    }

    pub(super) fn page_until(
        &mut self,
        subject: &ElementFacts,
        paging: &mut Paging,
        page_key: u16,
    ) -> Result<bool, &'static str> {
        let body = self.document_body()?;
        let center_inside =
            crate::viewport::center_in_document_viewport(subject.bounds, body.bounds)?;
        let focused_subject = matches!(
            self.call(Request::Check {
                element: subject.handle,
                condition: input::Condition::Focused,
            })?,
            Response::Checked(true)
        );
        paging.saw_document_focus |= focused_subject;
        // A bootstrap click must be acknowledged by its exact paragraph even
        // if animation makes the final subject visible in the meantime.
        let bootstrap_focus = if let Some(Bootstrap::AwaitingFocus(expected)) = &paging.bootstrap {
            match self.call(Request::Focused { root: body.handle })? {
                Response::Found(found) => {
                    match select::select(found, Some(self.process_id), Requirements::INSPECT) {
                        Selection::One(focused) if focused.locator.as_ref() == Some(expected) => {
                            paging.bootstrap = Some(Bootstrap::Focused);
                            Some(focused)
                        }
                        Selection::NotFound => {
                            paging.clear_observation();
                            return Ok(false);
                        }
                        _ => return Err("document-focus-changed"),
                    }
                }
                _ => return Err("unexpected-provider-response"),
            }
        } else {
            None
        };
        if center_inside {
            let progress = PageProgress {
                subject: subject.bounds,
                body: body.bounds,
                focused: None,
            };
            if !paging.settled(&progress) {
                return Ok(false);
            }
            if focused_subject {
                return Ok(true);
            }
            if paging.focus_sent {
                return Ok(false);
            }
            match self.call(Request::Input(Consequence {
                element: subject.handle,
                root_window: self.window_handle,
                process_id: self.process_id,
                action: Action::Focus,
            }))? {
                Response::Input(input::Result::Complete { .. }) => {
                    paging.focus_sent = true;
                    paging.observed = None;
                    return Ok(false);
                }
                Response::Input(input::Result::Refused(code)) => return Err(code),
                _ => return Err("unexpected-provider-response"),
            }
        }
        // PageUp/Down animates scroll over several frames. Progress must both
        // differ from the last dispatch and settle before another page; a new
        // animation frame alone is never permission to send another key.
        // Focus identity also distinguishes progress for unlaid-out subjects.
        let focused = if let Some(focused) = bootstrap_focus {
            focused
        } else {
            match self.call(Request::Focused { root: body.handle })? {
                Response::Found(found) => {
                    match select::select(found, Some(self.process_id), Requirements::INSPECT) {
                        Selection::One(focused) => focused,
                        Selection::NotFound => return self.bootstrap_focus(subject, &body, paging),
                        _ => return Err("document-focus-unavailable"),
                    }
                }
                _ => return Err("unexpected-provider-response"),
            }
        };
        if focused.role != crate::inspect::Role::Edit {
            return Err("document-focus-not-paragraph");
        }
        // Initial focus bootstrap is no longer available after observing a
        // paragraph with focus, even before the first settled page dispatch.
        paging.saw_document_focus = true;
        let progress = PageProgress {
            subject: subject.bounds,
            body: body.bounds,
            focused: Some((focused.locator.clone(), focused.bounds)),
        };
        if !paging.settled(&progress) || paging.sent.as_ref() == Some(&progress) {
            return Ok(false);
        }
        match self.call(Request::Input(Consequence {
            element: focused.handle,
            root_window: self.window_handle,
            process_id: self.process_id,
            action: Action::Chord(vec![page_key]),
        }))? {
            Response::Input(input::Result::Complete { .. }) => {
                paging.sent = Some(progress);
                paging.observed = None;
                Ok(false)
            }
            Response::Input(input::Result::Refused(code)) => Err(code),
            _ => Err("unexpected-provider-response"),
        }
    }
    /// A restored scroll position need not contain the requested entry, and
    /// opening a tab does not promise a paragraph owns the keyboard. Establish
    /// that prerequisite once through a real visible paragraph in this body.
    fn bootstrap_focus(
        &mut self,
        subject: &ElementFacts,
        body: &ElementFacts,
        paging: &mut Paging,
    ) -> Result<bool, &'static str> {
        if paging.saw_document_focus
            || paging.sent.is_some()
            || paging.focus_sent
            || matches!(paging.bootstrap, Some(Bootstrap::Focused))
        {
            return Err("document-focus-unavailable");
        }
        if paging.bootstrap.is_some() {
            // A completed focus batch is not a focus acknowledgement. Wait;
            // never click again if the provider has not published the result.
            paging.clear_observation();
            return Ok(false);
        }
        let found = match self.call(Request::MatchesPart {
            root: body.handle,
            part: "text".into(),
        })? {
            Response::Found(found) => found,
            _ => return Err("unexpected-provider-response"),
        };
        let Some(candidate) = bootstrap_candidate(found, body, self.window_slot, self.process_id)?
        else {
            paging.clear_observation();
            return Ok(false);
        };
        let progress = PageProgress {
            subject: subject.bounds,
            body: body.bounds,
            focused: Some((candidate.locator.clone(), candidate.bounds)),
        };
        if !paging.settled(&progress) {
            return Ok(false);
        }
        match self.call(Request::Input(Consequence {
            element: candidate.handle,
            root_window: self.window_handle,
            process_id: self.process_id,
            action: Action::Focus,
        }))? {
            Response::Input(input::Result::Complete { .. }) => {
                paging.bootstrap = Some(Bootstrap::AwaitingFocus(
                    candidate.locator.expect("validated candidate locator"),
                ));
                paging.clear_observation();
                Ok(false)
            }
            Response::Input(input::Result::Refused(code)) => Err(code),
            _ => Err("unexpected-provider-response"),
        }
    }
}

/// The intermediate paragraph is chosen geometrically, not by private content.
/// Discovery is scoped to the body; each retained locator additionally proves
/// its window/view/model/text identity and PID before becoming an input target.
fn bootstrap_candidate(
    found: Vec<ElementFacts>,
    body: &ElementFacts,
    window_slot: u64,
    process_id: u32,
) -> Result<Option<ElementFacts>, &'static str> {
    use crate::locator::{LocatorPattern, Object, Slot};
    let body_id = body
        .locator
        .as_deref()
        .and_then(|id| LocatorPattern::parse(id).ok())
        .filter(|id| id.exact().is_some() && id.window == Slot::Exact(window_slot))
        .ok_or("document-body-unavailable")?;
    let viewport = body.bounds.ok_or("document-viewport-unavailable")?;
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for candidate in found {
        let id = candidate
            .locator
            .as_deref()
            .and_then(|id| LocatorPattern::parse(id).ok())
            .filter(|id| {
                id.exact().is_some() && matches!(id.object, Object::Model(_)) && id.part == "text"
            })
            .ok_or("document-paragraph-scope-changed")?;
        if candidate.process_id != Some(process_id) {
            return Err("wrong-process");
        }
        if id.window != body_id.window || id.view != body_id.view {
            return Err("document-paragraph-scope-changed");
        }
        if !seen.insert(candidate.locator.clone()) {
            return Err("ambiguous-target");
        }
        if candidate.role != crate::inspect::Role::Edit
            || !candidate.enabled
            || candidate.offscreen
            || !crate::viewport::center_in_document_viewport(candidate.bounds, body.bounds)?
        {
            continue;
        }
        candidates.push(candidate);
    }
    // Prefer the center of the visible document, away from clipped edges. Exact
    // locator ordering makes ties deterministic without depending on UIA order.
    candidates.sort_by_key(|candidate| {
        let bounds = candidate
            .bounds
            .expect("validated nonempty candidate bounds");
        let distance = (i64::from(bounds.left) + i64::from(bounds.right)
            - i64::from(viewport.left)
            - i64::from(viewport.right))
        .abs()
            + (i64::from(bounds.top) + i64::from(bounds.bottom)
                - i64::from(viewport.top)
                - i64::from(viewport.bottom))
            .abs();
        (distance, candidate.locator.clone())
    });
    Ok(candidates.into_iter().next())
}
