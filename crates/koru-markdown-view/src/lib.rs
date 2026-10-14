// SPDX-License-Identifier: MPL-2.0

//! **One Markdown surface** — a Markdown file read as a Koru document,
//! drawn the way Koru draws a document, edited, and written back through
//! the exporter.
//!
//! A delegated agent's answer is a Markdown file. So is the preamble it was
//! started with. Both were being shown as plain lines of text — the file's
//! own `#` and `-` and `**` on screen, exactly as an editor with no idea
//! what Markdown is would show them — while Koru's own document renderer sat
//! three crates away. This is the surface both of them now use.
//!
//! ## The round trip is the design
//!
//! ```text
//!   bytes ──import──▶ WorkspaceDoc ──export──▶ markdown ──parse──▶ blocks
//!                          ▲                      │                  │
//!                          └────── a save ────────┘             what is drawn
//! ```
//!
//! Every arrow is a crate that already existed. `koru-import-markdown` is
//! the parser file import and rich clipboard paste already share;
//! `koru-export-markdown` is the serializer the Export menu already writes
//! with; `koru-markdown` is the renderer the AI chat and the editor's
//! tables already draw with. **Nothing here decides what Markdown means or
//! what a heading looks like** — the reason there is a crate at all is the
//! piece in the middle, which is the projection: which blocks are on screen,
//! where they sit, and the buffer an edit is typed into.
//!
//! Because the export happens on the way *in*, what a person reads is
//! exactly what a save would write. A view rendered from the file as typed
//! but saved through the exporter would be a view that changes a file in
//! ways it never showed.
//!
//! ## What it costs
//!
//! Parsing is proportional to the file and happens **once, off the window
//! thread**, before the bytes reach [`MarkdownView`]. A frame is
//! proportional to what is on screen: at most [`BLOCKS_SHOWN`] blocks are
//! laid out, whatever the document's length, and the rest sit behind one
//! *show the rest* line. [`Work`] counts both so a test asserts the ratio.
//!
//! ## What it is not
//!
//! It is not an editor. There is no caret in the rendered document, no
//! block commands, no undo of a paragraph: an edit opens the document's
//! Markdown in a composer, and what is typed there is what is saved. The
//! editor is `koru-editor`, it works on a stored document, and this
//! surface deliberately cannot reach one — `src/tests/dependency.rs` is the
//! fence.
//!
//! It is also **not the only way to show what it built**. A host that *does*
//! have Koru's document view can take [`MarkdownDocument::snapshot`] — the
//! Loro snapshot this crate imported on its worker — open it in an in-memory
//! workspace nobody owns, and draw the version as a page: real bullets, real
//! heading sizes, real link pills. That is what the result tab does
//! (ADR-372), and it is why the snapshot is carried rather than dropped. The
//! surface here stays the one for a host that has no such view, for the
//! states that are not a document at all, and for the composer.
//!
//! It also owns no control and no worker. Both hosts lay out their own
//! chips and do their own saving, because *what a save means* is different
//! in each: the run views write a file beside the executables, and the
//! Artifacts column registers a **new version** of an immutable one
//! (ADR-344). What they share is the part that is the same, and it is here.

mod document;
pub mod text;
mod view;

#[cfg(test)]
mod tests;

pub use document::{MarkdownDocument, NotADocument, is_comment};
pub use view::{BLOCKS_SHOWN, Content, MarkdownView, Work};
