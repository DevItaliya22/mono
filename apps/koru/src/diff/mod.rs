// SPDX-License-Identifier: MPL-2.0

//! The version-compare view: a read-only page over a
//! [`koru_history::unified::UnifiedDiff`].
//!
//! Data arrives from the history worker (`mainview::history`) already diffed
//! and in reading order; this file only lays rows out and draws them. Layout
//! runs once per comparison or width change, never per frame, and nothing
//! here touches the store or the document.

use koru_draw::geometry::Rect;
use koru_draw::text::{RangeStyle, TextEngine};
use koru_draw::theme::{self, Brush};
use koru_draw::{icons, scene};
use koru_history::DiffTag;
use koru_history::unified::{DiffRow, UnifiedDiff};
use koru_i18n::Key;
use koru_ui::scrollbar::{self, ScrollbarGeom};
use koru_workspace_host::View;
use parley::Layout;
use vello::Scene;
use vello::kurbo::Affine;
use winit::event::MouseScrollDelta;
use winit::window::CursorIcon;

/// A finished comparison, produced on the history worker and handed to this
/// view whole. `from` is always the older side (decided at enqueue time), so
/// the header never reads backwards.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub diff: UnifiedDiff,
    pub from_recorded_at_ms: i64,
    pub to_recorded_at_ms: i64,
    pub from_reason: Key,
    pub to_reason: Key,
    /// The names the person gave the endpoints, when they gave any — shown in
    /// the header in place of the reasons, the same rule the panel's rows
    /// follow.
    pub from_label: Option<String>,
    pub to_label: Option<String>,
}

/// The page's own margin, matching the Trash page's sheet feel.
const PAD: f32 = 40.0;
/// Vertical air between rows.
const ROW_GAP: f32 = 6.0;
/// How far one level of tree depth indents a row.
const INDENT: f32 = 20.0;
/// Horizontal overshoot of a row's tint band past its text.
const BAND_PAD_X: f32 = 8.0;
/// Vertical overshoot of a row's tint band.
const BAND_PAD_Y: f32 = 2.0;
/// Between the header block and the first row.
const HEADER_GAP: f32 = 24.0;
/// Raw wght for heading blocks, the document heading ramp's neighbourhood.
const HEADING_WEIGHT: f32 = 650.0;

/// One laid-out row: everything paint needs, nothing layout needs twice.
struct LaidRow {
    y: f32,
    h: f32,
    indent: f32,
    layout: Layout<Brush>,
    /// Whole-row tint behind the text (an added or removed block).
    band: Option<vello::peniko::Color>,
    /// Word-level highlight rects, as byte ranges into the layout's text.
    highlights: Vec<(std::ops::Range<usize>, vello::peniko::Color)>,
}

pub struct DiffView {
    viewport: (f32, f32),
    engine: TextEngine,
    comparison: Option<Comparison>,
    lang: koru_i18n::Lang,
    /// The source document's page layout — Page, Narrow or Full — so the diff
    /// reads at the same measure as the document it is about (owner decision,
    /// 2026-08-31). Rows are laid out column-relative; paint translates by
    /// [`origin_x`](Self::origin_x), the same split the editor makes.
    view_mode: theme::ViewMode,
    /// The source document's canvas tint, for Page mode's surround.
    canvas_bg: Option<[u8; 4]>,
    rows: Vec<LaidRow>,
    /// The column width the rows were last laid out at; a mismatch marks them
    /// stale (the viewport and the mode both feed it).
    laid_w: f32,
    content_h: f32,
    scroll: f32,
    /// Pointer offset into the thumb while it is being dragged.
    scroll_drag: Option<f32>,
    dirty: bool,
}

impl DiffView {
    pub fn new(lang: koru_i18n::Lang) -> Self {
        Self {
            viewport: (900.0, 600.0),
            engine: TextEngine::new(),
            comparison: None,
            lang,
            view_mode: theme::default_view_mode(),
            canvas_bg: None,
            rows: Vec::new(),
            laid_w: 0.0,
            content_h: 0.0,
            scroll: 0.0,
            scroll_drag: None,
            dirty: true,
        }
    }

    /// Whether the scrollbar thumb is being dragged — the one drag this view
    /// owns, reported so the container keeps forwarding pointer moves.
    pub fn is_dragging(&self) -> bool {
        self.scroll_drag.is_some()
    }

    /// Adopt the compared document's page layout and canvas, captured by the
    /// container at open time from the tab the compare was asked in.
    pub fn set_source_style(&mut self, mode: theme::ViewMode, canvas_bg: Option<[u8; 4]>) {
        if self.view_mode == mode && self.canvas_bg == canvas_bg {
            return;
        }
        self.view_mode = mode;
        self.canvas_bg = canvas_bg;
        self.dirty = true;
    }

    /// Replace what the view shows. The next draw lays it out.
    pub fn set_comparison(&mut self, comparison: Comparison) {
        if self.comparison.as_ref() == Some(&comparison) {
            return;
        }
        self.comparison = Some(comparison);
        self.scroll = 0.0;
        self.dirty = true;
    }

    fn t(&self, key: Key) -> &'static str {
        koru_i18n::t(self.lang, key)
    }

    /// The content column's width under the current mode.
    fn column_w(&self) -> f32 {
        self.view_mode.effective_w(self.viewport.0)
    }

    /// Where the column starts — zero in Full, centred in Page/Narrow.
    fn origin_x(&self) -> f32 {
        self.view_mode.content_x(self.viewport.0)
    }

    fn wrap_width(&self) -> f32 {
        (self.column_w() - 2.0 * PAD).max(120.0)
    }

    fn max_scroll(&self) -> f32 {
        (self.content_h - self.viewport.1).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn scrollbar(&self) -> Option<ScrollbarGeom> {
        let (w, h) = self.viewport;
        ScrollbarGeom::for_content(Rect::new(0.0, 0.0, w, h), self.content_h, self.scroll)
    }

    /// Lay every row out at the current width. Runs when the comparison or
    /// the width changed — the draw path only reads the cache.
    fn rebuild(&mut self) {
        self.rows.clear();
        self.dirty = false;
        self.laid_w = self.column_w();
        let Some(comparison) = self.comparison.take() else {
            self.content_h = 0.0;
            return;
        };
        let wrap = self.wrap_width();
        let mut y = PAD;

        // Header: what is being compared, and the one-line tally.
        let title = self
            .engine
            .simple_layout(self.t(Key::DiffTabTitle), theme::role::section_heading());
        y = push_plain(&mut self.rows, y, PAD, title, 4.0);
        // A name the person gave the endpoint outranks the reason it was
        // taken — the rule the panel's rows already follow.
        let endpoint = |label: &Option<String>, reason: Key, ms: i64| {
            format!(
                "{} · {}",
                label
                    .as_deref()
                    .filter(|label| !label.is_empty())
                    .unwrap_or(koru_i18n::t(self.lang, reason)),
                crate::format::absolute_local(self.lang, ms)
            )
        };
        let subtitle = format!(
            "{} → {}",
            endpoint(
                &comparison.from_label,
                comparison.from_reason,
                comparison.from_recorded_at_ms
            ),
            endpoint(
                &comparison.to_label,
                comparison.to_reason,
                comparison.to_recorded_at_ms
            ),
        );
        let subtitle = self.engine.simple_layout(&subtitle, theme::role::ui());
        y = push_plain(&mut self.rows, y, PAD, subtitle, 2.0);
        let stats = comparison.diff.stats;
        let changed = stats.added + stats.removed + stats.modified + stats.moved + stats.reordered;
        let summary = koru_i18n::fill(
            self.t(Key::DiffSummary),
            &[&changed.to_string(), &stats.unchanged.to_string()],
        );
        let summary = self.engine.simple_layout(&summary, theme::role::hint());
        y = push_plain(&mut self.rows, y, PAD, summary, HEADER_GAP);

        let green = theme::accent_tone("green");
        let red = theme::accent_tone("red");
        let body = theme::role::body();

        for row in &comparison.diff.rows {
            y = match row {
                DiffRow::Context(block) => self.push_block(y, wrap, block, None, &[], None),
                DiffRow::Added(block) => self.push_block(
                    y,
                    wrap,
                    block,
                    green.map(|t| t.fill),
                    &[],
                    green.map(|t| t.ink),
                ),
                DiffRow::Removed(block) => {
                    let text = block_text(block);
                    let strike = [RangeStyle {
                        range: 0..text.len(),
                        color: None,
                        strikethrough: true,
                    }];
                    self.push_block(
                        y,
                        wrap,
                        block,
                        red.map(|t| t.fill),
                        &strike,
                        red.map(|t| t.ink),
                    )
                }
                DiffRow::Skipped { blocks } => {
                    let text = koru_i18n::fill(self.t(Key::DiffSkipped), &[&blocks.to_string()]);
                    let layout = self.engine.simple_layout(&text, theme::role::hint());
                    push_plain(&mut self.rows, y, PAD, layout, ROW_GAP)
                }
                DiffRow::Moved { block } => self.push_marker(y, wrap, Key::DiffMoved, block),
                DiffRow::Reordered { block } => {
                    self.push_marker(y, wrap, Key::DiffReordered, block)
                }
                DiffRow::Modified {
                    block,
                    spans,
                    kind_changed,
                    metadata_changed,
                    marks_changed,
                } => {
                    let mut text = String::new();
                    let mut styles: Vec<RangeStyle> = Vec::new();
                    let mut highlights = Vec::new();
                    push_prefix(&mut text, block);
                    for span in spans {
                        let start = text.len();
                        text.push_str(&span.text);
                        let range = start..text.len();
                        match span.tag {
                            DiffTag::Equal => {}
                            DiffTag::Insert => {
                                if let Some(tone) = green {
                                    highlights.push((range.clone(), tone.fill));
                                    styles.push(RangeStyle {
                                        range,
                                        color: Some(tone.ink),
                                        strikethrough: false,
                                    });
                                }
                            }
                            DiffTag::Delete => {
                                if let Some(tone) = red {
                                    highlights.push((range.clone(), tone.fill));
                                    styles.push(RangeStyle {
                                        range,
                                        color: Some(tone.ink),
                                        strikethrough: true,
                                    });
                                }
                            }
                        }
                    }
                    // A change with no visible words — a checked-off task, a
                    // kind switch, new marks — still has to say something.
                    let visible = spans.iter().any(|s| s.tag != DiffTag::Equal);
                    if !visible && (*kind_changed || *metadata_changed || *marks_changed) {
                        let start = text.len();
                        text.push_str(" · ");
                        text.push_str(self.t(Key::DiffPropertiesChanged));
                        styles.push(RangeStyle {
                            range: start..text.len(),
                            color: Some(theme::p().text_dim),
                            strikethrough: false,
                        });
                    }
                    let indent = indent_of(block.depth);
                    let layout = self.engine.styled_layout(
                        &text,
                        body.metrics,
                        weight_of(block),
                        wrap - (indent - PAD),
                        None,
                        &styles,
                    );
                    push_row(&mut self.rows, y, indent, layout, None, highlights)
                }
            };
        }
        self.content_h = y + PAD;
        self.comparison = Some(comparison);
        self.clamp_scroll();
    }

    /// One block's text as a full-width row: tint band, optional ink override,
    /// optional range styles.
    fn push_block(
        &mut self,
        y: f32,
        wrap: f32,
        block: &koru_history::unified::BlockView,
        band: Option<vello::peniko::Color>,
        styles: &[RangeStyle],
        ink: Option<vello::peniko::Color>,
    ) -> f32 {
        let mut text = String::new();
        push_prefix(&mut text, block);
        let prefix = text.len();
        text.push_str(&block.text);
        // Range styles from the caller are relative to the bare text; shift
        // them past the prefix.
        let styles: Vec<RangeStyle> = styles
            .iter()
            .map(|s| RangeStyle {
                range: s.range.start + prefix..s.range.end + prefix,
                color: s.color,
                strikethrough: s.strikethrough,
            })
            .collect();
        let indent = indent_of(block.depth);
        let layout = self.engine.styled_layout(
            &text,
            theme::role::body().metrics,
            weight_of(block),
            wrap - (indent - PAD),
            ink,
            &styles,
        );
        push_row(&mut self.rows, y, indent, layout, band, Vec::new())
    }

    /// A one-line marker for a moved or reordered block: the label, then an
    /// elided preview of the block it is talking about.
    fn push_marker(
        &mut self,
        y: f32,
        wrap: f32,
        label: Key,
        block: &koru_history::unified::BlockView,
    ) -> f32 {
        let indent = indent_of(block.depth);
        let preview = self.engine.elided(
            &block.text,
            theme::role::hint(),
            (wrap - (indent - PAD)) * 0.7,
        );
        let text = if preview.is_empty() {
            self.t(label).to_string()
        } else {
            format!("{} · {}", self.t(label), preview)
        };
        let layout = self.engine.simple_layout(&text, theme::role::hint());
        push_row(&mut self.rows, y, indent, layout, None, Vec::new())
    }

    fn paint(&mut self, scene: &mut Scene, root: Affine) {
        if self.dirty || self.laid_w != self.column_w() {
            self.rebuild();
        }
        let (w, h) = self.viewport;
        let ox = self.origin_x();
        let column = self.column_w();
        // The ground under the column, per the source document's mode: Page
        // gets its canvas with the sheet scrolling over it, Narrow and Full a
        // uniform page background — the editor's own vocabulary.
        if self.view_mode == theme::ViewMode::Page {
            let canvas = self
                .canvas_bg
                .map(|c| vello::peniko::Color::from_rgba8(c[0], c[1], c[2], c[3]))
                .unwrap_or(theme::p().canvas);
            scene::fill(scene, root, canvas, &scene::rect(0.0, 0.0, w, h));
            let sheet_y = theme::PAGE_GAP - self.scroll;
            scene::fill(
                scene,
                root,
                theme::p().bg,
                &scene::rrect(
                    ox,
                    sheet_y,
                    column,
                    self.content_h.max(h),
                    theme::PAGE_RADIUS,
                ),
            );
        } else {
            scene::fill(scene, root, theme::p().bg, &scene::rect(0.0, 0.0, w, h));
        }
        let top = self.scroll;
        let bottom = self.scroll + h;
        for row in &self.rows {
            if row.y + row.h < top || row.y > bottom {
                continue;
            }
            let rx = ox + row.indent;
            let oy = row.y - self.scroll;
            if let Some(color) = row.band {
                scene::fill(
                    scene,
                    root,
                    color,
                    &scene::rrect(
                        rx - BAND_PAD_X,
                        oy - BAND_PAD_Y,
                        column - row.indent - PAD + 2.0 * BAND_PAD_X,
                        row.h + 2.0 * BAND_PAD_Y,
                        4.0,
                    ),
                );
            }
            for (range, color) in &row.highlights {
                scene::fill_text_range(scene, root, &row.layout, range.clone(), rx, oy, *color);
            }
            scene::draw_layout(scene, root, &row.layout, rx, oy, None);
        }
        if let Some(geom) = self.scrollbar() {
            scrollbar::draw(scene, root, geom, self.scroll_drag.is_some());
        }
    }
}

fn indent_of(depth: usize) -> f32 {
    PAD + depth.saturating_sub(1) as f32 * INDENT
}

fn weight_of(block: &koru_history::unified::BlockView) -> f32 {
    if block.heading.is_some() {
        HEADING_WEIGHT
    } else {
        theme::role::body().weight.value()
    }
}

/// A task's checkbox state, spelled so a done flip is visible in the text.
fn push_prefix(text: &mut String, block: &koru_history::unified::BlockView) {
    match block.done {
        Some(true) => text.push_str("☑ "),
        Some(false) => text.push_str("☐ "),
        None => {}
    }
}

fn block_text(block: &koru_history::unified::BlockView) -> String {
    block.text.clone()
}

fn push_plain(
    rows: &mut Vec<LaidRow>,
    y: f32,
    indent: f32,
    layout: Layout<Brush>,
    gap: f32,
) -> f32 {
    let h = layout.height();
    rows.push(LaidRow {
        y,
        h,
        indent,
        layout,
        band: None,
        highlights: Vec::new(),
    });
    y + h + gap
}

fn push_row(
    rows: &mut Vec<LaidRow>,
    y: f32,
    indent: f32,
    layout: Layout<Brush>,
    band: Option<vello::peniko::Color>,
    highlights: Vec<(std::ops::Range<usize>, vello::peniko::Color)>,
) -> f32 {
    let h = layout.height();
    rows.push(LaidRow {
        y,
        h,
        indent,
        layout,
        band,
        highlights,
    });
    y + h + ROW_GAP
}

impl View for DiffView {
    fn key(
        &mut self,
        _ws: &mut koru_core::schema::WorkspaceDoc,
        _key: &winit::keyboard::Key,
        _text: Option<&str>,
        _mods: winit::keyboard::ModifiersState,
    ) -> bool {
        false
    }

    fn mouse_double_click(
        &mut self,
        _ws: &koru_core::schema::WorkspaceDoc,
        _x: f32,
        _y: f32,
    ) -> bool {
        false
    }

    fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w, h);
        self.clamp_scroll();
    }

    fn scroll_by(&mut self, delta: MouseScrollDelta) {
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * theme::ROW_H,
            MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        self.scroll -= dy;
        self.clamp_scroll();
    }

    fn mouse_down(
        &mut self,
        _ws: &mut koru_core::schema::WorkspaceDoc,
        x: f32,
        y: f32,
        _ctrl: bool,
    ) -> bool {
        if let Some(geom) = self.scrollbar() {
            if geom.on_thumb(x, y) {
                self.scroll_drag = Some(y - geom.thumb_top);
                return true;
            }
            if geom.in_gutter(x, y) {
                self.scroll = geom.scroll_for_pointer(y, geom.thumb_h / 2.0);
                self.clamp_scroll();
                self.scroll_drag = Some(geom.thumb_h / 2.0);
                return true;
            }
        }
        false
    }

    fn mouse_move(&mut self, _x: f32, y: f32) -> bool {
        let Some(grab) = self.scroll_drag else {
            return false;
        };
        if let Some(geom) = self.scrollbar() {
            self.scroll = geom.scroll_for_pointer(y, grab);
            self.clamp_scroll();
        }
        true
    }

    fn mouse_up(&mut self, _ws: &mut koru_core::schema::WorkspaceDoc) -> bool {
        self.scroll_drag.take().is_some()
    }

    fn cursor_icon(&self) -> CursorIcon {
        CursorIcon::Default
    }

    fn adopt(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}

    fn reseat(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}

    /// The theme may have changed under us, and every row has colours baked
    /// in — relayout before the next draw.
    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn draw(
        &mut self,
        scene: &mut Scene,
        _workspace: &koru_workspace::Workspace,
        _images: &koru_draw::ImageCache,
        root: Affine,
        _caret_on: bool,
        _pointer: (f32, f32),
    ) {
        self.paint(scene, root);
    }
}

/// The tab-strip glyph: the same clock the History section wears.
pub fn tab_icon() -> char {
    icons::CLOCK
}

#[cfg(test)]
mod tests;
