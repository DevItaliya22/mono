// SPDX-License-Identifier: MPL-2.0

//! Repetition is compiled orchestration. A failed case never starts a reset.
use crate::{
    cases::{self, Case, Fixture, Operation, Step, Target},
    execute::StepResult,
    input::{Action, Condition},
    locator::LocatorPattern,
    report::RunReport,
};

pub fn reset_document(fixture: &Fixture) -> Result<Case, &'static str> {
    let subject = Target::Locator(fixture.subject.clone());
    let entry = Target::Locator(fixture.entry.clone().ok_or("fixture-entry-required")?);
    let save = Target::Locator(LocatorPattern::parse("koru:w*/v*/s:save#document").unwrap());
    let step = |id, target: &Target, operation| Step {
        id,
        target: target.clone(),
        operation,
    };
    Ok(Case {
        name: "document-edit-reset",
        steps: vec![
            step("reset-reach-subject", &subject, Operation::ReachByPageDown),
            step(
                "reset-exact-after",
                &subject,
                Operation::Expect(Condition::Text(cases::DOCUMENT_AFTER.into())),
            ),
            step(
                "reset-subject-visible",
                &subject,
                Operation::InDocumentViewport(true),
            ),
            step(
                "reset-focus-subject",
                &subject,
                Operation::Input(Action::Focus),
            ),
            step(
                "reset-line-start",
                &subject,
                Operation::Input(Action::Chord(vec![0x24])),
            ),
            step(
                "reset-select-line",
                &subject,
                Operation::Input(Action::Chord(vec![0x10, 0x23])),
            ),
            step(
                "reset-selection-exact",
                &subject,
                Operation::Expect(Condition::SelectedText(cases::DOCUMENT_AFTER.into())),
            ),
            step(
                "reset-replace",
                &subject,
                Operation::Input(Action::Text(cases::DOCUMENT_BEFORE.into())),
            ),
            step(
                "reset-text-before",
                &subject,
                Operation::Expect(Condition::Text(cases::DOCUMENT_BEFORE.into())),
            ),
            step(
                "reset-saved",
                &save,
                Operation::Expect(Condition::Name("Saved.".into())),
            ),
            step("reset-reach-entry", &entry, Operation::ReachByPageUp),
            step(
                "reset-entry-visible",
                &entry,
                Operation::InDocumentViewport(true),
            ),
            step(
                "reset-subject-outside-document-viewport",
                &subject,
                Operation::InDocumentViewport(false),
            ),
        ],
    })
}

pub fn succeeded(case: &Case, results: &[StepResult]) -> bool {
    results.len() == case.steps.len() && results.iter().all(|result| result.failure.is_none())
}

/// The callback is the same visible executor for the main case and reset.
pub fn run(
    name: &str,
    fixtures: &[Fixture],
    report: &mut RunReport,
    mut execute: impl FnMut(usize, &str, &Case, &mut RunReport) -> Result<Vec<StepResult>, &'static str>,
) {
    for (index, fixture) in fixtures.iter().enumerate() {
        let iteration = index + 1;
        let case = match cases::compile(name, fixture) {
            Ok(case) => case,
            Err(code) => {
                report.fail("compile", code);
                return;
            }
        };
        let results = match execute(iteration, "case", &case, report) {
            Ok(results) => results,
            Err(code) => {
                report.fail("execution", code);
                return;
            }
        };
        report.add_case(iteration, "case", &case, &results);
        if !succeeded(&case, &results) {
            return;
        }
        if name == "document-edit" && iteration < fixtures.len() {
            let reset = match reset_document(fixture) {
                Ok(case) => case,
                Err(code) => {
                    report.fail("reset", code);
                    return;
                }
            };
            let results = match execute(iteration, "reset", &reset, report) {
                Ok(results) => results,
                Err(code) => {
                    report.fail("execution", code);
                    return;
                }
            };
            report.add_case(iteration, "reset", &reset, &results);
            if !succeeded(&reset, &results) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
