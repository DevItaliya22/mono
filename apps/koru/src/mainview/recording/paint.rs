// SPDX-License-Identifier: MPL-2.0

//! Where the recording modal's controls are, and what they look like.
//!
//! **One geometry, two readers**, the delegation modal's own rule: [`layout`]
//! returns every control's rectangle in draw order, and both the drawing pass
//! and the container's hit test walk that same list, so a control can never be
//! clickable somewhere it is not painted. Nothing here decides anything — it
//! reads [`super::modal::Modal`] and puts pixels on the screen.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_i18n::{Key as I18nKey, t};
use koru_ui::text_field::{self, draw_field};
use koru_ui::{ActionButton, ActionStyle, Chip};
use vello::Scene;
use vello::kurbo::Affine;

use super::modal::{FRAME_RATES, Hit, MAX_DIM, MIN_DIM, Modal, Preset};

const WIDTH: f32 = 480.0;
const PAD: f32 = 16.0;
const GAP: f32 = 10.0;
const TITLE_H: f32 = 28.0;
const LABEL_H: f32 = 22.0;
const ROW_H: f32 = 26.0;
const FIELD_H: f32 = 30.0;
const PROBLEM_H: f32 = 20.0;
const RADIUS: f32 = 10.0;
const ROW_RADIUS: f32 = 6.0;

fn content_height(modal: &Modal) -> f32 {
    let mut h = TITLE_H + LABEL_H + Preset::ALL.len() as f32 * ROW_H + GAP * 0.5;
    if modal.preset() == Preset::Custom {
        h += LABEL_H + FIELD_H + GAP * 0.5;
    }
    // Frame rate, cursor, destination: label + control row each.
    h += 3.0 * (LABEL_H + FIELD_H) + GAP * 0.5;
    if modal.problem().is_some() {
        h += PROBLEM_H;
    }
    h += GAP + ActionButton::H + PAD;
    h
}

/// The panel, centred, sized to what it holds and clamped to the viewport.
pub(crate) fn panel(modal: &Modal, viewport: (f32, f32)) -> Rect {
    let h = content_height(modal).min(viewport.1 - 16.0).max(1.0);
    let w = WIDTH.min((viewport.0 - 16.0).max(1.0));
    Rect::new(
        ((viewport.0 - w) * 0.5).max(8.0),
        ((viewport.1 - h) * 0.5).max(8.0),
        w,
        h,
    )
}

/// Every control's rectangle, in draw order. The single source both the paint
/// below and the container's hit test read.
pub(crate) fn layout(
    engine: &mut TextEngine,
    modal: &Modal,
    viewport: (f32, f32),
) -> Vec<(Hit, Rect)> {
    let panel = panel(modal, viewport);
    let row_w = panel.w - PAD * 2.0;
    let mut out = Vec::new();
    let mut y = panel.y + PAD + TITLE_H + LABEL_H;
    for index in 0..Preset::ALL.len() {
        out.push((
            Hit::Preset(index),
            Rect::new(panel.x + PAD, y, row_w, ROW_H),
        ));
        y += ROW_H;
    }
    y += GAP * 0.5;
    if modal.preset() == Preset::Custom {
        y += LABEL_H;
        let half = (row_w - GAP) * 0.5;
        out.push((Hit::CustomWidth, Rect::new(panel.x + PAD, y, half, FIELD_H)));
        out.push((
            Hit::CustomHeight,
            Rect::new(panel.x + PAD + half + GAP, y, half, FIELD_H),
        ));
        y += FIELD_H + GAP * 0.5;
    }
    // Frame-rate chips.
    y += LABEL_H;
    let mut x = panel.x + PAD;
    for (index, rate) in FRAME_RATES.iter().enumerate() {
        let w = Chip::new(&format!("{rate} fps"), false).width(engine) + 12.0;
        out.push((Hit::FrameRate(index), Rect::new(x, y, w, Chip::H)));
        x += w + GAP;
    }
    y += Chip::H + GAP * 0.5;
    // Cursor chips.
    y += LABEL_H;
    let lang = crate::format::lang();
    let mut x = panel.x + PAD;
    for on in [true, false] {
        let label = t(
            lang,
            if on {
                I18nKey::RecordingCursorOn
            } else {
                I18nKey::RecordingCursorOff
            },
        );
        let w = Chip::new(label, false).width(engine) + 12.0;
        out.push((Hit::Cursor(on), Rect::new(x, y, w, Chip::H)));
        x += w + GAP;
    }
    y += Chip::H + GAP * 0.5;
    // Destination field + Browse.
    y += LABEL_H;
    let browse = ActionButton::new(
        t(lang, I18nKey::RecordingBrowse),
        ActionStyle::Secondary,
        None,
    );
    let browse_w = browse.width(engine) + 12.0;
    out.push((
        Hit::Path,
        Rect::new(panel.x + PAD, y, row_w - browse_w - GAP, FIELD_H),
    ));
    out.push((
        Hit::Browse,
        Rect::new(panel.x + PAD + row_w - browse_w, y, browse_w, FIELD_H),
    ));
    y += FIELD_H;
    if modal.problem().is_some() {
        y += PROBLEM_H;
    }
    y += GAP;
    // The answers: Record dominant on the right, Cancel quiet beside it.
    let record = ActionButton::new(
        t(lang, I18nKey::RecordingRecord),
        ActionStyle::Primary,
        None,
    );
    let cancel = ActionButton::new(
        t(lang, I18nKey::RecordingCancel),
        ActionStyle::Secondary,
        None,
    );
    let record_w = record.width(engine) + 24.0;
    let cancel_w = cancel.width(engine) + 12.0;
    out.push((
        Hit::Cancel,
        Rect::new(
            panel.x + panel.w - PAD - cancel_w - GAP - record_w,
            y,
            cancel_w,
            ActionButton::H,
        ),
    ));
    out.push((
        Hit::Record,
        Rect::new(
            panel.x + panel.w - PAD - record_w,
            y,
            record_w,
            ActionButton::H,
        ),
    ));
    out
}

/// Where the text of a field starts, for turning a press into a text-local x.
pub(crate) fn text_x(rect: Rect) -> f32 {
    rect.x + text_field::PAD
}

/// The 3-2-1 over a dim — **paint-only**, which is why no numeral can outlive
/// the frame that drops it. `numeral` is 3, 2 or 1.
pub(crate) fn draw_countdown(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    viewport: (f32, f32),
    numeral: u32,
) {
    let p = theme::p();
    scene::fill(
        scene,
        root,
        p.scrim,
        &scene::rect(0.0, 0.0, viewport.0, viewport.1),
    );
    // A display numeral rather than a type role: the _3XL metrics, scaled.
    // The countdown is the one place chrome may be this loud.
    const SCALE: f64 = 4.0;
    let style = theme::TextStyle::new(theme::text_metrics::_3XL, theme::FontWeight::SemiBold);
    let text = engine.simple_layout(&numeral.to_string(), style);
    let centered = root
        * Affine::translate((viewport.0 as f64 * 0.5, viewport.1 as f64 * 0.5))
        * Affine::scale(SCALE);
    scene::draw_layout_first_line(
        scene,
        centered,
        &text,
        -text.width() * 0.5,
        -text.height() * 0.5,
        Some(p.text),
    );
}

fn label(scene: &mut Scene, root: Affine, engine: &mut TextEngine, key: I18nKey, x: f32, y: f32) {
    let lang = crate::format::lang();
    let layout = engine.simple_layout(t(lang, key), theme::role::hint());
    scene::draw_layout_first_line(scene, root, &layout, x, y + 4.0, Some(theme::p().text_dim));
}

/// The inline problem's sentence. `t` returns the template raw — only
/// `koru_i18n::fill` substitutes the `{}`s — so the size bounds are filled
/// here, at the one place the sentence is drawn.
fn problem_text(lang: koru_i18n::Lang, problem: I18nKey) -> String {
    match problem {
        I18nKey::RecordingProblemSize => koru_i18n::fill(
            t(lang, problem),
            &[&MIN_DIM.to_string(), &MAX_DIM.to_string()],
        ),
        _ => t(lang, problem).to_owned(),
    }
}

/// The modal, its scrim, and everything in the panel.
pub(crate) fn draw(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    modal: &mut Modal,
    viewport: (f32, f32),
    pointer: (f32, f32),
) {
    let p = theme::p();
    scene::fill(
        scene,
        root,
        p.scrim,
        &scene::rect(0.0, 0.0, viewport.0, viewport.1),
    );
    let panel = panel(modal, viewport);
    scene::floating_panel(
        scene,
        root,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        &scene::Panel::popover(RADIUS),
    );

    let lang = crate::format::lang();
    let focus = modal.focus();
    let hovered = |rect: Rect| rect.contains(pointer.0, pointer.1);
    let focused = |hit: Hit| focus == Some(hit);
    let hits = layout(engine, modal, viewport);
    let rect_of = |want: Hit| {
        hits.iter()
            .find(|(hit, _)| *hit == want)
            .map(|(_, rect)| *rect)
    };

    // Title.
    let title = engine.simple_layout(t(lang, I18nKey::RecordingTitle), theme::role::ui());
    scene::draw_layout_first_line(
        scene,
        root,
        &title,
        panel.x + PAD,
        panel.y + PAD + 4.0,
        Some(p.text),
    );

    // Resolution presets.
    let mut y = panel.y + PAD + TITLE_H;
    label(
        scene,
        root,
        engine,
        I18nKey::RecordingResolutionLabel,
        panel.x + PAD,
        y,
    );
    y += LABEL_H;
    for (index, preset) in Preset::ALL.iter().enumerate() {
        let rect = rect_of(Hit::Preset(index)).unwrap_or(Rect::new(
            panel.x + PAD,
            y,
            panel.w - PAD * 2.0,
            ROW_H,
        ));
        let chosen = modal.preset() == *preset;
        if chosen {
            scene::fill(
                scene,
                root,
                p.row_selected,
                &scene::rrect(rect.x, rect.y, rect.w, rect.h, ROW_RADIUS),
            );
        } else if hovered(rect) {
            scene::fill(
                scene,
                root,
                p.hover_tint,
                &scene::rrect(rect.x, rect.y, rect.w, rect.h, ROW_RADIUS),
            );
        }
        if focused(Hit::Preset(index)) {
            scene::stroke(
                scene,
                root,
                p.focus_ring,
                theme::BORDER_W,
                &scene::rrect(rect.x, rect.y, rect.w, rect.h, ROW_RADIUS),
            );
        }
        let text = engine.simple_layout(t(lang, preset.label()), theme::role::row());
        scene::draw_layout_first_line(
            scene,
            root,
            &text,
            rect.x + 8.0,
            rect.y + (ROW_H - text.height().min(ROW_H)) * 0.5,
            Some(p.text),
        );
        if let Some((w, h)) = preset.dims() {
            let dims = engine.simple_layout(&format!("{w} × {h}"), theme::role::hint());
            scene::draw_layout_first_line(
                scene,
                root,
                &dims,
                rect.x + rect.w - dims.width() - 8.0,
                rect.y + (ROW_H - dims.height().min(ROW_H)) * 0.5,
                Some(p.text_dim),
            );
        }
        y += ROW_H;
    }
    y += GAP * 0.5;

    // The custom pair, only while *Custom…* is chosen — same rule the ring
    // follows, so nothing hidden is ever focusable or clickable.
    if modal.preset() == Preset::Custom {
        label(
            scene,
            root,
            engine,
            I18nKey::RecordingWidthLabel,
            panel.x + PAD,
            y,
        );
        if let Some(rect) = rect_of(Hit::CustomHeight) {
            label(
                scene,
                root,
                engine,
                I18nKey::RecordingHeightLabel,
                rect.x,
                y,
            );
        }
        y += LABEL_H;
        for hit in [Hit::CustomWidth, Hit::CustomHeight] {
            let Some(rect) = rect_of(hit) else { continue };
            let field = match hit {
                Hit::CustomWidth => modal.custom_width_field(),
                _ => modal.custom_height_field(),
            };
            draw_field(
                scene,
                root,
                field,
                engine,
                None,
                (rect.x, rect.y, rect.w, rect.h),
                focused(hit),
            );
        }
        y += FIELD_H + GAP * 0.5;
    }

    // Frame rate.
    label(
        scene,
        root,
        engine,
        I18nKey::RecordingFrameRateLabel,
        panel.x + PAD,
        y,
    );
    y += LABEL_H;
    for (index, rate) in FRAME_RATES.iter().enumerate() {
        let Some(rect) = rect_of(Hit::FrameRate(index)) else {
            continue;
        };
        Chip::new(&format!("{rate} fps"), modal.frame_rate() == *rate).draw(
            scene,
            root,
            engine,
            rect,
            hovered(rect),
            focused(Hit::FrameRate(index)),
        );
    }
    y += Chip::H + GAP * 0.5;

    // Cursor.
    label(
        scene,
        root,
        engine,
        I18nKey::RecordingCursorLabel,
        panel.x + PAD,
        y,
    );
    y += LABEL_H;
    for on in [true, false] {
        let Some(rect) = rect_of(Hit::Cursor(on)) else {
            continue;
        };
        let text = t(
            lang,
            if on {
                I18nKey::RecordingCursorOn
            } else {
                I18nKey::RecordingCursorOff
            },
        );
        Chip::new(text, modal.cursor() == on).draw(
            scene,
            root,
            engine,
            rect,
            hovered(rect),
            focused(Hit::Cursor(on)),
        );
    }
    y += Chip::H + GAP * 0.5;

    // Destination.
    label(
        scene,
        root,
        engine,
        I18nKey::RecordingSaveToLabel,
        panel.x + PAD,
        y,
    );
    y += LABEL_H;
    if let Some(rect) = rect_of(Hit::Path) {
        draw_field(
            scene,
            root,
            modal.path_field(),
            engine,
            None,
            (rect.x, rect.y, rect.w, rect.h),
            focused(Hit::Path),
        );
    }
    if let Some(rect) = rect_of(Hit::Browse) {
        ActionButton::new(
            t(lang, I18nKey::RecordingBrowse),
            ActionStyle::Secondary,
            None,
        )
        .draw_focused(
            scene,
            root,
            engine,
            rect,
            hovered(rect),
            focused(Hit::Browse),
        );
    }
    y += FIELD_H;

    // The inline problem, when there is one — the same sentence *Record*
    // refuses with. The buttons below find their rectangles from `layout`,
    // so the walk ends here.
    if let Some(problem) = modal.problem() {
        let text = engine.simple_layout(&problem_text(lang, problem), theme::role::hint());
        scene::draw_layout_first_line(
            scene,
            root,
            &text,
            panel.x + PAD,
            y + 2.0,
            Some(p.close_hover),
        );
    }

    // The answers. Record is the primary and visually dominant; when the
    // values have a problem it draws dimmed rather than scolding on press.
    let valid = modal.problem().is_none();
    if let Some(rect) = rect_of(Hit::Cancel) {
        ActionButton::new(
            t(lang, I18nKey::RecordingCancel),
            ActionStyle::Secondary,
            None,
        )
        .draw_focused(
            scene,
            root,
            engine,
            rect,
            hovered(rect),
            focused(Hit::Cancel),
        );
    }
    if let Some(rect) = rect_of(Hit::Record) {
        if valid {
            ActionButton::new(
                t(lang, I18nKey::RecordingRecord),
                ActionStyle::Primary,
                None,
            )
            .draw_focused(
                scene,
                root,
                engine,
                rect,
                hovered(rect),
                focused(Hit::Record),
            );
        } else {
            let shape = scene::rrect(rect.x, rect.y, rect.w, rect.h, ActionButton::RADIUS);
            scene::stroke(scene, root, p.panel_border, theme::BORDER_W, &shape);
            let text = engine.simple_layout(t(lang, I18nKey::RecordingRecord), theme::role::row());
            scene::draw_layout_first_line(
                scene,
                root,
                &text,
                rect.x + (rect.w - text.width()).max(0.0) * 0.5,
                rect.y + (rect.h - text.height().min(rect.h)) * 0.5,
                Some(p.text_dim),
            );
        }
    }
}
