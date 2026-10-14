// SPDX-License-Identifier: MPL-2.0

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{icons, scene, theme};
use peniko::Fill;
use vello::Scene;
use vello::kurbo::Affine;

use super::layout::PendingInputLayout;
use super::{PendingAttachment, PendingAttachmentStatus, PendingInputCopy, PendingInputModel};
use koru_session_protocol::{DeliveryMode, DeliveryState, ErrorCode};

const MAX_RENDERED_DRAFT_CHARS: usize = 512;

pub(super) struct RenderState<'a> {
    pub model: &'a PendingInputModel,
    pub copy: &'a PendingInputCopy,
    pub focused: bool,
    pub hovered: Option<super::PendingInputHit>,
}

#[derive(Clone, Copy)]
struct ButtonVisual {
    hovered: bool,
    enabled: bool,
    primary: bool,
}

#[derive(Clone, Copy)]
struct AttachmentVisual {
    remove_hovered: bool,
    remove_enabled: bool,
}

#[derive(Clone, Copy)]
struct LabelStyle {
    role: theme::TextStyle,
    colour: peniko::Color,
    right_aligned: bool,
}

pub(super) fn draw(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    layout: &PendingInputLayout,
    state: RenderState<'_>,
) {
    let RenderState {
        model,
        copy,
        focused,
        hovered,
    } = state;
    let palette = theme::p();
    scene::fill(
        canvas,
        root,
        palette.panel_bg,
        &scene::rect(layout.area.x, layout.area.y, layout.area.w, layout.area.h),
    );
    scene::stroke(
        canvas,
        root,
        palette.panel_border,
        1.0,
        &scene::rect(layout.area.x, layout.area.y, layout.area.w, 0.0),
    );
    draw_text(
        canvas,
        root,
        engine,
        &copy.heading,
        layout.heading,
        LabelStyle {
            role: theme::role::section_heading(),
            colour: palette.text,
            right_aligned: false,
        },
    );
    if let Some(label) = delivery_label(model, copy) {
        draw_text(
            canvas,
            root,
            engine,
            label,
            layout.delivery,
            LabelStyle {
                role: theme::role::meta(),
                colour: palette.text_dim,
                right_aligned: true,
            },
        );
    }

    for item_layout in &layout.attachments {
        let Some(item) = model
            .attachments()
            .iter()
            .find(|item| item.id == item_layout.id)
        else {
            continue;
        };
        draw_attachment(
            canvas,
            root,
            engine,
            item_layout,
            item,
            copy,
            AttachmentVisual {
                remove_hovered: !model.submitting()
                    && hovered == Some(super::PendingInputHit::Remove(item.id)),
                remove_enabled: !model.submitting(),
            },
        );
    }
    if let Some(overflow) = layout.overflow {
        let hidden = model
            .attachments()
            .len()
            .saturating_sub(layout.attachments.len());
        if hidden > 0 {
            draw_text(
                canvas,
                root,
                engine,
                &format!("+{hidden}"),
                overflow,
                LabelStyle {
                    role: theme::role::meta(),
                    colour: palette.text_dim,
                    right_aligned: false,
                },
            );
        }
    }

    if let Some(code) = model.error() {
        draw_text(
            canvas,
            root,
            engine,
            error_label(code, copy),
            layout.error,
            LabelStyle {
                role: theme::role::hint(),
                colour: palette.notice_fg,
                right_aligned: false,
            },
        );
    }

    let draft_border = if focused {
        palette.focus_ring
    } else {
        palette.panel_border
    };
    scene::fill(
        canvas,
        root,
        palette.bg,
        &scene::rrect(
            layout.draft.x,
            layout.draft.y,
            layout.draft.w,
            layout.draft.h,
            7.0,
        ),
    );
    scene::stroke(
        canvas,
        root,
        draft_border,
        1.0,
        &scene::rrect(
            layout.draft.x,
            layout.draft.y,
            layout.draft.w,
            layout.draft.h,
            7.0,
        ),
    );
    let (draft, showing_placeholder) = draft_line(model.draft(), &copy.placeholder, focused);
    let colour = if showing_placeholder {
        palette.placeholder
    } else {
        palette.text
    };
    canvas.push_clip_layer(
        Fill::NonZero,
        root,
        &scene::rect(
            layout.draft.x + 8.0,
            layout.draft.y,
            (layout.draft.w - 16.0).max(0.0),
            layout.draft.h,
        ),
    );
    let draft_layout = engine.simple_layout(draft, theme::role::ui());
    scene::draw_layout_first_line(
        canvas,
        root,
        &draft_layout,
        layout.draft.x + 9.0,
        layout.draft.y + (layout.draft.h - draft_layout.height()) * 0.5,
        Some(colour),
    );
    if focused && !model.submitting() && !model.has_retry() {
        let caret_x = (layout.draft.x + 9.0 + draft_layout.width())
            .min(layout.draft.x + layout.draft.w - 7.0);
        scene::fill(
            canvas,
            root,
            palette.caret,
            &scene::rect(caret_x, layout.draft.y + 9.0, 1.0, layout.draft.h - 18.0),
        );
    }
    canvas.pop_layer();

    draw_icon_button(
        canvas,
        root,
        engine,
        layout.attach,
        icons::PAPERCLIP,
        ButtonVisual {
            hovered: hovered == Some(super::PendingInputHit::Attach),
            enabled: model.session_id().is_some() && !model.submitting() && !model.has_retry(),
            primary: false,
        },
    );
    draw_icon_button(
        canvas,
        root,
        engine,
        layout.send,
        icons::CARET_UP,
        ButtonVisual {
            hovered: hovered
                == Some(if model.has_retry() {
                    super::PendingInputHit::Retry
                } else {
                    super::PendingInputHit::Send
                }),
            enabled: model.can_send() || (model.has_retry() && !model.submitting()),
            primary: true,
        },
    );
}

fn draw_attachment(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    layout: &super::AttachmentLayout,
    item: &PendingAttachment,
    copy: &PendingInputCopy,
    visual: AttachmentVisual,
) {
    let palette = theme::p();
    let border = match (&item.status, &item.delivery) {
        (PendingAttachmentStatus::Rejected { .. }, _) | (_, DeliveryState::Failed { .. }) => {
            palette.notice_fg
        }
        (_, DeliveryState::Delivered { .. }) => palette.accent,
        _ => palette.panel_border,
    };
    scene::fill(
        canvas,
        root,
        palette.chip_bg,
        &scene::rrect(
            layout.chip.x,
            layout.chip.y,
            layout.chip.w,
            layout.chip.h,
            6.0,
        ),
    );
    scene::stroke(
        canvas,
        root,
        border,
        1.0,
        &scene::rrect(
            layout.chip.x,
            layout.chip.y,
            layout.chip.w,
            layout.chip.h,
            6.0,
        ),
    );
    let preview_shape = scene::rrect(
        layout.preview.x,
        layout.preview.y,
        layout.preview.w,
        layout.preview.h,
        4.0,
    );
    if let Some(preview) = &item.preview {
        canvas.push_clip_layer(Fill::NonZero, root, &preview_shape);
        let scale = (layout.preview.w / preview.width.max(1) as f32)
            .max(layout.preview.h / preview.height.max(1) as f32);
        canvas.draw_image(
            &preview.brush,
            root * Affine::translate((
                (layout.preview.x + (layout.preview.w - preview.width as f32 * scale) * 0.5) as f64,
                (layout.preview.y + (layout.preview.h - preview.height as f32 * scale) * 0.5)
                    as f64,
            )) * Affine::scale(scale as f64),
        );
        canvas.pop_layer();
    } else {
        scene::fill(canvas, root, palette.row_hint, &preview_shape);
        let icon = engine.icon_layout(icons::IMAGE, 16.0);
        scene::draw_layout_first_line(
            canvas,
            root,
            &icon,
            layout.preview.x + (layout.preview.w - icon.width()) * 0.5,
            layout.preview.y + (layout.preview.h - icon.height()) * 0.5,
            Some(palette.text_dim),
        );
    }

    let status_rect = Rect::new(
        layout.preview.x + layout.preview.w + 5.0,
        layout.chip.y + 20.0,
        (layout.chip.w - layout.preview.w - 10.0).max(0.0),
        20.0,
    );
    draw_text(
        canvas,
        root,
        engine,
        attachment_label(item, copy),
        status_rect,
        LabelStyle {
            role: theme::role::hint(),
            colour: palette.text_dim,
            right_aligned: false,
        },
    );
    if visual.remove_hovered {
        scene::fill(
            canvas,
            root,
            palette.hover_tint,
            &scene::rrect(
                layout.remove.x,
                layout.remove.y,
                layout.remove.w,
                layout.remove.h,
                4.0,
            ),
        );
    }
    let close = engine.icon_layout(icons::WINDOW_CLOSE, 11.0);
    scene::draw_layout_first_line(
        canvas,
        root,
        &close,
        layout.remove.x + (layout.remove.w - close.width()) * 0.5,
        layout.remove.y + (layout.remove.h - close.height()) * 0.5,
        Some(if visual.remove_enabled {
            palette.text_dim
        } else {
            palette.placeholder
        }),
    );
}

fn draw_icon_button(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    rect: Rect,
    glyph: char,
    visual: ButtonVisual,
) {
    let palette = theme::p();
    let background = if visual.primary && visual.enabled {
        palette.accent
    } else if visual.hovered && visual.enabled {
        palette.hover_tint
    } else {
        palette.chip_bg
    };
    scene::fill(
        canvas,
        root,
        background,
        &scene::rrect(rect.x, rect.y, rect.w, rect.h, 7.0),
    );
    if !visual.primary {
        scene::stroke(
            canvas,
            root,
            palette.panel_border,
            1.0,
            &scene::rrect(rect.x, rect.y, rect.w, rect.h, 7.0),
        );
    }
    let icon = engine.icon_layout(glyph, 17.0);
    let colour = if !visual.enabled {
        palette.placeholder
    } else if visual.primary {
        palette.on_accent
    } else {
        palette.text
    };
    scene::draw_layout_first_line(
        canvas,
        root,
        &icon,
        rect.x + (rect.w - icon.width()) * 0.5,
        rect.y + (rect.h - icon.height()) * 0.5,
        Some(colour),
    );
}

fn draw_text(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    text: &str,
    rect: Rect,
    style: LabelStyle,
) {
    if rect.w <= 0.0 || rect.h <= 0.0 || text.is_empty() {
        return;
    }
    let value = engine.simple_layout(text, style.role);
    let x = if style.right_aligned {
        rect.x + rect.w - value.width()
    } else {
        rect.x
    };
    canvas.push_clip_layer(
        Fill::NonZero,
        root,
        &scene::rect(rect.x, rect.y, rect.w, rect.h),
    );
    scene::draw_layout_first_line(
        canvas,
        root,
        &value,
        x,
        rect.y + (rect.h - value.height()) * 0.5,
        Some(style.colour),
    );
    canvas.pop_layer();
}

fn delivery_label<'a>(model: &PendingInputModel, copy: &'a PendingInputCopy) -> Option<&'a str> {
    let mode = model
        .attachments()
        .iter()
        .find(|item| item.status == PendingAttachmentStatus::Ready)
        .map(|item| item.delivery_mode)?;
    Some(match mode {
        DeliveryMode::NativeStructured => &copy.delivery_native,
        DeliveryMode::QuotedPath => &copy.delivery_path,
        DeliveryMode::Unsupported => &copy.delivery_unsupported,
    })
}

fn attachment_label<'a>(item: &PendingAttachment, copy: &'a PendingInputCopy) -> &'a str {
    match (&item.status, &item.delivery) {
        (PendingAttachmentStatus::Ingesting, _) => &copy.ingesting,
        (PendingAttachmentStatus::Rejected { .. }, _) => &copy.rejected,
        (_, DeliveryState::Delivering { .. }) => &copy.delivering,
        (_, DeliveryState::Delivered { .. }) => &copy.delivered,
        (_, DeliveryState::Failed { .. }) => &copy.delivery_failed,
        (_, DeliveryState::Pending) => match item.delivery_mode {
            DeliveryMode::NativeStructured => &copy.native_short,
            DeliveryMode::QuotedPath => &copy.path_short,
            DeliveryMode::Unsupported => &copy.unsupported_short,
        },
    }
}

fn error_label<'a>(code: &ErrorCode, copy: &'a PendingInputCopy) -> &'a str {
    match code {
        ErrorCode::AttachmentLimit | ErrorCode::InputTooLarge => &copy.limit_error,
        ErrorCode::InvalidImage | ErrorCode::UnsupportedMedia => &copy.invalid_image,
        ErrorCode::DeliveryUnsupported => &copy.delivery_unsupported,
        ErrorCode::DeliveryFailed | ErrorCode::ProviderFailed | ErrorCode::ProviderUnavailable => {
            &copy.delivery_failed
        }
        ErrorCode::HostUnavailable => &copy.host_unavailable,
        _ => &copy.generic_error,
    }
}

fn visible_draft(value: &str) -> &str {
    let mut boundaries = value.char_indices().rev();
    let start = boundaries
        .nth(MAX_RENDERED_DRAFT_CHARS.saturating_sub(1))
        .map_or(0, |(index, _)| index);
    &value[start..]
}

fn draft_line<'a>(draft: &'a str, placeholder: &'a str, focused: bool) -> (&'a str, bool) {
    if draft.is_empty() && !focused {
        (placeholder, true)
    } else {
        (visible_draft(draft), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_unicode_draft_shapes_only_its_visible_tail() {
        let value = "界".repeat(MAX_RENDERED_DRAFT_CHARS + 20);
        let visible = visible_draft(&value);
        assert_eq!(visible.chars().count(), MAX_RENDERED_DRAFT_CHARS);
        assert!(value.ends_with(visible));
    }

    #[test]
    fn focusing_an_empty_draft_hides_the_placeholder_and_places_the_caret_at_the_start() {
        assert_eq!(
            draft_line("", "Write a message or command", false),
            ("Write a message or command", true)
        );
        assert_eq!(
            draft_line("", "Write a message or command", true),
            ("", false)
        );
        assert_eq!(
            draft_line("cd e:", "Write a message or command", true),
            ("cd e:", false)
        );
    }
}
