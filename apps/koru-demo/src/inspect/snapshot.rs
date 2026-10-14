// SPDX-License-Identifier: MPL-2.0

//! Content crosses the read boundary only through this explicitly requested type.
use super::{ElementFacts, InspectError};
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct ContentSnapshot {
    pub facts: ElementFacts,
    pub text: String,
    pub truncated: bool,
}

impl fmt::Debug for ContentSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentSnapshot")
            .field("facts", &self.facts)
            .field("text", &"[redacted]")
            .field("truncated", &self.truncated)
            .finish()
    }
}

impl ContentSnapshot {
    /// Bounds retained output, not the provider's allocation: AccessKit ignores
    /// GetText(maxLength). That call still belongs on the supervised worker.
    pub fn bounded(
        facts: ElementFacts,
        mut text: String,
        max_utf16: u32,
    ) -> Result<Self, InspectError> {
        if !(1..=16384).contains(&max_utf16) {
            return Err(InspectError::Unavailable("snapshot-limit-invalid".into()));
        }
        let mut units = 0;
        let cut = text.char_indices().find_map(|(offset, ch)| {
            units += ch.len_utf16() as u32;
            (units > max_utf16).then_some(offset)
        });
        if let Some(cut) = cut {
            text.truncate(cut);
        }
        Ok(Self {
            facts,
            text,
            truncated: cut.is_some(),
        })
    }
}

#[cfg(test)]
mod tests;
