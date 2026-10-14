// SPDX-License-Identifier: MPL-2.0

use crate::{ApprovalChoiceId, ApprovalDecision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Semantic intent; provider option identifiers never cross this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalIntent {
    Allow,
    AllowRemembered,
    Reject,
    RejectRemembered,
}

impl ApprovalIntent {
    pub fn grants(self) -> bool {
        matches!(self, Self::Allow | Self::AllowRemembered)
    }
    pub fn decision(self) -> ApprovalDecision {
        if self.grants() {
            ApprovalDecision::Approve
        } else {
            ApprovalDecision::Reject
        }
    }
}

/// Label is approval content, drawn but never included in diagnostics.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalChoice {
    pub id: ApprovalChoiceId,
    pub intent: ApprovalIntent,
    pub label: String,
}

impl std::fmt::Debug for ApprovalChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalChoice")
            .field("id", &self.id)
            .field("intent", &self.intent)
            .field("label", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalChoiceError {
    Stale,
    Unrepresentable,
}

/// Resolve against the choices of the approval the adapter verified is pending.
/// Explicit identity is authoritative; legacy answers need exactly one matching
/// one-time intent and never imply a remembered permission.
pub fn resolve_approval_choice(
    choices: &[ApprovalChoice],
    decision: ApprovalDecision,
    choice: Option<ApprovalChoiceId>,
) -> Result<&ApprovalChoice, ApprovalChoiceError> {
    if let Some(id) = choice {
        return choices
            .iter()
            .find(|option| option.id == id)
            .ok_or(ApprovalChoiceError::Stale);
    }
    let intent = match decision {
        ApprovalDecision::Approve => ApprovalIntent::Allow,
        ApprovalDecision::Reject => ApprovalIntent::Reject,
    };
    let mut matches = choices.iter().filter(|option| option.intent == intent);
    match (matches.next(), matches.next()) {
        (Some(option), None) => Ok(option),
        _ => Err(ApprovalChoiceError::Unrepresentable),
    }
}

/// Empty labels ask the surface for localized intent copy.
pub fn one_time_approval_choices() -> Vec<ApprovalChoice> {
    [ApprovalIntent::Reject, ApprovalIntent::Allow]
        .into_iter()
        .map(|intent| ApprovalChoice {
            id: ApprovalChoiceId::mint(),
            intent,
            label: String::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests;

/// Content-free failure marker. Older peers retain ProviderFailed semantics;
/// current surfaces recognize only this exact marker and localize guidance.
pub const APPROVAL_UNREPRESENTABLE_MESSAGE: &str = "approval_choices_unrepresentable";
