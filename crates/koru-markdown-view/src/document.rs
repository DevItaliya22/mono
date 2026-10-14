// SPDX-License-Identifier: MPL-2.0

//! **Markdown in, a document, Markdown out.** The round trip, in one place.
//!
//! A delegated agent's answer is a Markdown file, and so is the preamble it
//! was started with. Koru already knows how to turn Markdown into a document
//! (`koru-import-markdown`) and a document back into Markdown
//! (`koru-export-markdown`); what was missing is the small piece in the
//! middle that holds both ends at once, so that a surface can *show* a file
//! and *save* it without either half inventing a dialect of its own.
//!
//! ```text
//!   bytes ──import──▶ WorkspaceDoc ──export──▶ markdown ──parse──▶ blocks
//!                          ▲                      │                  │
//!                          └────── a save ────────┘             what is drawn
//! ```
//!
//! **The markdown a save writes is the markdown the view was drawn from.**
//! That is the reason the export happens on the way *in* rather than only on
//! the way out: what a person reads on screen is exactly what pressing Save
//! would put on disk, down to the escaping. A view rendered from the file as
//! typed and saved from the exporter would be a view that changes the file in
//! ways it never showed.
//!
//! **The export is a fixed point after one pass.** `export(import(x))` is not
//! always `x` — Koru's Markdown escapes punctuation the author may not have,
//! and a shape Koru has no node for (an HTML block, a letter-numbered list, an
//! image whose bytes are not here) is carried as text or as a comment. But
//! `export(import(export(import(x))))` *is* `export(import(x))`, which is what
//! makes the second save of an untouched document a no-op rather than a slow
//! drift. `crates/koru-markdown-view/src/tests/round_trip.rs` asserts both
//! halves.

use koru_core::node::BlobHash;
use koru_core::schema::WorkspaceDoc;
use koru_export::BlobSource;
use koru_import_markdown::Block;

/// The workspace a rendered document is nominally in.
///
/// It is nobody's workspace: nothing here is stored, synced or shown, and
/// the importer needs one only because a `WorkspaceDoc` carries an id. A
/// constant rather than a mint, so two renders of the same bytes are the
/// same document and a test can compare them.
const RENDERING_WORKSPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x4b6f_7275_0000_4000_8000_0000_0000_0001);

/// Why a Markdown document could not be made.
///
/// One value, deliberately: the importer's failures here are all *these
/// bytes are not a document Koru can build*, and a surface's honest answer
/// to every one of them is the same sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotADocument;

/// One Markdown file, as this surface holds it: the document Koru read out
/// of it, the Markdown that document exports to, and the blocks that
/// Markdown draws as.
#[derive(Clone)]
pub struct MarkdownDocument {
    /// **The document itself**, as the snapshot it was built from.
    ///
    /// Carried rather than dropped because a host with Koru's own document
    /// view can open it — `WorkspaceDoc::from_snapshot`, an in-memory
    /// workspace nobody owns, and the ordinary editor over it, read-only
    /// (ADR-372). Keeping it here is what stops that host having to parse
    /// the Markdown a second time on the window thread: the build already
    /// happened, on a worker, and this is its result.
    snapshot: Vec<u8>,
    markdown: String,
    blocks: Vec<Block>,
    /// **Whether the bytes this was built from are what it exports.**
    ///
    /// `export(import(x)) == x` for everything Koru has a node for, and not
    /// for everything else — an ordered list that did not start at 1, a hard
    /// line break, an HTML block, a footnote. Measured here, where the source
    /// is in hand and the worker is doing the work anyway, because the only
    /// honest moment to tell somebody that saving will rewrite lines they
    /// never touched is *before* they start typing. See
    /// [`is_faithful`](Self::is_faithful).
    faithful: bool,
}

impl PartialEq for MarkdownDocument {
    /// **The projection is the identity**, not the bytes behind it. Two
    /// renders of the same text produce the same Markdown and the same
    /// blocks; whether their Loro snapshots are byte-identical is a question
    /// about a CRDT's encoding, not about whether a surface is showing the
    /// same document.
    fn eq(&self, other: &Self) -> bool {
        self.markdown == other.markdown && self.blocks == other.blocks
    }
}

impl Eq for MarkdownDocument {}

impl std::fmt::Debug for MarkdownDocument {
    /// **A document is content.** Its text is a person's writing or a
    /// provider's payload, and standing requirement 7 keeps both out of every
    /// `Debug`, which is what reaches a log. What is printed is the shape:
    /// how many bytes, how many blocks.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownDocument")
            .field("bytes", &self.markdown.len())
            .field("blocks", &self.blocks.len())
            .field("faithful", &self.faithful)
            .finish()
    }
}

impl MarkdownDocument {
    /// Read Markdown as a Koru document.
    ///
    /// Pure and off-thread-safe: it touches no disk, opens no store and asks
    /// for no blob. An image the exporter cannot fetch becomes the comment
    /// the exporter already writes for one, which is the honest rendering of
    /// a picture whose bytes are somewhere else.
    pub fn of(text: &str) -> Result<Self, NotADocument> {
        let workspace = koru_core::WorkspaceId::from_uuid(RENDERING_WORKSPACE);
        let snapshot =
            koru_import_markdown::document_from_text(workspace, text).map_err(|_| NotADocument)?;
        let doc = WorkspaceDoc::from_snapshot(&snapshot).map_err(|_| NotADocument)?;
        Self::over(snapshot, &doc, Some(text))
    }

    /// The projection of a document that is already built.
    ///
    /// **The way back out.** A host that opened [`snapshot`](Self::snapshot)
    /// in Koru's own document view and let somebody edit it hands the edited
    /// `WorkspaceDoc` back here to get the Markdown a save writes — through
    /// the same exporter every other export goes through, so the page and the
    /// file cannot become two dialects.
    ///
    /// It is faithful by construction: there are no source bytes to be
    /// unfaithful to, because the document *is* the source.
    pub fn of_document(doc: &WorkspaceDoc) -> Result<Self, NotADocument> {
        let snapshot = doc.export_snapshot().map_err(|_| NotADocument)?;
        Self::over(snapshot, doc, None)
    }

    fn over(
        snapshot: Vec<u8>,
        doc: &WorkspaceDoc,
        source: Option<&str>,
    ) -> Result<Self, NotADocument> {
        let rendered = koru_export_markdown::render_document(doc, &NoBlobs, "./assets")
            .map_err(|_| NotADocument)?;
        let blocks = koru_import_markdown::parse(&rendered.markdown);
        Ok(Self {
            faithful: source.is_none_or(|source| source == rendered.markdown),
            snapshot,
            markdown: rendered.markdown,
            blocks,
        })
    }

    /// **The document, for a host that can open one.**
    ///
    /// A Loro snapshot of a workspace holding exactly this document and
    /// nothing else — no title line added and no heading moved, because
    /// `document_from_text` adds neither. It is nobody's workspace: it is
    /// not in the store, it is not synced, and a host that opens it opens a
    /// document that exists only for as long as the surface showing it.
    pub fn snapshot(&self) -> &[u8] {
        &self.snapshot
    }

    /// **What a save writes.** The document as Markdown, which is also what
    /// the blocks below were parsed from.
    pub fn markdown(&self) -> &str {
        &self.markdown
    }

    /// The blocks, in order. The view lays out the ones on screen and no
    /// others.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// A document with nothing in it — an empty file, or one holding only
    /// whitespace. It is a state a surface says out loud rather than an
    /// empty column.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// **Whether the bytes this was read from are the bytes it would write.**
    ///
    /// `false` means the source holds Markdown Koru spells another way, and a
    /// save would rewrite those lines whether or not anybody touched them.
    /// `src/tests/round_trip.rs` records exactly which constructs those are: an
    /// ordered list that does not start at `1`, a hard line break, an HTML
    /// block or a comment butted against a paragraph, a footnote or a `[1]`
    /// reference, a reference-style link definition, an `![image]`, setext
    /// headings, YAML frontmatter, indented code, and soft-wrapped lines.
    ///
    /// It is not a reason to refuse the edit — this is what a save has
    /// written since the surface existed, because the composer was always
    /// loaded from [`markdown`](Self::markdown) rather than from the source.
    /// It is a reason to **say so before the first keystroke**, which is what
    /// the result tab does with it (ADR-378).
    pub fn is_faithful(&self) -> bool {
        self.faithful
    }
}

/// A blob store that holds nothing.
///
/// This surface renders bytes it was handed; it has no workspace behind it
/// and must not acquire one to draw a picture. The exporter's answer to a
/// blob it cannot fetch is already the right one — `<!-- image omitted -->`
/// — so the honest implementation is the empty one.
struct NoBlobs;

impl BlobSource for NoBlobs {
    fn get(&self, _hash: &BlobHash) -> Result<Vec<u8>, koru_export::ExportError> {
        Err(koru_export::ExportError::new("no blob store here"))
    }
}

/// Whether a block is an HTML comment carried through as text.
///
/// **It is shown, and it is shown quietly.** The comment at the top of the
/// delegated-agent preamble is part of what the model receives — hiding it
/// would be a surface lying about what a run is given — but it is a note to
/// whoever edits the file rather than an instruction, and drawing it in the
/// same ink as the instructions makes the first screen of the tab read as
/// prose that is not. So it is dimmed: present, legible, and visibly not the
/// document's own voice.
pub fn is_comment(block: &Block) -> bool {
    let Block::Text(text) = block else {
        return false;
    };
    let trimmed = text.text.trim();
    trimmed.starts_with("<!--") && trimmed.ends_with("-->")
}
