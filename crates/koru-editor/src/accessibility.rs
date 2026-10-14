// SPDX-License-Identifier: MPL-2.0

//! **What a document's paragraphs are, said in AccessKit's vocabulary.**
//!
//! One sentence, and what it excludes: *this module turns laid-out
//! [`Paragraph`](crate::Paragraph)s into the ordered text runs, positions and
//! selection an assistive technology reads, tells its caller what changed since
//! last time, and turns an AT's request to select text back into byte offsets
//! the existing editor already understands.* It excludes the platform — when a
//! window has a provider, what a `WM_GETOBJECT` is — which is
//! `koru_window::accessibility`'s. It excludes identity, tree publishing and
//! action routing, which are `koru_workspace_host::accessibility`'s. And it
//! excludes *editing*: nothing here writes to a document. It hands back byte
//! offsets, and the caller drives the same `Paragraph` methods a keystroke
//! does.
//!
//! ## Five files, five subjects
//!
//! Plan 1b.1 wrote one file, because one paragraph at a time was the whole
//! subject. A document that is read, edited and edited *by somebody else* is
//! five, and they fail in different ways:
//!
//! - [`text_positions`] — **the arithmetic between five ways of counting one
//!   string.** Bytes, unicode scalars, parley's shaping clusters, Koru's
//!   grapheme clusters and UTF-16 code units are not interchangeable, and the
//!   two corrections that make them agree ([`merge_to_graphemes`] and
//!   [`trim_strut`]) live there. Its failures are off-by-one, and silent.
//! - [`geometry`] — **layout coordinates to window coordinates, and no
//!   further.** It applies no scale: that conversion has exactly one owner, and
//!   it is not this crate. Its failures only appear on a display nobody
//!   develops on.
//! - [`projection`] — **ordered paragraphs, and what changed.** The per-row
//!   cache, keyed on the shaping stamp `rowlayout::build_row` leaves behind, so
//!   an unchanged paragraph costs a `u64` comparison rather than an
//!   O(clusters) `build_nodes`. Its failures are a slow editor.
//! - [`actions`] — **a request, checked, and handed to the editor action that
//!   already performs it.** Three actions and no fourth; nothing here writes a
//!   document. Its failures are an assistive technology selecting text the
//!   person did not.
//! - [`events`] — **whether an update may be published, and what its arrival
//!   will make the platform say.** Its failure is an abort inside a UI
//!   Automation callback with no backtrace.
//!
//! ## The two traps, in one place so they are not rediscovered
//!
//! **A parley cluster is not a Koru character.** Verified rather than assumed:
//! parley shapes `a👍🏻e\u{301}b` as clusters of `1, 4, 4, 1, 2, 1` bytes — the
//! skin-tone modifier and the combining acute are clusters of their own — while
//! `Paragraph::after` steps `1, 8, 3, 1`, because Koru's Left and Right move by
//! *extended grapheme cluster* on purpose (landing the caret inside a ZWJ
//! sequence and typing splits it into tofu). AccessKit's own documentation says
//! the character lengths must come from the text editor rather than from the
//! text, and this is exactly why. So [`merge_to_graphemes`] folds parley's
//! clusters back into the unit the arrow keys move by, and an AT can never
//! report — or ask for — a caret the editor could not have put there.
//!
//! **The layout strut is not in the text.** `build_row` shapes
//! `row.text + LAYOUT_STRUT`, a trailing `U+200B` that pins every line's
//! descent (see [`crate::rowlayout`] and `guide/contributing/deps/PARLEY.md`). It must not
//! reach a screen reader, or every paragraph ends in an invisible character
//! that `GetText` returns, `Move` steps over and a Braille display shows. It is
//! pushed with a brush of its own, so parley gives it a text run of its own;
//! [`trim_strut`] *empties* that run rather than dropping it, which keeps the
//! end-of-text position addressable and gives an empty paragraph the one run
//! AccessKit needs before it will answer a text pattern at all.
//!
//! ## What is parley's, and what is Koru's
//!
//! parley 0.11 ships `LayoutAccessibility::build_nodes`, and it is reused
//! rather than reimplemented: it owns run splitting (per line, per style), bidi
//! ordering within a line, per-cluster geometry, word boundaries, font
//! attributes, stable ids across passes, and the 256-character cap that
//! `accesskit::Node::word_starts` being a `Box<[u8]>` forces on a run (see
//! [`MAX_WORD_STARTS_INDEX`]). Writing that again would be a second place to
//! get shaping wrong.
//!
//! What is Koru's is the two corrections above, the position map, the
//! coordinate transform, and *knowing when none of it has to run again*.

pub mod actions;
pub mod events;
pub mod geometry;
pub mod projection;
pub mod text_positions;

pub use actions::{Outcome, Performed, Refused};
pub use events::{Effects, Incoherent};
pub use projection::{
    BodyUpdate, Counters, DocumentAccessibility, InlineObject, ParagraphText, Publish, RowFacts,
    RowText, RowUpdate, ViewGeometry,
};
pub use text_positions::{MAX_WORD_STARTS_INDEX, merge_to_graphemes, trim_strut};

#[cfg(test)]
mod tests;
