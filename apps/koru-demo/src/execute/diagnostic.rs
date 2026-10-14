// SPDX-License-Identifier: MPL-2.0

//! Expected state and observed state without copying expected or actual content.
use crate::{
    cases::Operation,
    input::{Action, Condition},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    pub kind: &'static str,
    pub boolean: Option<bool>,
    pub content_redacted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Observed {
    pub matched: Option<bool>,
    pub boolean: Option<bool>,
}

impl Expected {
    pub(super) fn for_operation(operation: &Operation) -> Self {
        let (kind, boolean, content_redacted) = match operation {
            Operation::Expect(condition) => (
                condition.kind(),
                match condition {
                    Condition::Focused => Some(true),
                    Condition::Enabled(value)
                    | Condition::Offscreen(value)
                    | Condition::ReadOnly(value)
                    | Condition::Selected(value) => Some(*value),
                    _ => None,
                },
                matches!(
                    condition,
                    Condition::Text(_)
                        | Condition::Name(_)
                        | Condition::NamePrefixWithout { .. }
                        | Condition::SelectedText(_)
                ),
            ),
            Operation::Present => ("present", Some(true), false),
            Operation::Absent => ("present", Some(false), false),
            Operation::InDocumentViewport(wanted) => ("in-document-viewport", Some(*wanted), false),
            Operation::ReachByPageDown | Operation::ReachByPageUp => {
                ("viewport-center-and-focused", Some(true), false)
            }
            Operation::Input(action) => (
                "input-complete",
                Some(true),
                matches!(action, Action::Text(_)),
            ),
        };
        Self {
            kind,
            boolean,
            content_redacted,
        }
    }

    pub(super) fn observe(&self, matched: bool) -> Observed {
        Observed {
            matched: Some(matched),
            boolean: self
                .boolean
                .map(|expected| if matched { expected } else { !expected }),
        }
    }
}
