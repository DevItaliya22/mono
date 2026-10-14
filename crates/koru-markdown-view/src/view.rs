// SPDX-License-Identifier: MPL-2.0

//! **One layout pass, walked twice** — the shape every chrome surface in
//! Koru has, and for the same reason: a block cannot be drawn anywhere other
//! than where it was measured.
//!
//! ## What it costs to open a large document
//!
//! A document contributes at most [`BLOCKS_SHOWN`] laid-out blocks, whatever
//! its length, and the rest sit behind one *show the rest* line — the
//! roadmap's rule for P1.13 stated as arithmetic. Parsing is a different
//! thing and happens once, off the window thread, before the bytes ever
//! reach this type ([`MarkdownDocument::of`]); what a *frame* does is bounded
//! by what is on screen. [`Work`] counts it so a test asserts the ratio
//! rather than a wall clock.
//!
//! ## What this surface does not own
//!
//! It draws a document and holds the buffer an edit is typed into. It owns
//! no control, no worker and no idea where the bytes came from: the run
//! views know what a result is, the Artifacts column knows what a version
//! is, and each lays out its own chips and does its own saving. What they
//! share is here, and it is the part that is the same in both.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_i18n::Lang;
use koru_markdown::{Rendered, Styles};
use koru_ui::text_field::{TextArea, Typed};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;
use winit::keyboard::Key as WinitKey;

use crate::document::{MarkdownDocument, is_comment};
use crate::text;

/// How many blocks one page lays out. A screen's worth of a document:
/// enough that an agent's answer reads without a click, few enough that
/// opening a ten-thousand-block file costs the same as opening a ten-block
/// one.
pub const BLOCKS_SHOWN: usize = 60;

/// Air between two blocks.
const BLOCK_GAP: f32 = 8.0;
/// Air under a state sentence.
const LINE_GAP: f32 = 4.0;
/// A horizontal rule's own height, air included.
const RULE_H: f32 = 17.0;
/// The shortest a line of anything is.
const LINE_H: f32 = 20.0;
/// How tall the composer is allowed to grow. Beyond this the host's own
/// scroll carries it, and the composer scrolls inside itself.
const MAX_EDIT_LINES: usize = 4_000;

/// What one layout pass built, so a test can assert a ratio rather than a
/// wall clock (standing requirement 3).
///
/// `blocks` is how many of the document's blocks were laid out; `layouts` is
/// how many text layouts that took. Both are per call, reset at the start of
/// each: what matters is the cost of one open, not the total since the
/// window came up.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct Work {
    pub blocks: usize,
    pub layouts: usize,
}

/// What the surface has been given to show.
///
/// **Five states and not two.** Bytes that have not arrived, a document,
/// bytes that are not a document, bytes that are on another device, and
/// bytes that would not read — each is a different fact about the person's
/// work, and collapsing them is how a surface ends up saying *this is empty*
/// about something it never fetched.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum Content {
    /// Asked for; nothing back yet.
    #[default]
    Reading,
    /// A Markdown document, ready to draw.
    Document(Box<MarkdownDocument>),
    /// Not Markdown. **Named, never drawn** — what it is and how big, which
    /// is the honest thing to say about bytes this surface has no way to
    /// render.
    NotMarkdown {
        mime: String,
        size_bytes: u64,
        filename: Option<String>,
    },
    /// D27: the bytes are on another device. Named when the device can be.
    Elsewhere { device: Option<String> },
    /// The bytes are here and are not a document Koru could build.
    Unreadable,
}

impl std::fmt::Debug for Content {
    /// **What is in a document, and what a file was called, are content**
    /// (standing requirement 7): a filename sits beside a title, and a
    /// document's text is somebody's writing. Both stay out of `Debug`,
    /// which is what reaches a log. The *state* is exactly what a reader of
    /// one needs, and the media type is safe to name — `application/pdf`
    /// says nothing about a person.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reading => f.write_str("Reading"),
            Self::Unreadable => f.write_str("Unreadable"),
            Self::Document(document) => f.debug_tuple("Document").field(document).finish(),
            Self::Elsewhere { device } => f
                .debug_struct("Elsewhere")
                .field("named", &device.is_some())
                .finish(),
            Self::NotMarkdown {
                mime, size_bytes, ..
            } => f
                .debug_struct("NotMarkdown")
                .field("mime", mime)
                .field("size_bytes", size_bytes)
                .finish(),
        }
    }
}

/// Which ink a piece is drawn in.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Tone {
    /// The document's own voice.
    Body,
    /// A note about the document rather than part of it — a state sentence,
    /// and an HTML comment carried through as text.
    Quiet,
    /// A label beside a value.
    Meta,
}

impl Tone {
    fn color(self) -> Color {
        let p = theme::p();
        match self {
            Self::Body => p.text,
            Self::Quiet => p.text_dim,
            Self::Meta => p.row_hint,
        }
    }
}

/// One measured thing, at a known place.
enum Piece {
    Text {
        y: f32,
        text: String,
        layout: parley::Layout<theme::Brush>,
        tone: Tone,
    },
    Table {
        y: f32,
        grid: koru_markdown::table::TableGrid,
    },
    Rule {
        y: f32,
    },
    /// The composer, when an edit is open.
    Editor {
        y: f32,
        h: f32,
    },
}

/// A Markdown file, drawn the way Koru draws a document.
pub struct MarkdownView {
    lang: Lang,
    content: Content,
    /// How many of the document's blocks are laid out.
    shown: usize,
    /// The composer, present exactly while an edit is open.
    editor: Option<TextArea>,
    /// The markdown the open edit started from, so a changed one can be told
    /// from an untouched one without re-exporting.
    baseline: String,
    pieces: Vec<Piece>,
    width: f32,
    bottom: f32,
    work: Work,
    stale: bool,
}

impl Default for MarkdownView {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownView {
    pub fn new() -> Self {
        Self {
            lang: Lang::default(),
            content: Content::Reading,
            shown: BLOCKS_SHOWN,
            editor: None,
            baseline: String::new(),
            pieces: Vec::new(),
            width: 0.0,
            bottom: 0.0,
            work: Work::default(),
            stale: true,
        }
    }

    // ------------------------------------------------------- what it knows

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn set_lang(&mut self, lang: Lang) -> bool {
        if self.lang == lang {
            return false;
        }
        self.lang = lang;
        self.stale = true;
        true
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    /// Replace what is shown. An open edit is dropped: the bytes under it
    /// have changed, and keeping the composer would let a save write an
    /// edit of something that is no longer there.
    pub fn set_content(&mut self, content: Content) {
        self.content = content;
        self.shown = BLOCKS_SHOWN;
        self.editor = None;
        self.baseline.clear();
        self.stale = true;
    }

    pub fn document(&self) -> Option<&MarkdownDocument> {
        match &self.content {
            Content::Document(document) => Some(document),
            _ => None,
        }
    }

    /// Whether there is a document here at all — which is the whole of
    /// whether editing and saving can be offered.
    pub fn is_document(&self) -> bool {
        self.document().is_some()
    }

    pub fn work(&self) -> Work {
        self.work
    }

    /// How many blocks have not been laid out yet.
    pub fn remaining(&self) -> usize {
        self.document()
            .map(|document| document.blocks().len().saturating_sub(self.shown))
            .unwrap_or(0)
    }

    /// Lay out one more page.
    pub fn reveal_more(&mut self) {
        self.shown = self.shown.saturating_add(BLOCKS_SHOWN);
        self.stale = true;
    }

    // -------------------------------------------------------------- editing

    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// Open the composer on the document's own Markdown — the same text the
    /// blocks above were drawn from, so what is typed over is what was read.
    pub fn begin_edit(&mut self, engine: &mut TextEngine) -> bool {
        let Some(document) = self.document() else {
            return false;
        };
        let markdown = document.markdown().to_owned();
        let mut area = TextArea::new(engine, theme::role::row());
        area.set_text(engine, &markdown);
        // What the composer holds, not what was handed to it: the two differ
        // by whatever the composer normalises, and a Save offered because of
        // *that* would rewrite a file nobody typed in.
        self.baseline = area.raw_text().to_owned();
        self.editor = Some(area);
        self.stale = true;
        true
    }

    /// Close the composer and throw the edit away.
    pub fn cancel_edit(&mut self) -> bool {
        let had = self.editor.take().is_some();
        self.baseline.clear();
        self.stale = true;
        had
    }

    /// The text in the composer — what a save writes.
    pub fn edited(&self) -> Option<String> {
        self.editor.as_ref().map(|area| area.raw_text().to_owned())
    }

    /// Whether the open edit differs from what it started as. A Save that is
    /// offered when nothing has been typed is a Save that rewrites a file for
    /// no reason.
    pub fn changed(&self) -> bool {
        self.edited().is_some_and(|edited| edited != self.baseline)
    }

    /// The save landed: adopt the saved text as the document and close the
    /// composer, so what is on screen is what is now stored.
    pub fn accept_saved(&mut self, saved: MarkdownDocument) {
        self.editor = None;
        self.baseline.clear();
        self.content = Content::Document(Box::new(saved));
        self.shown = BLOCKS_SHOWN;
        self.stale = true;
    }

    // ------------------------------------------------------------ the pass

    /// Lay the whole surface out at this width, if anything has moved.
    ///
    /// Only the shown blocks are measured. Everything a draw or a hit test
    /// needs afterwards comes out of [`Self::pieces`].
    pub fn relayout(&mut self, engine: &mut TextEngine, width: f32) {
        let width = width.max(1.0);
        if !self.stale && (self.width - width).abs() < 0.5 {
            return;
        }
        self.width = width;
        self.stale = false;
        self.work = Work::default();
        self.pieces.clear();
        let mut y = 0.0;
        // Taken out and put back rather than borrowed: every arm below
        // measures, and measuring is `&mut self`.
        let content = std::mem::replace(&mut self.content, Content::Reading);
        match &content {
            Content::Reading => {
                let text = text::reading(self.lang).to_owned();
                self.line(engine, &mut y, text, Tone::Quiet);
            }
            Content::Unreadable => {
                let text = text::unreadable(self.lang).to_owned();
                self.line(engine, &mut y, text, Tone::Quiet);
            }
            Content::Elsewhere { device } => {
                let text = text::held_elsewhere(self.lang, device.as_deref());
                self.line(engine, &mut y, text, Tone::Quiet);
            }
            Content::NotMarkdown {
                mime,
                size_bytes,
                filename,
            } => {
                let lang = self.lang;
                let head = text::not_markdown(lang).to_owned();
                self.line(engine, &mut y, head, Tone::Quiet);
                let kind = format!("{}: {mime}", text::type_label(lang));
                self.line(engine, &mut y, kind, Tone::Meta);
                let size = format!(
                    "{}: {}",
                    text::size_label(lang),
                    text::size(lang, *size_bytes)
                );
                self.line(engine, &mut y, size, Tone::Meta);
                if let Some(name) = filename {
                    let named = format!("{}: {name}", text::file_label(lang));
                    self.line(engine, &mut y, named, Tone::Meta);
                }
            }
            Content::Document(document) => {
                if document.is_empty() && self.editor.is_none() {
                    let text = text::empty_document(self.lang).to_owned();
                    self.line(engine, &mut y, text, Tone::Quiet);
                } else {
                    self.document_pieces(engine, &mut y, document);
                }
            }
        }
        self.content = content;
        self.bottom = y;
    }

    fn document_pieces(
        &mut self,
        engine: &mut TextEngine,
        y: &mut f32,
        document: &MarkdownDocument,
    ) {
        if let Some(mut editor) = self.editor.take() {
            let note = text::editing(self.lang).to_owned();
            self.line(engine, y, note, Tone::Quiet);
            let h = editor.desired_height(
                engine,
                (self.width - 16.0).max(1.0),
                LINE_H,
                6,
                MAX_EDIT_LINES,
            );
            self.pieces.push(Piece::Editor { y: *y, h });
            *y += h + BLOCK_GAP;
            self.editor = Some(editor);
            return;
        }
        let width = self.width;
        let styles = Styles::document();
        for block in document.blocks().iter().take(self.shown) {
            self.work.blocks += 1;
            let tone = if is_comment(block) {
                Tone::Quiet
            } else {
                Tone::Body
            };
            match koru_markdown::render_parsed(engine, block.clone(), width, &styles) {
                Rendered::Text {
                    text,
                    layout,
                    links: _,
                } => {
                    self.work.layouts += 1;
                    let h = layout.height().max(LINE_H);
                    self.pieces.push(Piece::Text {
                        y: *y,
                        text,
                        layout,
                        tone,
                    });
                    *y += h + BLOCK_GAP;
                }
                Rendered::Table(grid) => {
                    self.work.layouts += grid.rows.iter().map(Vec::len).sum::<usize>();
                    let h = grid.height;
                    self.pieces.push(Piece::Table { y: *y, grid });
                    *y += h + BLOCK_GAP;
                }
                Rendered::Image { url, alt } => {
                    // **A picture is named, not fetched.** Nothing here
                    // reaches the network or the blob store, so what an
                    // image gets is the honest line: what it was called and
                    // where it points.
                    let shown = if alt.trim().is_empty() { url } else { alt };
                    self.line(engine, y, shown, Tone::Quiet);
                }
                Rendered::Rule => {
                    self.pieces.push(Piece::Rule { y: *y });
                    *y += RULE_H;
                }
            }
        }
    }

    fn line(&mut self, engine: &mut TextEngine, y: &mut f32, text: String, tone: Tone) {
        let layout = engine.simple_layout_wrapped(&text, theme::role::meta(), self.width);
        self.work.layouts += 1;
        let h = layout.height().max(LINE_H);
        self.pieces.push(Piece::Text {
            y: *y,
            text,
            layout,
            tone,
        });
        *y += h + LINE_GAP;
    }

    /// How tall the surface is, at the width it was last laid out for.
    pub fn height(&self) -> f32 {
        self.bottom
    }

    /// Every line of text on screen, in order — what a test reads a document
    /// out of.
    pub fn lines(&self) -> Vec<String> {
        self.pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    // --------------------------------------------------------------- paint

    /// Draw everything at `(x, y)`. Nothing is measured here: a piece that
    /// was not laid out is not drawn.
    pub fn draw(
        &mut self,
        scene_out: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        x: f32,
        y: f32,
    ) {
        let p = theme::p();
        let mut editor = self.editor.take();
        for piece in &self.pieces {
            match piece {
                Piece::Text {
                    layout,
                    tone,
                    y: py,
                    ..
                } => scene::draw_layout(scene_out, root, layout, x, y + py, Some(tone.color())),
                Piece::Table { grid, y: py } => {
                    koru_markdown::table::draw_grid(scene_out, root, grid, x, y + py, 0.0, None)
                }
                Piece::Rule { y: py } => scene::fill(
                    scene_out,
                    root,
                    p.panel_border,
                    &scene::rect(x, y + py + RULE_H * 0.5, self.width, theme::BORDER_W as f32),
                ),
                Piece::Editor { y: py, h } => {
                    if let Some(area) = editor.as_mut() {
                        area.draw(
                            scene_out,
                            root,
                            engine,
                            Rect::new(x, y + py, self.width, *h),
                            None,
                            true,
                        );
                    }
                }
            }
        }
        self.editor = editor;
    }

    /// The composer's rect in the caller's own coordinates, when one is
    /// open. The host needs it to route a click; nothing else does.
    fn editor_rect(&self, x: f32, y: f32) -> Option<Rect> {
        self.pieces.iter().find_map(|piece| match piece {
            Piece::Editor { y: py, h } => Some(Rect::new(x, y + py, self.width, *h)),
            _ => None,
        })
    }

    // --------------------------------------------------------------- input

    /// A press, in the caller's coordinates, with the surface's origin.
    /// `true` when it landed on the composer.
    pub fn mouse_down(
        &mut self,
        engine: &mut TextEngine,
        origin: (f32, f32),
        px: f32,
        py: f32,
    ) -> bool {
        let Some(rect) = self.editor_rect(origin.0, origin.1) else {
            return false;
        };
        let Some(area) = self.editor.as_mut() else {
            return false;
        };
        area.click(engine, rect, px, py)
    }

    pub fn mouse_up(&mut self) {
        if let Some(area) = self.editor.as_mut() {
            area.release();
        }
    }

    /// A key, while the composer holds it. `false` when nothing is open, so
    /// the host's own keyboard is untouched by a surface that is only being
    /// read.
    pub fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &WinitKey,
        typed: Option<&str>,
        mods: winit::keyboard::ModifiersState,
    ) -> bool {
        let Some(area) = self.editor.as_mut() else {
            return false;
        };
        let handled = area.key(engine, key, typed, mods);
        self.stale = true;
        matches!(handled, Typed::Handled)
    }
}
