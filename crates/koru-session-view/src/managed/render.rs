// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use koru_draw::{scene, text::TextEngine, theme};
use peniko::Color;
use vello::{
    Scene,
    kurbo::{Affine, Rect},
};

use super::layout::{
    BUTTON_H, BUTTON_PAD_X, ITEM_GAP, ITEM_PAD_X, ITEM_PAD_Y, ManagedItemLayout, ManagedLayout,
    TRANSCRIPT_PAD_X, TRANSCRIPT_PAD_Y, approval_layout,
};
use super::model::{
    ManagedApprovalState, ManagedAttachmentState, ManagedItemContent, ManagedToolState,
    ManagedTranscriptItem, ManagedTurnState,
};
use super::{ApprovalFocus, ManagedSessionCopy};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManagedRenderWork {
    pub items_considered: u64,
    pub items_shaped: u64,
    pub cache_hits: u64,
    pub items_appended_to_scene: u64,
    pub cache_invalidations: u64,
    pub approval_layouts_shaped: u64,
}

pub(crate) struct ManagedRenderFrame<'a> {
    pub(crate) root: Affine,
    pub(crate) bounds: Rect,
    pub(crate) scale: f32,
    pub(crate) items: &'a std::collections::VecDeque<ManagedTranscriptItem>,
    pub(crate) copy: &'a ManagedSessionCopy,
    pub(crate) scroll_offset: &'a mut f32,
    pub(crate) approval_scroll: &'a mut f32,
    pub(crate) reveal_approval_focus: bool,
    pub(crate) follow_tail: bool,
    pub(crate) approval: Option<&'a ManagedTranscriptItem>,
    pub(crate) approval_focus: ApprovalFocus,
    pub(crate) active_turn: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ManagedStyleEpoch {
    dark: bool,
    density: theme::Density,
    font: theme::FontChoice,
    scale_bits: u32,
    width_bits: u32,
}

impl ManagedStyleEpoch {
    fn current(scale: f32, width: f32) -> Self {
        Self {
            dark: theme::is_dark(),
            density: theme::density(),
            font: theme::ui_font(),
            scale_bits: scale.to_bits(),
            width_bits: width.to_bits(),
        }
    }
}

struct CachedItem {
    epoch: ManagedStyleEpoch,
    fingerprint: u64,
    height: f32,
    scene: Scene,
}

struct CachedApproval {
    epoch: ManagedStyleEpoch,
    fingerprint: u64,
    title: parley::Layout<theme::Brush>,
    body: parley::Layout<theme::Brush>,
    labels: Vec<parley::Layout<theme::Brush>>,
}

#[derive(Default)]
pub(crate) struct ManagedRenderer {
    items: BTreeMap<u64, CachedItem>,
    approval: Option<CachedApproval>,
    copy_generation: u64,
}

impl ManagedRenderer {
    pub(crate) fn copy_changed(&mut self) {
        self.copy_generation = self.copy_generation.saturating_add(1);
        self.items.clear();
        self.approval = None;
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.approval = None;
    }

    pub(crate) fn draw(
        &mut self,
        engine: &mut TextEngine,
        scene: &mut Scene,
        frame: ManagedRenderFrame<'_>,
    ) -> (ManagedRenderWork, ManagedLayout) {
        let ManagedRenderFrame {
            root,
            bounds,
            scale,
            items,
            copy,
            scroll_offset,
            approval_scroll,
            reveal_approval_focus,
            follow_tail,
            approval,
            approval_focus,
            active_turn,
        } = frame;
        let mut work = ManagedRenderWork::default();
        let width = (bounds.width() as f32 - TRANSCRIPT_PAD_X * 2.0).max(1.0);
        let epoch = ManagedStyleEpoch::current(scale, width);
        let mut heights = Vec::with_capacity(items.len());
        for item in items {
            let height = self
                .items
                .get(&item.id)
                .filter(|cached| {
                    cached.epoch == epoch
                        && cached.fingerprint == fingerprint(item, self.copy_generation)
                })
                .map_or_else(
                    || estimate_height(item, copy, width),
                    |cached| cached.height,
                );
            heights.push(height);
        }
        let total_height = heights.iter().sum::<f32>()
            + ITEM_GAP * items.len().saturating_sub(1) as f32
            + TRANSCRIPT_PAD_Y * 2.0;
        let maximum_scroll = (total_height - bounds.height() as f32).max(0.0);
        if follow_tail {
            *scroll_offset = maximum_scroll;
        } else {
            *scroll_offset = scroll_offset.clamp(0.0, maximum_scroll);
        }

        scene::fill(scene, root, theme::p().bg, &bounds);
        let visible_top = bounds.y0 as f32 - theme::role::ui().metrics.line_height;
        let visible_bottom = bounds.y1 as f32 + theme::role::ui().metrics.line_height;
        let mut y = bounds.y0 as f32 + TRANSCRIPT_PAD_Y - *scroll_offset;
        let mut layout = ManagedLayout::default();
        for (index, item) in items.iter().enumerate() {
            let estimated = heights[index];
            let intersects = y + estimated >= visible_top && y <= visible_bottom;
            if intersects {
                work.items_considered = work.items_considered.saturating_add(1);
                let expected_fingerprint = fingerprint(item, self.copy_generation);
                let hit = self.items.get(&item.id).is_some_and(|cached| {
                    cached.epoch == epoch && cached.fingerprint == expected_fingerprint
                });
                if hit {
                    work.cache_hits = work.cache_hits.saturating_add(1);
                } else {
                    let cached = build_item(engine, item, copy, width, epoch, expected_fingerprint);
                    self.items.insert(item.id, cached);
                    work.items_shaped = work.items_shaped.saturating_add(1);
                }
                let cached = &self.items[&item.id];
                let item_bounds = Rect::new(
                    bounds.x0 + f64::from(TRANSCRIPT_PAD_X),
                    f64::from(y),
                    bounds.x0 + f64::from(TRANSCRIPT_PAD_X + width),
                    f64::from(y + cached.height),
                );
                scene.append(
                    &cached.scene,
                    Some(root * Affine::translate((item_bounds.x0, item_bounds.y0))),
                );
                layout.items.push(ManagedItemLayout {
                    item_id: item.id,
                    rect: item_bounds,
                });
                if let ManagedItemContent::Approval {
                    approval_id,
                    state: ManagedApprovalState::Pending,
                    ..
                } = item.content
                {
                    layout.review_approvals.push((approval_id, item_bounds));
                }
                work.items_appended_to_scene = work.items_appended_to_scene.saturating_add(1);
                heights[index] = cached.height;
            }
            y += heights[index] + ITEM_GAP;
        }
        let retained: std::collections::BTreeSet<_> = items.iter().map(|item| item.id).collect();
        self.items.retain(|id, _| retained.contains(id));

        if let Some(item) = approval {
            if let ManagedItemContent::Approval { choices, .. } = &item.content
                && !choices.is_empty()
                && !self.approval.as_ref().is_some_and(|cached| {
                    cached.epoch == epoch
                        && cached.fingerprint == fingerprint(item, self.copy_generation)
                })
            {
                work.approval_layouts_shaped = choices.len() as u64 + 2;
            }
            let approval_layout_value = draw_approval(
                engine,
                scene,
                root,
                bounds,
                item,
                copy,
                approval_focus,
                approval_scroll,
                reveal_approval_focus,
                &mut self.approval,
                epoch,
                self.copy_generation,
            );
            layout.approval = approval_layout_value;
        } else if active_turn {
            let label = engine.simple_layout(&copy.cancel_turn, theme::role::row());
            let width = label.width() + BUTTON_PAD_X * 2.0;
            let rect = Rect::new(
                bounds.x1 - f64::from(TRANSCRIPT_PAD_X + width),
                bounds.y1 - f64::from(TRANSCRIPT_PAD_Y + BUTTON_H),
                bounds.x1 - f64::from(TRANSCRIPT_PAD_X),
                bounds.y1 - f64::from(TRANSCRIPT_PAD_Y),
            );
            draw_button(
                engine,
                scene,
                root,
                rect,
                &copy.cancel_turn,
                false,
                false,
                false,
            );
            layout.cancel_turn = Some(rect);
        }

        (work, layout)
    }
}

fn build_item(
    engine: &mut TextEngine,
    item: &ManagedTranscriptItem,
    copy: &ManagedSessionCopy,
    width: f32,
    epoch: ManagedStyleEpoch,
    fingerprint: u64,
) -> CachedItem {
    let display = display_item(item, copy);
    let primary =
        engine.simple_layout_wrapped(&display.primary, display.role, width - ITEM_PAD_X * 2.0);
    let secondary = display.secondary.as_ref().map(|text| {
        engine.simple_layout_wrapped(text, theme::role::meta(), width - ITEM_PAD_X * 2.0)
    });
    let secondary_h = secondary
        .as_ref()
        .map_or(0.0, |layout| layout.height() + 4.0);
    let height = ITEM_PAD_Y * 2.0 + primary.height() + secondary_h;
    let mut cached_scene = Scene::new();
    if display.framed {
        scene::fill(
            &mut cached_scene,
            Affine::IDENTITY,
            theme::p().panel_bg,
            &scene::rrect(0.0, 0.0, width, height, 7.0),
        );
        scene::stroke(
            &mut cached_scene,
            Affine::IDENTITY,
            display.border,
            1.0,
            &scene::rrect(0.0, 0.0, width, height, 7.0),
        );
    }
    scene::draw_layout(
        &mut cached_scene,
        Affine::IDENTITY,
        &primary,
        ITEM_PAD_X,
        ITEM_PAD_Y,
        Some(display.colour),
    );
    if let Some(secondary) = secondary {
        scene::draw_layout(
            &mut cached_scene,
            Affine::IDENTITY,
            &secondary,
            ITEM_PAD_X,
            ITEM_PAD_Y + primary.height() + 4.0,
            Some(theme::p().text_dim),
        );
    }
    CachedItem {
        epoch,
        fingerprint,
        height,
        scene: cached_scene,
    }
}

struct DisplayItem {
    primary: String,
    secondary: Option<String>,
    role: theme::TextStyle,
    colour: Color,
    border: Color,
    framed: bool,
}

fn display_item(item: &ManagedTranscriptItem, copy: &ManagedSessionCopy) -> DisplayItem {
    let p = theme::p();
    let mut display = DisplayItem {
        primary: String::new(),
        secondary: None,
        role: theme::role::row(),
        colour: p.text,
        border: p.panel_border,
        framed: true,
    };
    match &item.content {
        ManagedItemContent::Message { text, .. } => {
            display.primary.clone_from(text);
            display.role = theme::role::ui();
            display.framed = false;
        }
        ManagedItemContent::Tool { label, state, .. } => {
            display.primary = copy.category(label);
            display.secondary = Some(
                match state {
                    ManagedToolState::Running => &copy.tool_running,
                    ManagedToolState::Completed => &copy.tool_completed,
                    ManagedToolState::Failed => &copy.tool_failed,
                }
                .clone(),
            );
            if *state == ManagedToolState::Failed {
                display.border = p.notice_fg;
            }
        }
        ManagedItemContent::Approval {
            summary,
            state,
            safely_reviewable,
            ..
        } => {
            display.primary.clone_from(&copy.approval_required);
            display.secondary = Some(if *safely_reviewable {
                copy.approval_summary(summary)
            } else {
                copy.approval_too_large.clone()
            });
            display.border = if matches!(state, ManagedApprovalState::Pending) {
                p.notice_fg
            } else {
                p.panel_border
            };
        }
        ManagedItemContent::Attachment { state, error, .. } => {
            display.primary = match state {
                ManagedAttachmentState::Accepted => copy.attachment_accepted.clone(),
                ManagedAttachmentState::Rejected => copy.attachment_rejected.clone(),
            };
            if let Some(code) = error {
                display.secondary = Some(copy.error(code));
                display.border = p.notice_fg;
            }
        }
        ManagedItemContent::Usage { usage, .. } => {
            display.primary.clone_from(&copy.usage);
            display.secondary = copy.format_usage(usage);
        }
        ManagedItemContent::TurnState {
            state,
            error,
            approval_unrepresentable,
            ..
        } => {
            display.primary = match state {
                ManagedTurnState::Running => copy.turn_running.clone(),
                ManagedTurnState::WaitingForInput => copy.waiting_for_input.clone(),
                ManagedTurnState::Completed => copy.turn_completed.clone(),
                ManagedTurnState::Cancelled => copy.turn_cancelled.clone(),
                ManagedTurnState::Failed => copy.turn_failed.clone(),
            };
            if let Some(code) = error {
                display.secondary = Some(if *approval_unrepresentable {
                    copy.approval_unrepresentable.clone()
                } else {
                    copy.error(code)
                });
                display.border = p.notice_fg;
            }
        }
        ManagedItemContent::ProviderState { state, .. } => {
            display.primary = copy.provider_state(*state);
        }
        ManagedItemContent::StreamGap { .. } => {
            display.primary.clone_from(&copy.stream_gap);
            display.border = p.notice_fg;
        }
        ManagedItemContent::Truncated => {
            display.primary.clone_from(&copy.transcript_truncated);
            display.colour = p.text_dim;
        }
    }
    display
}

fn estimate_height(item: &ManagedTranscriptItem, copy: &ManagedSessionCopy, width: f32) -> f32 {
    let display = display_item(item, copy);
    let usable = (width - ITEM_PAD_X * 2.0).max(1.0);
    let approximate_columns = (usable / 7.5).floor().max(1.0) as usize;
    let lines = display
        .primary
        .chars()
        .count()
        .div_ceil(approximate_columns)
        .max(1)
        + display.secondary.as_ref().map_or(0, |text| {
            text.chars().count().div_ceil(approximate_columns).max(1)
        });
    ITEM_PAD_Y * 2.0 + lines as f32 * theme::role::ui().metrics.line_height + 4.0
}

#[allow(clippy::too_many_arguments)]
fn draw_approval(
    engine: &mut TextEngine,
    canvas: &mut Scene,
    root: Affine,
    viewport: Rect,
    item: &ManagedTranscriptItem,
    copy: &ManagedSessionCopy,
    focus: ApprovalFocus,
    scroll: &mut f32,
    reveal_focus: bool,
    cache: &mut Option<CachedApproval>,
    epoch: ManagedStyleEpoch,
    copy_generation: u64,
) -> Option<super::layout::ManagedApprovalLayout> {
    let ManagedItemContent::Approval {
        summary,
        choices,
        state,
        safely_reviewable,
        ..
    } = &item.content
    else {
        return None;
    };
    if !matches!(
        state,
        ManagedApprovalState::Pending | ManagedApprovalState::Submitting(_)
    ) {
        return None;
    }
    if !choices.is_empty() {
        return Some(draw_dynamic_approval(
            engine,
            canvas,
            root,
            viewport,
            summary,
            choices,
            *state,
            *safely_reviewable,
            copy,
            focus,
            scroll,
            reveal_focus,
            cache,
            epoch,
            fingerprint(item, copy_generation),
        ));
    }
    let title = engine.simple_layout_wrapped(
        &copy.approval_required,
        theme::role::section_heading(),
        440.0,
    );
    let body_text = if matches!(state, ManagedApprovalState::Submitting(_)) {
        copy.approval_submitting.clone()
    } else if *safely_reviewable {
        copy.approval_summary(summary)
    } else {
        copy.approval_too_large.clone()
    };
    let body = engine.simple_layout_wrapped(&body_text, theme::role::ui(), 440.0);
    let reject_label = engine.simple_layout(&copy.reject, theme::role::row());
    let approve_label = engine.simple_layout(&copy.approve, theme::role::row());
    let reject_width = reject_label.width() + BUTTON_PAD_X * 2.0;
    let approve_width = approve_label.width() + BUTTON_PAD_X * 2.0;
    let panel_height = ITEM_PAD_Y * 3.0 + title.height() + body.height() + BUTTON_H + 12.0;
    let layout = approval_layout(viewport, panel_height, reject_width, approve_width);
    scene::fill(canvas, root, theme::p().scrim, &viewport);
    scene::floating_panel(
        canvas,
        root,
        layout.panel.x0 as f32,
        layout.panel.y0 as f32,
        layout.panel.width() as f32,
        layout.panel.height() as f32,
        &scene::Panel::popover(10.0),
    );
    scene::draw_layout(
        canvas,
        root,
        &title,
        layout.panel.x0 as f32 + ITEM_PAD_X,
        layout.panel.y0 as f32 + ITEM_PAD_Y,
        Some(theme::p().text),
    );
    scene::draw_layout(
        canvas,
        root,
        &body,
        layout.panel.x0 as f32 + ITEM_PAD_X,
        layout.panel.y0 as f32 + ITEM_PAD_Y + title.height() + 8.0,
        Some(theme::p().text_dim),
    );
    let submitting = matches!(state, ManagedApprovalState::Submitting(_));
    draw_button(
        engine,
        canvas,
        root,
        layout.reject,
        &copy.reject,
        focus == ApprovalFocus::Reject,
        submitting,
        false,
    );
    draw_button(
        engine,
        canvas,
        root,
        layout.approve,
        &copy.approve,
        focus == ApprovalFocus::Approve,
        submitting || !*safely_reviewable,
        true,
    );
    Some(layout)
}

#[allow(clippy::too_many_arguments)]
fn draw_dynamic_approval(
    engine: &mut TextEngine,
    canvas: &mut Scene,
    root: Affine,
    viewport: Rect,
    summary: &str,
    choices: &[koru_session_protocol::ApprovalChoice],
    state: ManagedApprovalState,
    safely_reviewable: bool,
    copy: &ManagedSessionCopy,
    focus: ApprovalFocus,
    scroll: &mut f32,
    reveal_focus: bool,
    cache: &mut Option<CachedApproval>,
    epoch: ManagedStyleEpoch,
    fingerprint: u64,
) -> super::layout::ManagedApprovalLayout {
    let width = super::layout::APPROVAL_WIDTH
        .min((viewport.width() as f32 - 2.0 * super::layout::APPROVAL_EDGE).max(1.0));
    let text_width = (width - ITEM_PAD_X * 4.0).max(1.0);
    let submitting = matches!(state, ManagedApprovalState::Submitting(_));
    if !cache
        .as_ref()
        .is_some_and(|cached| cached.epoch == epoch && cached.fingerprint == fingerprint)
    {
        let title = engine.simple_layout_wrapped(
            &copy.approval_required,
            theme::role::section_heading(),
            text_width,
        );
        let body_text = if submitting {
            copy.approval_submitting.clone()
        } else if safely_reviewable {
            copy.approval_summary(summary)
        } else {
            copy.approval_too_large.clone()
        };
        let body = engine.simple_layout_wrapped(&body_text, theme::role::ui(), text_width);
        let labels = choices
            .iter()
            .map(|option| {
                engine.simple_layout_wrapped(
                    if option.label.is_empty() {
                        copy.choice_label(option)
                    } else {
                        &option.label
                    },
                    theme::role::row(),
                    text_width,
                )
            })
            .collect();
        *cache = Some(CachedApproval {
            epoch,
            fingerprint,
            title,
            body,
            labels,
        });
    }
    let cached = cache.as_ref().expect("approval cache populated");
    let title = &cached.title;
    let body = &cached.body;
    let labels = &cached.labels;
    let content_height = body.height()
        + 12.0
        + labels
            .iter()
            .map(|label| label.height().max(BUTTON_H - 16.0) + 24.0)
            .sum::<f32>();
    let mut layout = approval_layout(
        viewport,
        title.height() + content_height + ITEM_PAD_Y * 4.0,
        0.0,
        0.0,
    );
    layout.reject = Rect::ZERO;
    layout.approve = Rect::ZERO;
    scene::fill(canvas, root, theme::p().scrim, &viewport);
    scene::floating_panel(
        canvas,
        root,
        layout.panel.x0 as f32,
        layout.panel.y0 as f32,
        layout.panel.width() as f32,
        layout.panel.height() as f32,
        &scene::Panel::popover(10.0),
    );
    canvas.push_layer(
        peniko::Fill::NonZero,
        peniko::Mix::Normal,
        1.0,
        root,
        &layout.panel,
    );
    scene::draw_layout(
        canvas,
        root,
        title,
        layout.panel.x0 as f32 + ITEM_PAD_X,
        layout.panel.y0 as f32 + ITEM_PAD_Y,
        Some(theme::p().text),
    );
    let left = (layout.panel.x0 + f64::from(ITEM_PAD_X)).min(layout.panel.x1);
    let right = (layout.panel.x1 - f64::from(ITEM_PAD_X)).max(left);
    let top = (layout.panel.y0 + f64::from(title.height() + ITEM_PAD_Y * 2.0)).min(layout.panel.y1);
    let bottom = (layout.panel.y1 - f64::from(ITEM_PAD_Y)).max(top);
    let clip = Rect::new(left, top, right, bottom);
    let maximum = (content_height - clip.height() as f32).max(0.0);
    if reveal_focus {
        let mut y = body.height() + 12.0;
        for (option, label) in choices.iter().zip(labels) {
            let height = label.height().max(BUTTON_H - 16.0) + 16.0;
            if focus == ApprovalFocus::Choice(option.id) {
                if y < *scroll {
                    *scroll = y;
                }
                if y + height > *scroll + clip.height() as f32 {
                    *scroll = y + height - clip.height() as f32;
                }
                break;
            }
            y += height + 8.0;
        }
    }
    *scroll = scroll.clamp(0.0, maximum);
    canvas.push_layer(peniko::Fill::NonZero, peniko::Mix::Normal, 1.0, root, &clip);
    let mut y = clip.y0 as f32 - *scroll;
    scene::draw_layout(
        canvas,
        root,
        body,
        clip.x0 as f32 + ITEM_PAD_X,
        y,
        Some(theme::p().text_dim),
    );
    y += body.height() + 12.0;
    for (index, (option, label)) in choices.iter().zip(labels).enumerate() {
        let height = label.height().max(BUTTON_H - 16.0) + 16.0;
        let rect = Rect::new(clip.x0, f64::from(y), clip.x1, f64::from(y + height));
        let focused = focus == ApprovalFocus::Choice(option.id)
            || (focus == ApprovalFocus::Reject
                && index
                    == choices
                        .iter()
                        .position(|option| !option.intent.grants())
                        .unwrap_or(0));
        let disabled = submitting || (option.intent.grants() && !safely_reviewable);
        scene::fill(canvas, root, theme::p().panel_bg, &rect);
        scene::stroke(
            canvas,
            root,
            if focused {
                theme::p().focus_ring
            } else {
                theme::p().panel_border
            },
            if focused { 2.0 } else { 1.0 },
            &rect,
        );
        scene::draw_layout(
            canvas,
            root,
            label,
            rect.x0 as f32 + ITEM_PAD_X,
            y + 8.0,
            Some(if disabled {
                theme::p().placeholder
            } else {
                theme::p().text
            }),
        );
        let hit = rect.intersect(clip);
        if hit.width() > 0.0 && hit.height() > 0.0 {
            layout.choices.push((option.id, hit));
        }
        y += height + 8.0;
    }
    canvas.pop_layer();
    canvas.pop_layer();
    layout
}

#[allow(clippy::too_many_arguments)]
fn draw_button(
    engine: &mut TextEngine,
    canvas: &mut Scene,
    root: Affine,
    rect: Rect,
    label: &str,
    focused: bool,
    disabled: bool,
    consequential: bool,
) {
    let p = theme::p();
    scene::fill(
        canvas,
        root,
        p.panel_bg,
        &scene::rrect(
            rect.x0 as f32,
            rect.y0 as f32,
            rect.width() as f32,
            rect.height() as f32,
            6.0,
        ),
    );
    scene::stroke(
        canvas,
        root,
        if focused {
            p.focus_ring
        } else if consequential {
            p.close_hover
        } else {
            p.panel_border
        },
        if focused { 2.0 } else { 1.0 },
        &scene::rrect(
            rect.x0 as f32,
            rect.y0 as f32,
            rect.width() as f32,
            rect.height() as f32,
            6.0,
        ),
    );
    let value = engine.simple_layout(label, theme::role::row());
    scene::draw_layout_first_line(
        canvas,
        root,
        &value,
        rect.x0 as f32 + (rect.width() as f32 - value.width()) * 0.5,
        rect.y0 as f32 + (rect.height() as f32 - value.height()) * 0.5,
        Some(if disabled {
            p.placeholder
        } else if consequential {
            p.close_hover
        } else {
            p.text
        }),
    );
}

fn fingerprint(item: &ManagedTranscriptItem, copy_generation: u64) -> u64 {
    item.id
        .rotate_left(17)
        .wrapping_add(item.revision.rotate_left(7))
        .wrapping_add(copy_generation)
}
