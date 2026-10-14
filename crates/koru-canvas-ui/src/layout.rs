// SPDX-License-Identifier: MPL-2.0

//! **One layout pass per object, shared by drawing and hit testing.**
//!
//! An object is laid out once, in canvas units, at its width: its text, its
//! header, its status line and its buttons, each at a place in the object's
//! own coordinates. Drawing walks those places and hit testing asks the same
//! ones — a button is pressable exactly where it is drawn, because there is no
//! second copy of the arithmetic (DESIGN, *Rules a view follows*).
//!
//! **Pan and zoom never come here.** They change the transform an object is
//! drawn under and nothing else; the cache below is keyed by what an object
//! says and how wide it is, so a layout is rebuilt only when one of those
//! moved.
//!
//! **One typeface for every card, on every computer.** Card text is set in
//! the default body face at the document body's size — not the reader's body
//! font or density — so a card wraps the same way wherever it is opened
//! (CANVAS Q11). A node's header, status and buttons are chrome and wear the
//! app's font like every other control.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::theme::{self, Brush, FontWeight, TextStyle, text_metrics};
use koru_draw::{icons, scene};
use koru_i18n::{Key, Lang, t};
use koru_ui::button::{ActionButton, ActionStyle};
use parley::Layout;

use crate::model::{CanvasRequest, NodeStatus, ObjectKey, ObjectKind, SceneObject};
use crate::words;

#[cfg(test)]
mod tests;

/// The width a new card is given, and the width drawn for an object whose
/// width was never set.
pub const DEFAULT_WIDTH: i32 = 240;
/// The narrowest a card can be dragged.
pub const MIN_CARD_WIDTH: i32 = 80;
/// The narrowest a workflow node can be dragged: its buttons need the room.
pub const MIN_NODE_WIDTH: i32 = 160;
/// The narrowest an image can be dragged.
pub const MIN_IMAGE_WIDTH: i32 = 24;
/// The widest anything can be dragged.
pub const MAX_WIDTH: i32 = 4_000;
/// Every position and width this surface writes is inside ±this (CANVAS §5).
pub const COORD_LIMIT: i32 = 1_000_000;

/// The air inside a card.
pub(crate) const PAD: f32 = 12.0;
/// A card's corner radius — the hover chip's, the same radius the explore
/// column's open-document card wears.
pub(crate) const RADIUS: f32 = 6.0;
/// Between the parts of a node.
pub(crate) const GAP: f32 = 8.0;
/// A node's header row, which is also the height of each of its buttons.
pub(crate) const ROW_H: f32 = ActionButton::H;
/// A node's status line.
pub(crate) const STATUS_H: f32 = 18.0;
/// The header glyph.
pub(crate) const GLYPH: f32 = theme::CAPTION_ICON;
/// The status glyph.
pub(crate) const STATUS_GLYPH: f32 = 12.0;
/// How many lines of an agent's result the node previews.
pub(crate) const RESULT_LINES: usize = 5;

/// The one face every card's text is set in.
pub(crate) fn card_stack() -> &'static str {
    theme::default_body_font().stack()
}

/// The document body's size, fixed: not the reader's density.
pub(crate) fn card_style() -> TextStyle {
    TextStyle::new(text_metrics::DOC_BODY, FontWeight::Medium)
}

/// An agent's result preview: the same face, a step smaller.
fn result_style() -> TextStyle {
    TextStyle::new(text_metrics::SM, FontWeight::Medium)
}

/// The width an object is drawn at: its own, or the default when it has
/// none, inside the limits for its kind.
pub(crate) fn clamp_width(kind: ObjectKind, w: i32) -> i32 {
    let w = if w <= 0 { DEFAULT_WIDTH } else { w };
    w.clamp(min_width(kind), MAX_WIDTH)
}

pub(crate) fn min_width(kind: ObjectKind) -> i32 {
    match kind {
        ObjectKind::Card => MIN_CARD_WIDTH,
        ObjectKind::Image => MIN_IMAGE_WIDTH,
        ObjectKind::Start | ObjectKind::Agent | ObjectKind::Output => MIN_NODE_WIDTH,
    }
}

/// A coordinate as this surface writes it: whole, and inside the limit.
pub(crate) fn clamp_coord(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    v.round()
        .clamp(-f64::from(COORD_LIMIT), f64::from(COORD_LIMIT)) as i32
}

/// An image's height at `w`, from its aspect ratio. An image whose size is
/// not known yet is drawn four by three until it is.
pub(crate) fn image_height(object: &SceneObject, w: i32) -> f32 {
    match &object.image {
        Some(image) if image.width > 0 && image.height > 0 => {
            (w as f32 * image.height as f32 / image.width as f32).max(1.0)
        }
        _ => w as f32 * 0.75,
    }
}

/// Something a press inside an object does.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Part {
    Run,
    Stop,
    Respond,
    ChooseAgent,
    PlaceResult,
    OpenLink,
}

impl Part {
    pub(crate) fn request(self, key: ObjectKey) -> CanvasRequest {
        match self {
            Self::Run => CanvasRequest::Run { key },
            Self::Stop => CanvasRequest::Stop { key },
            Self::Respond => CanvasRequest::Respond { key },
            Self::ChooseAgent => CanvasRequest::ChooseAgent { key },
            Self::PlaceResult => CanvasRequest::PlaceResult { key },
            Self::OpenLink => CanvasRequest::OpenLink { key },
        }
    }
}

/// A button inside an object, in the object's own coordinates.
pub(crate) struct Button {
    pub part: Part,
    pub rect: Rect,
    pub label: String,
    pub style: ActionStyle,
    pub icon: Option<char>,
}

/// Wrapped text and where it sits.
pub(crate) struct TextBlock {
    /// What was laid out, and at what width — the cache key.
    source: String,
    wrap: f32,
    pub layout: Layout<Brush>,
    pub x: f32,
    pub y: f32,
}

impl TextBlock {
    pub(crate) fn height(&self, style: TextStyle) -> f32 {
        self.layout.height().max(style.metrics.line_height)
    }
}

/// A single glyph or line of chrome text, and where it sits.
pub(crate) struct Mark {
    pub layout: Layout<Brush>,
    pub x: f32,
    pub y: f32,
}

/// How a status line is inked.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Tone {
    Dim,
    Failed,
}

/// What an object was laid out *from*: when any of it differs, the layout is
/// stale. Compared field by field against the scene, without allocating.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Content {
    kind: ObjectKind,
    w: i32,
    lang: Lang,
    text: String,
    lines: Vec<String>,
    image: Option<(u32, u32)>,
    agent: Option<String>,
    status: Option<NodeStatus>,
    result: Vec<String>,
    link: Option<String>,
}

impl Content {
    fn of(object: &SceneObject, w: i32, lang: Lang) -> Self {
        Self {
            kind: object.kind,
            w,
            lang,
            text: object.text.clone(),
            lines: object.lines.clone(),
            image: object.image.as_ref().map(|i| (i.width, i.height)),
            agent: object.agent.clone(),
            status: object.status.clone(),
            result: object.result.iter().take(RESULT_LINES).cloned().collect(),
            link: object.link.clone(),
        }
    }

    pub(crate) fn matches(&self, object: &SceneObject, w: i32, lang: Lang) -> bool {
        self.kind == object.kind
            && self.w == w
            && self.lang == lang
            && self.text == object.text
            && self.lines == object.lines
            && self.image == object.image.as_ref().map(|i| (i.width, i.height))
            && self.agent == object.agent
            && self.status == object.status
            && self.result.len() == object.result.len().min(RESULT_LINES)
            && self.result.iter().zip(&object.result).all(|(a, b)| a == b)
            && self.link == object.link
    }
}

/// One object, laid out.
pub(crate) struct ObjectLayout {
    pub content: Content,
    pub w: f32,
    pub h: f32,
    pub header_glyph: Option<Mark>,
    pub header_label: Option<Mark>,
    pub body: Option<TextBlock>,
    pub status_glyph: Option<(Mark, bool)>,
    pub status: Option<(Mark, Tone)>,
    pub divider_y: Option<f32>,
    pub result: Option<TextBlock>,
    pub buttons: Vec<Button>,
    /// The mark an image draws while its pixels are not here.
    pub placeholder: Option<Mark>,
}

impl ObjectLayout {
    /// The button under a point in the object's own coordinates.
    pub(crate) fn button_at(&self, x: f32, y: f32) -> Option<Part> {
        self.buttons
            .iter()
            .find(|button| button.rect.contains(x, y))
            .map(|button| button.part)
    }

    #[cfg(test)]
    pub(crate) fn button(&self, part: Part) -> Option<&Button> {
        self.buttons.iter().find(|button| button.part == part)
    }
}

/// What the layout pass counted — how the culling test knows what was done.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct Counters {
    /// Objects laid out: a geometry pass run because one was missing or
    /// stale.
    pub objects_laid: u64,
    /// Blocks of card text shaped and wrapped.
    pub texts_laid: u64,
    /// Objects drawn by the last `draw`.
    pub objects_drawn: u64,
}

/// A card's text as one string: the first line, then the further lines.
fn joined(first: &str, rest: &[String]) -> String {
    if rest.is_empty() {
        return first.to_owned();
    }
    let mut out =
        String::with_capacity(first.len() + rest.iter().map(|l| l.len() + 1).sum::<usize>());
    out.push_str(first);
    for line in rest {
        out.push('\n');
        out.push_str(line);
    }
    out
}

/// Lay a block of card text out at `wrap`, or keep the one already laid out
/// if it says the same thing at the same width.
fn text_block(
    engine: &mut TextEngine,
    reuse: Option<TextBlock>,
    source: String,
    style: TextStyle,
    wrap: f32,
    (x, y): (f32, f32),
    counters: &mut Counters,
) -> TextBlock {
    if let Some(mut block) = reuse
        && block.source == source
        && block.wrap == wrap
    {
        block.x = x;
        block.y = y;
        return block;
    }
    counters.texts_laid += 1;
    let layout = engine.simple_layout_wrapped_in(&source, style, card_stack(), wrap.max(1.0));
    TextBlock {
        source,
        wrap,
        layout,
        x,
        y,
    }
}

fn glyph(engine: &mut TextEngine, glyph: char, size: f32, x: f32, row_y: f32, row_h: f32) -> Mark {
    let layout = engine.icon_layout(glyph, size);
    let y = row_y + (row_h - layout.height()) * 0.5;
    Mark { layout, x, y }
}

fn label(
    engine: &mut TextEngine,
    text: &str,
    style: TextStyle,
    max_w: f32,
    x: f32,
    row_y: f32,
    row_h: f32,
) -> Mark {
    let text = engine.elided(text, style, max_w.max(0.0));
    let layout = engine.simple_layout(&text, style);
    let line_h = scene::first_line_height(&layout);
    Mark {
        layout,
        x,
        y: row_y + (row_h - line_h) * 0.5,
    }
}

/// A compact action sized to its label, never wider than `max_w`.
fn button(
    engine: &mut TextEngine,
    part: Part,
    text: &str,
    style: ActionStyle,
    icon: Option<char>,
    (x, y): (f32, f32),
    max_w: f32,
) -> Button {
    let icon_room = icon.map_or(0.0, |glyph| {
        ActionButton::GAP + engine.icon_layout(glyph, ActionButton::ICON_SIZE).width()
    });
    let label = engine.elided(
        text,
        theme::role::row(),
        (max_w - ActionButton::PAD_X * 2.0 - icon_room).max(0.0),
    );
    let w = ActionButton::new(&label, style, icon)
        .width(engine)
        .min(max_w.max(0.0));
    Button {
        part,
        rect: Rect::new(x, y, w, ROW_H),
        label,
        style,
        icon,
    }
}

/// Lay one object out at width `w` (already clamped for its kind).
///
/// `old` is the layout this object had before, if any: a text block that
/// says the same thing at the same width is kept rather than shaped again, so
/// a status that ticks never re-wraps an agent's instruction.
///
/// Without `workflows` a node has no Run, Stop or Respond button, and an
/// agent node's name is a plain label rather than the agent chooser
/// ([`crate::CanvasSurface::set_workflows`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lay_out(
    engine: &mut TextEngine,
    lang: Lang,
    object: &SceneObject,
    w: i32,
    old: Option<ObjectLayout>,
    counters: &mut Counters,
    workflows: bool,
) -> ObjectLayout {
    counters.objects_laid += 1;
    let (mut old_body, mut old_result) = match old {
        Some(old) => (old.body, old.result),
        None => (None, None),
    };
    let wf = w as f32;
    let inner = (wf - PAD * 2.0).max(1.0);
    let mut out = ObjectLayout {
        content: Content::of(object, w, lang),
        w: wf,
        h: 0.0,
        header_glyph: None,
        header_label: None,
        body: None,
        status_glyph: None,
        status: None,
        divider_y: None,
        result: None,
        buttons: Vec::new(),
        placeholder: None,
    };
    match object.kind {
        ObjectKind::Image => {
            out.h = image_height(object, w);
            let size = (wf.min(out.h) * 0.3).clamp(12.0, 32.0);
            let mut mark = glyph(engine, icons::IMAGE, size, 0.0, 0.0, out.h);
            mark.x = (wf - mark.layout.width()) * 0.5;
            out.placeholder = Some(mark);
        }
        ObjectKind::Card => {
            let body = text_block(
                engine,
                old_body.take(),
                joined(&object.text, &object.lines),
                card_style(),
                inner,
                (PAD, PAD),
                counters,
            );
            out.h = PAD * 2.0 + body.height(card_style());
            out.body = Some(body);
        }
        ObjectKind::Start | ObjectKind::Agent | ObjectKind::Output => {
            let mut y = PAD;
            let icon = match object.kind {
                ObjectKind::Start => icons::PLAY,
                ObjectKind::Agent => icons::ROBOT,
                _ => icons::DOCUMENT,
            };
            out.header_glyph = Some(glyph(engine, icon, GLYPH, PAD, y, ROW_H));
            let label_x = PAD + GLYPH + 6.0;
            let label_room = wf - PAD - label_x;
            match object.kind {
                ObjectKind::Agent if workflows => {
                    let name = object
                        .agent
                        .as_deref()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| t(lang, Key::CanvasChooseAgent));
                    // A ghost action whose label lines up with the other
                    // nodes' header words: its own padding hangs to the left.
                    out.buttons.push(button(
                        engine,
                        Part::ChooseAgent,
                        name,
                        ActionStyle::Ghost,
                        Some(icons::CARET_DOWN),
                        (label_x - ActionButton::PAD_X, y),
                        label_room + ActionButton::PAD_X,
                    ));
                }
                _ => {
                    // An agent node without the chooser keeps its agent's
                    // name when it has one.
                    let word = object
                        .agent
                        .as_deref()
                        .filter(|name| object.kind == ObjectKind::Agent && !name.trim().is_empty())
                        .unwrap_or_else(|| words::kind_word(lang, object.kind));
                    out.header_label = Some(label(
                        engine,
                        word,
                        theme::role::row().with_weight(FontWeight::SemiBold),
                        label_room,
                        label_x,
                        y,
                        ROW_H,
                    ));
                }
            }
            y += ROW_H + GAP * 0.5;
            if !object.text.is_empty() || !object.lines.is_empty() {
                let body = text_block(
                    engine,
                    old_body.take(),
                    joined(&object.text, &object.lines),
                    card_style(),
                    inner,
                    (PAD, y),
                    counters,
                );
                y += body.height(card_style()) + GAP;
                out.body = Some(body);
            }
            if object.kind == ObjectKind::Output
                && let Some(link) = object.link.as_deref().filter(|l| !l.trim().is_empty())
            {
                out.buttons.push(button(
                    engine,
                    Part::OpenLink,
                    link,
                    ActionStyle::Ghost,
                    Some(icons::LINK),
                    (PAD - ActionButton::PAD_X, y),
                    inner + ActionButton::PAD_X,
                ));
                y += ROW_H + GAP;
            }
            if let Some(status) = &object.status
                && let Some(text) = words::status_text(lang, status)
            {
                let mark = match status {
                    NodeStatus::Running { .. } => Some((icons::CIRCLE_NOTCH, true)),
                    NodeStatus::NeedsResponse => Some((icons::WARNING, false)),
                    NodeStatus::Done => Some((icons::CHECK, false)),
                    NodeStatus::Failed(_) => Some((icons::WARNING, false)),
                    NodeStatus::Stopped => Some((icons::STOP, false)),
                    NodeStatus::Idle | NodeStatus::NotStarted => None,
                };
                let text_x = PAD + STATUS_GLYPH + 6.0;
                if let Some((icon, spin)) = mark {
                    out.status_glyph =
                        Some((glyph(engine, icon, STATUS_GLYPH, PAD, y, STATUS_H), spin));
                }
                let tone = if matches!(status, NodeStatus::Failed(_)) {
                    Tone::Failed
                } else {
                    Tone::Dim
                };
                out.status = Some((
                    label(
                        engine,
                        &text,
                        theme::role::meta(),
                        wf - PAD - text_x,
                        text_x,
                        y,
                        STATUS_H,
                    ),
                    tone,
                ));
                y += STATUS_H + GAP;
            }
            if workflows && object.kind.runs() {
                if object.kind == ObjectKind::Agent
                    && object.status == Some(NodeStatus::NeedsResponse)
                {
                    out.buttons.push(button(
                        engine,
                        Part::Respond,
                        t(lang, Key::CanvasRespond),
                        ActionStyle::Primary,
                        None,
                        (PAD, y),
                        inner,
                    ));
                    y += ROW_H + GAP;
                }
                let running = object.status.as_ref().is_some_and(NodeStatus::is_running);
                let (part, word, style) = if running {
                    (Part::Stop, Key::CanvasStop, ActionStyle::Secondary)
                } else {
                    (Part::Run, Key::CanvasRun, ActionStyle::Primary)
                };
                out.buttons.push(button(
                    engine,
                    part,
                    t(lang, word),
                    style,
                    None,
                    (PAD, y),
                    inner,
                ));
                y += ROW_H;
            } else {
                // The last part added its trailing gap; a node that ends
                // without buttons closes on its padding alone.
                y -= GAP;
            }
            if object.kind == ObjectKind::Agent && !object.result.is_empty() {
                y += GAP;
                out.divider_y = Some(y);
                y += GAP;
                let lines = &object.result[..object.result.len().min(RESULT_LINES)];
                let result = text_block(
                    engine,
                    old_result.take(),
                    lines.join("\n"),
                    result_style(),
                    inner,
                    (PAD, y),
                    counters,
                );
                y += result.height(result_style()) + GAP;
                out.result = Some(result);
                out.buttons.push(button(
                    engine,
                    Part::PlaceResult,
                    t(lang, Key::CanvasPlaceResult),
                    ActionStyle::Secondary,
                    None,
                    (PAD, y),
                    inner,
                ));
                y += ROW_H;
            }
            out.h = (y + PAD).max(PAD * 2.0 + ROW_H);
        }
    }
    out
}

/// **An upper bound on an object's height, without laying it out.**
///
/// Culling has to decide whether an object could be on screen before paying
/// for its text; this answers that from arithmetic. Every term is at least
/// what [`lay_out`] can produce — a glyph is taken at one and a half ems, the
/// widest a shaped glyph plausibly runs — so an object this says is off
/// screen is off screen.
pub(crate) fn height_bound(object: &SceneObject, w: i32) -> f32 {
    if object.kind == ObjectKind::Image {
        return image_height(object, w);
    }
    let inner = (w as f32 - PAD * 2.0).max(1.0);
    let lines_bound = |lines: &mut dyn Iterator<Item = &str>, style: TextStyle| -> f32 {
        let per_line = (inner / (style.metrics.size * 1.5)).max(1.0);
        let mut count = 0.0f32;
        for line in lines {
            let chars = line.chars().count() as f32;
            count += (chars / per_line).ceil().max(1.0);
        }
        count.max(1.0) * style.metrics.line_height
    };
    let body = lines_bound(
        &mut std::iter::once(object.text.as_str()).chain(object.lines.iter().map(String::as_str)),
        card_style(),
    );
    let slack = 8.0;
    if object.kind == ObjectKind::Card {
        return PAD * 2.0 + body + slack;
    }
    let mut h = PAD * 2.0 + ROW_H + GAP + body + GAP;
    if object.link.is_some() {
        h += ROW_H + GAP;
    }
    if object.status.is_some() {
        h += STATUS_H + GAP;
    }
    h += ROW_H;
    if object.kind == ObjectKind::Agent && object.status == Some(NodeStatus::NeedsResponse) {
        h += ROW_H + GAP;
    }
    if !object.result.is_empty() {
        h += GAP * 3.0
            + lines_bound(
                &mut object.result.iter().take(RESULT_LINES).map(String::as_str),
                result_style(),
            )
            + ROW_H;
    }
    h + slack
}
