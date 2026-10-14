// SPDX-License-Identifier: MPL-2.0

//! Sequential execution with bounded provider calls and state-based synchronization.
use crate::{
    cases::{Case, Operation, Step, Target},
    input::{self, Action, Consequence},
    inspect::{ElementFacts, Handle, Request, Response},
    select::{self, Requirements, Selection},
    supervise::{CancelToken, Outcome, Supervisor},
    wait::{Clock, Patience},
};
use std::time::Duration;
mod diagnostic;
mod navigation;
pub use diagnostic::{Expected, Observed};
use navigation::Paging;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepResult {
    pub step: &'static str,
    pub operation: &'static str,
    pub elapsed: Duration,
    /// Facts from the last lookup, not the native consequence geometry.
    pub target: Option<ElementFacts>,
    pub selection: Option<Selection>,
    pub failure: Option<&'static str>,
    pub expected: Expected,
    pub actual: Observed,
    /// Instant the native input consequence completed. Assertions and waits
    /// leave this empty: they are not delivered actions.
    pub delivered_at: Option<std::time::Instant>,
    /// Instant this step returned. Recording stamps assertion times from it.
    pub finished_at: Option<std::time::Instant>,
}

pub struct Executor<'a> {
    pub supervisor: &'a mut Supervisor,
    pub cancel: &'a CancelToken,
    pub root: Handle,
    pub window_slot: u64,
    pub window_handle: i64,
    pub process_id: u32,
    pub patience: Patience,
    pub clock: &'a dyn Clock,
}

impl Executor<'_> {
    pub fn run(&mut self, case: &Case) -> Vec<StepResult> {
        let mut out = Vec::new();
        for step in &case.steps {
            let result = self.step(step);
            let failed = result.failure.is_some();
            out.push(result);
            if failed {
                break;
            }
        }
        out
    }

    fn call(&mut self, request: Request) -> Result<Response, &'static str> {
        match self.supervisor.call(request, self.cancel) {
            Outcome::Answered(Response::Failed(crate::inspect::InspectError::ElementGone)) => {
                Err("target-destroyed")
            }
            Outcome::Answered(Response::Failed(crate::inspect::InspectError::StaleHandle)) => {
                Err("stale-handle")
            }
            Outcome::Answered(Response::Failed(_)) => Err("provider-unavailable"),
            Outcome::Answered(response) => Ok(response),
            Outcome::Cancelled => Err("cancelled"),
            Outcome::Lost => Err("provider-timeout"),
            Outcome::Wedged { .. } => Err("provider-wedged"),
            Outcome::Unavailable(_) => Err("provider-unavailable"),
        }
    }

    fn find(&mut self, target: &Target) -> Result<Selection, &'static str> {
        let request = match target {
            Target::Locator(pattern) => Request::Matches {
                root: self.root,
                pattern: pattern.clone().bound_to_window(self.window_slot),
            },
            Target::Named {
                role,
                name,
                language,
            } => Request::MatchesByName {
                root: self.root,
                role: *role,
                name: name.clone(),
                language: (*language).into(),
            },
            Target::Part { parent, part } => {
                let found = self.find(parent)?;
                let Selection::One(parent) = found else {
                    return Ok(found);
                };
                Request::MatchesPart {
                    root: parent.handle,
                    part: (*part).into(),
                }
            }
        };
        match self.call(request)? {
            Response::Found(found) => Ok(select::select(
                found,
                Some(self.process_id),
                Requirements::INSPECT,
            )),
            _ => Err("unexpected-provider-response"),
        }
    }

    pub fn step(&mut self, step: &Step) -> StepResult {
        let start = self.clock.now();
        let mut result = StepResult {
            step: step.id,
            operation: match &step.operation {
                Operation::Input(action) => action.kind(),
                Operation::Expect(condition) => condition.kind(),
                Operation::Present => "present",
                Operation::Absent => "absent",
                Operation::InDocumentViewport(_) => "in-document-viewport",
                Operation::ReachByPageDown => "page-down-to-subject",
                Operation::ReachByPageUp => "page-up-to-subject",
            },
            elapsed: Duration::ZERO,
            target: None,
            selection: None,
            failure: None,
            expected: Expected::for_operation(&step.operation),
            actual: Observed::default(),
            delivered_at: None,
            finished_at: None,
        };
        let mut paging = Paging::default();
        loop {
            if self.cancel.is_cancelled() {
                result.failure = Some("cancelled");
                break;
            }
            result.actual = Observed::default();
            result.target = None;
            result.selection = None;
            let found = match self.find(&step.target) {
                Ok(found) => found,
                Err(error) => {
                    result.failure = Some(error);
                    break;
                }
            };
            result.selection = Some(found.clone());
            if matches!(step.operation, Operation::Present | Operation::Absent) {
                if matches!(found, Selection::NotFound) {
                    result.actual = result
                        .expected
                        .observe(matches!(step.operation, Operation::Absent));
                }
            }
            if matches!(step.operation, Operation::Absent) && matches!(found, Selection::NotFound) {
                break;
            }
            match found {
                Selection::One(facts) => {
                    result.target = Some(facts.clone());
                    let outcome = match &step.operation {
                        Operation::Present => Ok(true),
                        Operation::Absent => Ok(false),
                        Operation::InDocumentViewport(wanted) => {
                            self.document_body().and_then(|body| {
                                crate::viewport::in_document_viewport(facts.bounds, body.bounds)
                                    .map(|inside| inside == *wanted)
                            })
                        }
                        Operation::Expect(condition) => match self.call(Request::Check {
                            element: facts.handle,
                            condition: condition.clone(),
                        }) {
                            Ok(Response::Checked(matched)) => Ok(matched),
                            Ok(_) => Err("unexpected-provider-response"),
                            Err(error) => Err(error),
                        },
                        Operation::ReachByPageDown | Operation::ReachByPageUp => self.page_until(
                            &facts,
                            &mut paging,
                            if matches!(step.operation, Operation::ReachByPageUp) {
                                0x21
                            } else {
                                0x22
                            },
                        ),
                        Operation::Input(action) => {
                            let requirements = if matches!(
                                action,
                                Action::Focus | Action::Click | Action::RightClick
                            ) {
                                Requirements::ACTIONABLE
                            } else {
                                Requirements {
                                    must_be_enabled: true,
                                    must_be_on_screen: false,
                                }
                            };
                            let usable = select::select(
                                vec![facts.clone()],
                                Some(self.process_id),
                                requirements,
                            );
                            if !matches!(usable, Selection::One(_)) {
                                result.selection = Some(usable);
                                Ok(false)
                            } else {
                                match self.call(Request::Input(Consequence {
                                    element: facts.handle,
                                    root_window: self.window_handle,
                                    process_id: self.process_id,
                                    action: action.clone(),
                                })) {
                                    Ok(Response::Input(input::Result::Complete { .. })) => {
                                        result.delivered_at = Some(self.clock.now());
                                        Ok(true)
                                    }
                                    Ok(Response::Input(input::Result::Refused(code))) => {
                                        result.actual = result.expected.observe(false);
                                        Err(code)
                                    }
                                    Ok(_) => Err("unexpected-provider-response"),
                                    Err(error) => Err(error),
                                }
                            }
                        }
                    };
                    if let Ok(matched) = outcome {
                        result.actual = result.expected.observe(matched);
                    }
                    match outcome {
                        Ok(true) => break,
                        Ok(false) => {}
                        Err(error) => {
                            result.failure = Some(error);
                            break;
                        }
                    }
                }
                Selection::Ambiguous(_) => {
                    result.failure = Some("ambiguous-target");
                    break;
                }
                Selection::WrongProcess { .. } => {
                    result.failure = Some("wrong-process");
                    break;
                }
                _ => paging.clear_observation(),
            }
            let elapsed = self.clock.now().saturating_duration_since(start);
            if elapsed >= self.patience.limit {
                result.failure = Some("state-timeout");
                break;
            }
            self.clock
                .rest(self.patience.interval.min(self.patience.limit - elapsed));
        }
        result.elapsed = self.clock.now().saturating_duration_since(start);
        result.finished_at = Some(self.clock.now());
        result
    }
}

#[cfg(test)]
mod tests;
