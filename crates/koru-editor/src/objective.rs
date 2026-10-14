// SPDX-License-Identifier: MPL-2.0

//! **The words on a row, as an objective** — one answer, two gestures.
//!
//! Both delegation gestures a document raises start from a line somebody
//! wrote: a selection's *Delegate…* takes the line it was raised on, and
//! `/delegate` takes what is left of the line once the command itself has been
//! erased. Neither is a second reading of the document, and both have to cut
//! at the same length, so the reading lives here rather than once in the view
//! and once in the editor.
//!
//! It was `apps/koru-document`'s private helper while only the right-click
//! path used it. `/delegate` picking the row's words up too made a second call
//! site, and two copies of "what does this row say" is exactly the pair that
//! drifts — one trimming, one not; one bounded, one not.

use koru_core::NodeId;
use koru_core::schema::WorkspaceDoc;

/// How much of a row may become an objective.
///
/// Bounded because it becomes a `koru_delegate_core::Objective`, which is
/// bounded, and a row that somehow exceeds it is cut on a character rather
/// than refused — the person can see the box and edit it before they press
/// anything.
///
/// The number is spelled here rather than linked: an editor has no business
/// depending on the delegation vocabulary to hand a container a string, and
/// this is the only fact it would have taken from it. It mirrors
/// `koru_delegate_core::MAX_OBJECTIVE_CHARS`; the modal validates on submit
/// either way, so a drift makes a long row shorter and never refuses a run.
pub const OBJECTIVE_CHARS: usize = 8_192;

/// The plain words of one row, trimmed and bounded.
///
/// The runs are joined and nothing else is kept — marks, pills and attributes
/// are how the row is *drawn*, not what the person asked for. A row that
/// cannot be read at all answers with nothing, which opens an empty box rather
/// than refusing the gesture.
pub fn row_words(ws: &WorkspaceDoc, node: NodeId) -> String {
    let Ok(runs) = ws.runs(node) else {
        return String::new();
    };
    let mut words = String::new();
    for run in &runs {
        words.push_str(&run.text);
    }
    let words = words.trim();
    match words.chars().count() > OBJECTIVE_CHARS {
        true => words.chars().take(OBJECTIVE_CHARS).collect(),
        false => words.to_owned(),
    }
}

#[cfg(test)]
mod tests;
