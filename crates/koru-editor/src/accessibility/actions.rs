// SPDX-License-Identifier: MPL-2.0

//! **What a screen reader asked for, checked and handed to the editor action
//! that already performs it.**
//!
//! One sentence, and what it excludes: *this module decides whether a request
//! from an assistive technology is about a paragraph of this body, whether the
//! editor performs that action at all, and — for a text range — whether the
//! range names runs this view actually published; then it calls the same
//! [`Editing`] method a keystroke or a click reaches.* It excludes identity and
//! routing, which are `koru_workspace_host::accessibility`'s; it excludes what
//! is on top of what, which is `koru_workspace_host::routing`'s and the shell's;
//! and it excludes inventing a behaviour. **There is no editor here.** Every
//! arm below is one call into [`Editing`] or into [`Paragraph`], and if the
//! editor cannot already do it, the answer is [`Refused::Unsupported`].
//!
//! ## The three actions, and why there are only three
//!
//! | Action | What it is | Who performs it |
//! | --- | --- | --- |
//! | [`Action::Focus`] | put the keyboard on this paragraph | `Body::focus` and `ViewState::active` |
//! | [`Action::SetTextSelection`] | select exactly these bytes | [`Paragraph::select_range_now`] |
//! | [`Action::ScrollIntoView`] | bring an off-screen paragraph into the band | [`Editing::reveal`] |
//!
//! Anything else is refused, **including the two that would write** —
//! [`Action::SetValue`] and [`Action::ReplaceSelectedText`]. That is not an
//! oversight to be filled in later: it is the answer to §5's *read-only, stale
//! and unsupported targets cannot mutate*. The accessibility route has **no
//! write path into the document at all**, so "a read-only document cannot be
//! mutated through an AT request" is a property of the shape of this module
//! rather than of a flag somebody remembered to check. What an assistive
//! technology does to *edit* is select a range and then deliver real key
//! presses, which arrive at `Editing::key` like anybody else's.
//!
//! **This module does not know what read-only is**, and that is deliberate
//! rather than an omission. Nothing here writes, so there is nothing here for
//! read-only to stop. Whether a *selection* may be moved while a character
//! would not reach the body — a read-only document, an open document menu, a
//! focused find bar, an open overlay — is a question about what is on top of
//! what, and the shell asks it of the router that already owns it
//! (`koru_workspace_host::routing::EditableTarget`) before a request ever
//! arrives here. One answer, the same one the keyboard gives, in one place.
//!
//! Worth knowing while reading the above: `TextBody::read_only` is published
//! and, on Windows, **does not reach a client at all**. `accesskit_windows`
//! carries `IsReadOnly` only on `ValuePattern`, and offers that pattern only to
//! a node with a `value` — which a paragraph deliberately has none of, because a
//! `ValuePattern` also offers `SetValue`. Established by experiment; see
//! `guide/contributing/deps/ACCESSKIT.md`.
//!
//! ## Why `Focus` does not move the caret
//!
//! Windows sends a `Focus` request before **every** invoke, whatever the node
//! advertised (`guide/contributing/deps/ACCESSKIT.md`). A `Focus` that placed the caret at
//! the start of the paragraph would therefore destroy a selection every time a
//! client touched anything, which is exactly the "an AT truncated my selection
//! to a different range" failure §5 names. So focus moves `Body::focus` and
//! sets `ViewState::active` and does nothing else: the caret this paragraph
//! already has is the caret it keeps.
//!
//! ## A refusal is never a guess
//!
//! [`DocumentAccessibility::byte_range`] answers `None` for a range naming runs
//! this paragraph never published — a range held across a rebuild, or one
//! belonging to a different paragraph. This module turns that into
//! [`Refused::StaleRange`] and changes nothing. Clamping an unknown range onto
//! whatever is nearest is how an assistive technology ends up replacing text the
//! person never selected.

use accesskit::{Action, ActionData};
use koru_core::NodeId as KoruNodeId;

use super::projection::DocumentAccessibility;
use crate::Editing;

/// What the editor did, when it did something.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Performed {
    /// The keyboard is now on this paragraph. The caret did not move.
    Focused,
    /// The selection is now exactly these two byte offsets into the row's own
    /// text, snapped to grapheme boundaries the way every other caret is.
    Selected { anchor: usize, focus: usize },
    /// The view scrolled so this paragraph is in the band, and the caret went
    /// with it — [`Editing::reveal`]'s own behaviour, unchanged.
    Revealed,
}

/// Why nothing happened.
///
/// **A closed list, and every arm is a refusal a test names.** "Nothing
/// happened" with no reason is the shape of report this feature would otherwise
/// get.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The target names no paragraph of this body — a row that has gone, or a
    /// control belonging to something else entirely.
    NoSuchRow,
    /// The editor performs nothing for this action. The ordinary case, because
    /// `accesskit_windows` forwards a request whatever the node advertised.
    Unsupported,
    /// The action needs data and none of the right kind arrived.
    Malformed,
    /// The range named runs this paragraph never published. **Refused, never
    /// clamped.**
    StaleRange,
    /// The action would write the document, and no assistive-technology request
    /// writes a Koru document. See the module note.
    WouldWrite,
}

/// What one request came to.
pub type Outcome = Result<Performed, Refused>;

/// Do what a client asked about one paragraph, or say why not.
///
/// `row` is the model node id the target's occurrence named — the shell has
/// already resolved *which* control this is, and this module never parses a
/// locator.
///
/// **Runs on the loop, not on a platform callback.** A request arrives on
/// whichever thread UI Automation chose, is queued on the window's mailbox and
/// is drained by `PaneAccessibility::resolve`, so by the time this runs it is
/// an ordinary command on the ordinary thread. That is why
/// [`Action::ScrollIntoView`] may reach [`Editing::reveal`], which reflows: it
/// is the same reflow the same call makes when the explore column reveals a
/// row, and §5's rule is about a *semantic query*, which this is not.
pub fn perform<E: Editing + ?Sized>(
    editor: &mut E,
    access: &mut DocumentAccessibility,
    row: KoruNodeId,
    action: Action,
    data: Option<&ActionData>,
) -> Outcome {
    let outcome = route(editor, access, row, action, data);
    access.note_request(outcome.is_ok());
    outcome
}

fn route<E: Editing + ?Sized>(
    editor: &mut E,
    access: &mut DocumentAccessibility,
    row: KoruNodeId,
    action: Action,
    data: Option<&ActionData>,
) -> Outcome {
    // **Asked first, for every action.** A request naming a row that has left
    // the document is refused whatever it wanted, which is the same answer a
    // request naming a closed menu gets one layer up.
    let Some(index) = editor.body().index_of(row) else {
        return Err(Refused::NoSuchRow);
    };
    match action {
        Action::Focus => {
            editor.body_mut().focus = index;
            editor.view_mut().active = true;
            Ok(Performed::Focused)
        }
        Action::SetTextSelection => {
            let Some(ActionData::SetTextSelection(request)) = data else {
                return Err(Refused::Malformed);
            };
            let paragraph = &editor.body().rows[index];
            let Some((anchor, focus)) = access.byte_range(paragraph, request) else {
                return Err(Refused::StaleRange);
            };
            editor.body_mut().focus = index;
            editor.view_mut().active = true;
            // The same call a mouse drag ends with. `select_range_now` snaps
            // both ends onto grapheme boundaries against the layout that
            // exists, so an AT can no more land inside an emoji than a pointer
            // can.
            editor.body_mut().rows[index].select_range_now(anchor, focus);
            Ok(Performed::Selected { anchor, focus })
        }
        // The view's own scrolling, and nothing else: a paragraph outside
        // `reflow`'s band has no layout, so this is the only way it can acquire
        // one — and it acquires it the way the explore column's *reveal* does.
        Action::ScrollIntoView => {
            if editor.reveal(row) {
                Ok(Performed::Revealed)
            } else {
                Err(Refused::NoSuchRow)
            }
        }
        // **The two that would write.** Refused explicitly rather than falling
        // into `Unsupported`, so the reason a document cannot be edited through
        // an accessibility request is written down where somebody adding an
        // arm to this match will read it.
        Action::SetValue | Action::ReplaceSelectedText => Err(Refused::WouldWrite),
        _ => Err(Refused::Unsupported),
    }
}

#[cfg(test)]
mod tests;
