// SPDX-License-Identifier: MPL-2.0

//! The "All Reminders" management surface: every reminder in the profile,
//! filterable and scrollable, with per-row actions, health and failure
//! banners, a clear-history footer, and the snooze duration chooser.
//!
//! This is a *component*, not chrome. A host hands it a content rect and it
//! fills that rect; it knows nothing about the bell, popover anchoring, or
//! height clamps — those belong to the host. Today the only host is the
//! caption shell (`reminder_shell.rs`), which shows the surface as its
//! `Page::All` with a back arrow and a close × ([`Chrome::Popover`]). A full
//! content-page tab can host the same surface bare ([`Chrome::Page`]).
//!
//! Presentation data — labels, ordering, measured widths — is owned by the
//! shell's shared `Presentation` and borrowed here; this module owns only
//! transient interaction state (filter, scroll position, hover/focus, the
//! open snooze chooser) plus the one geometry snapshot rebuilt from it during
//! maintenance. It reports `Intent`s; acting on them is the container's
//! business, exactly as it is for the attention page.

use std::time::Instant;

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{icons, scene, theme};
use koru_editor::reminders::{
    GlobalFilter, GlobalItem, GlobalItemState, GlobalManagement, Snooze, SnoozeIncrement,
};
use koru_i18n::{Key, Lang, t};
use koru_ui::{ActionButton, ActionStyle, Chip};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key as KeyboardKey, NamedKey};

use crate::reminder_settings::HealthPresentation;

use super::reminder_shell::{Intent, Presentation, draw_text, draw_wrapped_text, filter_slot};

pub(super) const OVERLAY_PAD: f32 = 16.0;
const FILTER_H: f32 = Chip::H;
const FILTER_GAP: f32 = 6.0;
const OVERLAY_ROW_H: f32 = 150.0;
pub(super) const ACTION_H: f32 = 24.0;
pub(super) const ACTION_GAP: f32 = 4.0;
const SNOOZE_COLUMNS: usize = 2;
const SNOOZE_GRID_TOP: f32 = 62.0;
const SNOOZE_PREVIEW_TOP: f32 = 172.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    All,
    NeedsAttention,
    ComingUp,
    Snoozed,
    Paused,
    History,
}

impl Filter {
    pub(super) const ALL: [Self; 6] = [
        Self::All,
        Self::NeedsAttention,
        Self::ComingUp,
        Self::Snoozed,
        Self::Paused,
        Self::History,
    ];

    pub(super) fn key(self) -> Key {
        match self {
            Self::All => Key::ReminderFilterAll,
            Self::NeedsAttention => Key::ReminderNeedsAttention,
            Self::ComingUp => Key::ReminderComingUp,
            Self::Snoozed => Key::ReminderFilterSnoozed,
            Self::Paused => Key::ReminderFilterPaused,
            Self::History => Key::ReminderHistory,
        }
    }
}

impl From<GlobalFilter> for Filter {
    fn from(value: GlobalFilter) -> Self {
        match value {
            GlobalFilter::All => Self::All,
            GlobalFilter::NeedsAttention => Self::NeedsAttention,
            GlobalFilter::ComingUp => Self::ComingUp,
            GlobalFilter::Snoozed => Self::Snoozed,
            GlobalFilter::Paused => Self::Paused,
            GlobalFilter::History => Self::History,
        }
    }
}

impl From<Filter> for GlobalFilter {
    fn from(value: Filter) -> Self {
        match value {
            Filter::All => Self::All,
            Filter::NeedsAttention => Self::NeedsAttention,
            Filter::ComingUp => Self::ComingUp,
            Filter::Snoozed => Self::Snoozed,
            Filter::Paused => Self::Paused,
            Filter::History => Self::History,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum RowAction {
    Open,
    Snooze,
    Handled,
    Edit,
    Recreate,
    PauseResume,
    Delete,
}

impl RowAction {
    // Effective Paused also covers an unavailable source. Only the authored
    // rule status decides whether this control can usefully Pause or Resume.
    fn key(self, rule_paused: bool) -> Key {
        match self {
            Self::Open => Key::ReminderNotificationOpen,
            Self::Snooze => Key::ReminderSnooze,
            Self::Handled => Key::ReminderNotificationHandled,
            Self::Edit => Key::ReminderEdit,
            Self::Recreate => Key::ReminderRecreate,
            Self::PauseResume if rule_paused => Key::ReminderResume,
            Self::PauseResume => Key::ReminderPause,
            Self::Delete => Key::ReminderDelete,
        }
    }

    pub(super) const fn width_slot(self, rule_paused: bool) -> usize {
        match self {
            Self::Open => 0,
            Self::Snooze => 1,
            Self::Handled => 2,
            Self::Edit => 3,
            Self::Recreate => 7,
            Self::PauseResume if rule_paused => 5,
            Self::PauseResume => 4,
            Self::Delete => 6,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    Back,
    Filter(Filter),
    Close,
    Row(usize),
    RowAction(usize, RowAction),
    ClearHistory,
    ConfirmClearHistory,
    CancelClearHistory,
    SnoozeAdjust(SnoozeIncrement, bool),
    SnoozeConfirm,
    SnoozeCancel,
    HealthAction,
    RetryFailed,
    DiscardFailed,
    Overlay,
}

/// Which host chrome the surface draws and hit-tests. The bell popover hosts
/// it with a back arrow and a close ×; a full content page hosts it bare,
/// because the page's own chrome owns navigation and dismissal.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Chrome {
    /// Inside the bell popover: back returns to the attention page, × closes.
    #[default]
    Popover,
    /// A full content page: the host owns navigation; no back arrow, no ×.
    /// Constructed by the page host; the bell popover always passes `Popover`.
    #[allow(dead_code)]
    Page,
}

impl Chrome {
    fn back_close(self) -> bool {
        matches!(self, Self::Popover)
    }
}

/// A navigation request only the host can fulfil: the component has no page
/// to go back to and no surface to close.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Request {
    Back,
    Close,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Geometry {
    /// The content rect the host supplied; `None` while the surface is not
    /// shown, so stale geometry never answers a hit.
    pub(super) rect: Option<Rect>,
    chrome: Chrome,
    back: Option<Rect>,
    close: Option<Rect>,
    filters: Vec<(Filter, Rect)>,
    rows: Vec<(usize, Rect)>,
    row_actions: Vec<(usize, Vec<(RowAction, Rect)>)>,
    clear_history: Option<Rect>,
    clear_confirm: Option<[Rect; 2]>,
    health_action: Option<Rect>,
    mutation_failure_actions: Option<[Rect; 2]>,
    snooze: Option<Rect>,
    snooze_increments: Vec<(SnoozeIncrement, [Rect; 3])>,
    snooze_footer: Option<[Rect; 2]>,
    empty: Option<Rect>,
    focus_order: Vec<Hit>,
}

impl Geometry {
    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if self.snooze.is_some() {
            for (increment, [minus, _, plus]) in &self.snooze_increments {
                if minus.contains(x, y) {
                    return Some(Hit::SnoozeAdjust(*increment, false));
                }
                if plus.contains(x, y) {
                    return Some(Hit::SnoozeAdjust(*increment, true));
                }
            }
            if let Some([cancel, confirm]) = self.snooze_footer {
                if cancel.contains(x, y) {
                    return Some(Hit::SnoozeCancel);
                }
                if confirm.contains(x, y) {
                    return Some(Hit::SnoozeConfirm);
                }
            }
            return self
                .rect
                .filter(|rect| rect.contains(x, y))
                .map(|_| Hit::Overlay);
        }
        for (filter, rect) in &self.filters {
            if rect.contains(x, y) {
                return Some(Hit::Filter(*filter));
            }
        }
        if self.close.is_some_and(|rect| rect.contains(x, y)) {
            return Some(Hit::Close);
        }
        if self.back.is_some_and(|rect| rect.contains(x, y)) {
            return Some(Hit::Back);
        }
        if self.health_action.is_some_and(|rect| rect.contains(x, y)) {
            return Some(Hit::HealthAction);
        }
        if let Some([retry, discard]) = self.mutation_failure_actions {
            if retry.contains(x, y) {
                return Some(Hit::RetryFailed);
            }
            if discard.contains(x, y) {
                return Some(Hit::DiscardFailed);
            }
        }
        if let Some([cancel, confirm]) = self.clear_confirm {
            if cancel.contains(x, y) {
                return Some(Hit::CancelClearHistory);
            }
            if confirm.contains(x, y) {
                return Some(Hit::ConfirmClearHistory);
            }
        }
        if self.clear_history.is_some_and(|rect| rect.contains(x, y)) {
            return Some(Hit::ClearHistory);
        }
        for (index, actions) in &self.row_actions {
            for (action, rect) in actions {
                if rect.contains(x, y) {
                    return Some(Hit::RowAction(*index, *action));
                }
            }
        }
        for (index, rect) in &self.rows {
            if rect.contains(x, y) {
                return Some(Hit::Row(*index));
            }
        }
        if self.rect.is_some_and(|rect| rect.contains(x, y)) {
            return Some(Hit::Overlay);
        }
        None
    }

    fn focus_order(&self) -> &[Hit] {
        &self.focus_order
    }

    fn rebuild_focus_order(&mut self) {
        self.focus_order.clear();
        if self.snooze.is_some() {
            for (increment, _) in &self.snooze_increments {
                self.focus_order.push(Hit::SnoozeAdjust(*increment, false));
                self.focus_order.push(Hit::SnoozeAdjust(*increment, true));
            }
            if self.snooze_footer.is_some() {
                self.focus_order
                    .extend([Hit::SnoozeCancel, Hit::SnoozeConfirm]);
            }
            return;
        }
        if self.rect.is_none() {
            return;
        }
        if self.back.is_some() {
            self.focus_order.push(Hit::Back);
        }
        if self.close.is_some() {
            self.focus_order.push(Hit::Close);
        }
        self.focus_order
            .extend(self.filters.iter().map(|(filter, _)| Hit::Filter(*filter)));
        for (index, _) in &self.rows {
            self.focus_order.push(Hit::Row(*index));
            if let Some((_, actions)) = self
                .row_actions
                .iter()
                .find(|(candidate, _)| candidate == index)
            {
                self.focus_order.extend(
                    actions
                        .iter()
                        .map(|(action, _)| Hit::RowAction(*index, *action)),
                );
            }
        }
        if self.clear_history.is_some() {
            self.focus_order.push(Hit::ClearHistory);
        }
        if self.clear_confirm.is_some() {
            self.focus_order
                .extend([Hit::CancelClearHistory, Hit::ConfirmClearHistory]);
        }
        if self.health_action.is_some() {
            self.focus_order.push(Hit::HealthAction);
        }
        if self.mutation_failure_actions.is_some() {
            self.focus_order
                .extend([Hit::RetryFailed, Hit::DiscardFailed]);
        }
    }
}

fn has_banner(health: Option<&HealthPresentation>, mutation_failure: bool) -> bool {
    mutation_failure || health.is_some_and(|health| health.banner.is_some())
}

fn banner_height(
    banner_text_h: f32,
    health: Option<&HealthPresentation>,
    mutation_failure: bool,
) -> f32 {
    if !has_banner(health, mutation_failure) {
        return 0.0;
    }
    let action_h = if mutation_failure || health.is_some_and(|health| health.action.is_some()) {
        ACTION_H + 6.0
    } else {
        0.0
    };
    (banner_text_h + action_h + 16.0).max(48.0)
}

/// The hostable "All Reminders" surface. Everything it shows is injected per
/// call — semantic rows, shared presentation, health, availability flags — so
/// the host's copies stay the only copies; what the component owns is the
/// transient interaction state and the geometry snapshot built from it.
pub(super) struct Management {
    pub(super) filter: Filter,
    scroll_row: usize,
    pub(super) snooze: Option<(GlobalItem, Snooze)>,
    clear_history_confirmation: bool,
    hover: Option<Hit>,
    pub(super) focus: Option<Hit>,
    /// The wall-clock sample the host captured during maintenance. The snooze
    /// preview and its commit read only this cached instant, so a confirmed
    /// snooze is exactly its preview.
    pub(super) now_ms: i64,
    clock_deadline: Option<Instant>,
    /// The host chrome the last geometry rebuild was built with, kept beside
    /// the snapshot so input handling agrees with it even before the first
    /// maintenance (when the geometry is still empty).
    chrome: Chrome,
    pub(super) geometry: Geometry,
    /// Set by every state change the geometry depends on; the host folds it
    /// into its own dirty check so one maintenance pass rebuilds both.
    pub(super) geometry_dirty: bool,
}

impl Default for Management {
    fn default() -> Self {
        Self {
            filter: Filter::default(),
            scroll_row: 0,
            snooze: None,
            clear_history_confirmation: false,
            hover: None,
            focus: None,
            now_ms: 0,
            clock_deadline: None,
            chrome: Chrome::default(),
            geometry: Geometry::default(),
            geometry_dirty: true,
        }
    }
}

impl Management {
    /// Reconcile with the semantic filter the Store published. A change the
    /// surface did not initiate restarts the list from the top.
    pub(super) fn sync_filter(&mut self, filter: GlobalFilter) {
        let filter = Filter::from(filter);
        if self.filter != filter {
            self.scroll_row = 0;
        }
        self.filter = filter;
    }

    /// Inject the host's wall-clock sample. Returns whether the minute rolled
    /// while the chooser is live, which is the host's cue to rebuild the
    /// shared presentation.
    pub(super) fn set_now_ms(&mut self, now_ms: i64) -> bool {
        let old_minute = self.now_ms.div_euclid(60_000);
        self.now_ms = now_ms;
        self.snooze.is_some() && old_minute != now_ms.div_euclid(60_000)
    }

    /// Whether the chooser's cached preview is due a refresh: it is live and
    /// its minute deadline has passed (or was never armed).
    pub(super) fn clock_refresh_due(&self, now: Instant) -> bool {
        self.snooze.is_some() && self.clock_deadline.is_none_or(|deadline| deadline <= now)
    }

    /// Arm the next minute boundary while the chooser is live. The event loop
    /// can sleep when it is closed.
    pub(super) fn arm_clock_deadline(&mut self, now: Instant, now_ms: i64) {
        self.clock_deadline = self.snooze.as_ref().map(|_| {
            let remaining = 60_000 - now_ms.rem_euclid(60_000) as u64;
            now + std::time::Duration::from_millis(remaining.max(1))
        });
    }

    pub(super) fn clock_deadline(&self) -> Option<Instant> {
        self.clock_deadline
    }

    pub(super) fn open_snooze(&mut self, row: GlobalItem, now: Instant) {
        self.snooze = Some((row, Snooze::new()));
        self.clock_deadline = Some(now);
        self.hover = None;
        self.focus = None;
        self.geometry_dirty = true;
    }

    /// The host navigated away: the list restarts and every transient closes.
    pub(super) fn leave(&mut self) {
        self.scroll_row = 0;
        self.snooze = None;
        self.clock_deadline = None;
        self.clear_history_confirmation = false;
        self.hover = None;
        self.focus = None;
        self.geometry_dirty = true;
    }

    /// The host dismissed the surface: modal state closes; the list position
    /// survives a reopen.
    pub(super) fn dismiss(&mut self) {
        self.snooze = None;
        self.clock_deadline = None;
        self.clear_history_confirmation = false;
        self.geometry_dirty = true;
    }

    pub(super) fn clear_hover_focus(&mut self) {
        self.hover = None;
        self.focus = None;
    }

    pub(super) fn apply_open_all(&mut self) {
        self.filter = Filter::All;
        self.scroll_row = 0;
        self.focus = None;
        self.geometry_dirty = true;
    }

    pub(super) fn apply_filter(&mut self, filter: GlobalFilter) {
        self.filter = filter.into();
        self.scroll_row = 0;
        self.geometry_dirty = true;
    }

    /// The content rect the host last supplied, while the surface is shown.
    pub(super) fn content_rect(&self) -> Option<Rect> {
        self.geometry.rect
    }

    /// Rebuild the one geometry snapshot from the host-supplied content rect
    /// and inputs. All collection growth happens here, during maintenance,
    /// never while a frame is drawn or an event is routed.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn rebuild_geometry(
        &mut self,
        rect: Option<Rect>,
        chrome: Chrome,
        global: &GlobalManagement,
        presentation: &Presentation,
        health: Option<&HealthPresentation>,
        mutation_failure: bool,
        edit_available: bool,
        recreate_available: bool,
    ) {
        self.geometry = self.build_geometry(
            rect,
            chrome,
            global,
            presentation,
            health,
            mutation_failure,
            edit_available,
            recreate_available,
        );
        self.chrome = chrome;
        self.geometry_dirty = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn build_geometry(
        &self,
        rect: Option<Rect>,
        chrome: Chrome,
        global: &GlobalManagement,
        presentation: &Presentation,
        health: Option<&HealthPresentation>,
        mutation_failure: bool,
        edit_available: bool,
        recreate_available: bool,
    ) -> Geometry {
        let mut filters = Vec::new();
        let mut back = None;
        let mut close = None;
        let mut rows = Vec::new();
        let mut row_actions = Vec::new();
        let mut clear_history = None;
        let mut clear_confirm = None;
        let mut health_action = None;
        let mut mutation_failure_actions = None;
        let mut snooze = None;
        let mut snooze_increments = Vec::new();
        let mut snooze_footer = None;
        let mut empty = None;
        if let Some(rect) = rect {
            if chrome.back_close() {
                back = Some(Rect::new(rect.x + 8.0, rect.y + 8.0, 32.0, 32.0));
                close = Some(Rect::new(rect.x + rect.w - 40.0, rect.y + 8.0, 32.0, 32.0));
            }
            let banner_h = banner_height(presentation.banner_text_h, health, mutation_failure);
            let action_y = rect.y + 52.0 + 8.0 + presentation.banner_text_h + 6.0;
            if banner_h > 0.0
                && !mutation_failure
                && health.is_some_and(|health| health.action.is_some())
            {
                health_action = Some(Rect::new(
                    rect.x + rect.w - OVERLAY_PAD - 124.0,
                    action_y,
                    116.0,
                    ACTION_H,
                ));
            }
            if mutation_failure {
                mutation_failure_actions = Some([
                    Rect::new(
                        rect.x + rect.w - OVERLAY_PAD - 214.0,
                        action_y,
                        96.0,
                        ACTION_H,
                    ),
                    Rect::new(
                        rect.x + rect.w - OVERLAY_PAD - 112.0,
                        action_y,
                        104.0,
                        ACTION_H,
                    ),
                ]);
            }
            let top = rect.y + 52.0 + if banner_h > 0.0 { banner_h + 8.0 } else { 0.0 };
            let left = rect.x + OVERLAY_PAD;
            let right = rect.x + rect.w - OVERLAY_PAD;
            let available_w = (right - left).max(1.0);
            let mut filter_x = left;
            let mut filter_y = top;
            for (index, filter) in Filter::ALL.iter().enumerate() {
                let width = presentation.filter_widths[index].min(available_w);
                if filter_x > left && filter_x + width > right {
                    filter_x = left;
                    filter_y += FILTER_H + FILTER_GAP;
                }
                filters.push((*filter, Rect::new(filter_x, filter_y, width, FILTER_H)));
                filter_x += width + FILTER_GAP;
            }
            let list_top = filter_y + FILTER_H + 12.0;
            let footer_h = 34.0;
            let list_bottom = rect.y + rect.h - OVERLAY_PAD - footer_h;
            let visible = self.visible_indices(presentation);
            let capacity = ((list_bottom - list_top) / OVERLAY_ROW_H).floor().max(0.0) as usize;
            let start = self.scroll_row.min(visible.len().saturating_sub(capacity));
            for (position, index) in visible.iter().skip(start).enumerate() {
                let y = list_top + position as f32 * OVERLAY_ROW_H;
                if y + OVERLAY_ROW_H > list_bottom {
                    break;
                }
                let row = Rect::new(
                    rect.x + OVERLAY_PAD,
                    y,
                    rect.w - OVERLAY_PAD * 2.0,
                    OVERLAY_ROW_H - 4.0,
                );
                let mut actions = Vec::new();
                let semantic = &global.rows[*index];
                let action_kinds: &[RowAction] = match semantic.state {
                    GlobalItemState::History => &[RowAction::Open, RowAction::Recreate],
                    GlobalItemState::NeedsAttention => &[
                        RowAction::Snooze,
                        RowAction::Handled,
                        RowAction::Edit,
                        RowAction::PauseResume,
                        RowAction::Delete,
                    ],
                    GlobalItemState::ComingUp | GlobalItemState::Snoozed => &[
                        RowAction::Snooze,
                        RowAction::Edit,
                        RowAction::PauseResume,
                        RowAction::Delete,
                    ],
                    GlobalItemState::Paused => {
                        &[RowAction::Edit, RowAction::PauseResume, RowAction::Delete]
                    }
                };
                let left = row.x + 8.0;
                let mut right = row.x + row.w - 8.0;
                let mut y = row.y + row.h - ACTION_H - 5.0;
                for action in action_kinds.iter().rev() {
                    if (*action == RowAction::Edit && !edit_available)
                        || (*action == RowAction::Recreate && !recreate_available)
                    {
                        continue;
                    }
                    let width = presentation.action_widths[action.width_slot(semantic.rule_paused)];
                    if right - width < left {
                        right = row.x + row.w - 8.0;
                        y -= ACTION_H + ACTION_GAP;
                    }
                    let x = right - width;
                    actions.push((*action, Rect::new(x, y, width, ACTION_H)));
                    right = x - ACTION_GAP;
                }
                actions.reverse();
                rows.push((*index, row));
                row_actions.push((*index, actions));
            }
            if visible.is_empty() {
                empty = Some(Rect::new(
                    rect.x + OVERLAY_PAD,
                    list_top,
                    rect.w - OVERLAY_PAD * 2.0,
                    (list_bottom - list_top).max(0.0),
                ));
            }
            if self.filter == Filter::History {
                if self.clear_history_confirmation {
                    let base = Rect::new(
                        rect.x + OVERLAY_PAD,
                        list_bottom,
                        rect.w - OVERLAY_PAD * 2.0,
                        ACTION_H,
                    );
                    clear_confirm = Some([
                        Rect::new(base.x, base.y, 84.0, base.h),
                        Rect::new(base.x + 90.0, base.y, 124.0, base.h),
                    ]);
                } else {
                    clear_history = Some(Rect::new(
                        rect.x + OVERLAY_PAD,
                        list_bottom,
                        136.0,
                        ACTION_H,
                    ));
                }
            }
            if self.snooze.is_some() {
                let chooser = Rect::new(
                    rect.x + 28.0,
                    rect.y + (rect.h - 260.0).max(0.0) * 0.5,
                    (rect.w - 56.0).max(1.0),
                    260.0_f32.min(rect.h),
                );
                snooze = Some(chooser);
                let button_w =
                    (chooser.w - 32.0 - ACTION_GAP * (SNOOZE_COLUMNS.saturating_sub(1)) as f32)
                        / SNOOZE_COLUMNS as f32;
                for (index, increment) in SnoozeIncrement::ALL.iter().enumerate() {
                    let col = index % SNOOZE_COLUMNS;
                    let row = index / SNOOZE_COLUMNS;
                    let cell = Rect::new(
                        chooser.x + 16.0 + col as f32 * (button_w + ACTION_GAP),
                        chooser.y + SNOOZE_GRID_TOP + row as f32 * (ACTION_H + ACTION_GAP),
                        button_w,
                        ACTION_H,
                    );
                    let adjust_w = 24.0_f32.min(cell.w * 0.28);
                    snooze_increments.push((
                        *increment,
                        [
                            Rect::new(cell.x, cell.y, adjust_w, cell.h),
                            Rect::new(cell.x + adjust_w, cell.y, cell.w - adjust_w * 2.0, cell.h),
                            Rect::new(cell.x + cell.w - adjust_w, cell.y, adjust_w, cell.h),
                        ],
                    ));
                }
                snooze_footer = Some([
                    Rect::new(
                        chooser.x + 16.0,
                        chooser.y + chooser.h - 40.0,
                        90.0,
                        ACTION_H,
                    ),
                    Rect::new(
                        chooser.x + chooser.w - 126.0,
                        chooser.y + chooser.h - 40.0,
                        110.0,
                        ACTION_H,
                    ),
                ]);
            }
        }
        let mut geometry = Geometry {
            rect,
            chrome,
            back,
            close,
            filters,
            rows,
            row_actions,
            clear_history,
            clear_confirm,
            health_action,
            mutation_failure_actions,
            snooze,
            snooze_increments,
            snooze_footer,
            empty,
            focus_order: Vec::new(),
        };
        geometry.rebuild_focus_order();
        geometry
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        lang: Lang,
        global: &GlobalManagement,
        presentation: &Presentation,
        health: Option<&HealthPresentation>,
        mutation_failure: bool,
    ) {
        let geometry = &self.geometry;
        let Some(rect) = geometry.rect else { return };
        if let Some(back) = geometry.back {
            koru_ui::icon_button::draw(
                canvas,
                root,
                engine,
                back,
                icons::CARET_LEFT,
                koru_ui::icon_button::State {
                    hovered: self.hover == Some(Hit::Back) || self.focus == Some(Hit::Back),
                    active: false,
                    disabled: false,
                },
            );
        }
        let title_x = if geometry.chrome.back_close() {
            rect.x + 42.0
        } else {
            rect.x + OVERLAY_PAD
        };
        draw_text(
            canvas,
            root,
            engine,
            t(lang, Key::ReminderAll),
            title_x,
            rect.y + 14.0,
            theme::role::section_heading(),
            theme::p().text,
        );
        if let Some(close) = geometry.close {
            koru_ui::icon_button::draw(
                canvas,
                root,
                engine,
                close,
                icons::WINDOW_CLOSE,
                koru_ui::icon_button::State {
                    hovered: self.hover == Some(Hit::Close) || self.focus == Some(Hit::Close),
                    active: false,
                    disabled: false,
                },
            );
        }
        for (filter, filter_rect) in &geometry.filters {
            Chip::new(t(lang, filter.key()), self.filter == *filter).draw(
                canvas,
                root,
                engine,
                *filter_rect,
                self.hover == Some(Hit::Filter(*filter)),
                self.focus == Some(Hit::Filter(*filter)),
            );
        }
        for (index, row_rect) in &geometry.rows {
            let row = &global.rows[*index];
            let revealed = self.row_actions_revealed(*index);
            if self.hover == Some(Hit::Row(*index)) || self.focus == Some(Hit::Row(*index)) {
                scene::fill(
                    canvas,
                    root,
                    theme::p().hover_tint,
                    &scene::rrect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 6.0),
                );
            }
            draw_text(
                canvas,
                root,
                engine,
                t(lang, state_key(row.state)),
                row_rect.x + 8.0,
                row_rect.y + 4.0,
                theme::role::hint(),
                if row.state == GlobalItemState::NeedsAttention {
                    theme::p().accent
                } else {
                    theme::p().row_hint
                },
            );
            draw_text(
                canvas,
                root,
                engine,
                &row.title,
                row_rect.x + 8.0,
                row_rect.y + 20.0,
                theme::role::row(),
                theme::p().text,
            );
            draw_text(
                canvas,
                root,
                engine,
                row_source_location(row),
                row_rect.x + 8.0,
                row_rect.y + 42.0,
                theme::role::meta(),
                theme::p().text_dim,
            );
            draw_text(
                canvas,
                root,
                engine,
                presentation
                    .schedule_labels
                    .get(*index)
                    .map_or("", String::as_str),
                row_rect.x + 8.0,
                row_rect.y + 60.0,
                theme::role::meta(),
                theme::p().text_dim,
            );
            if revealed
                && let Some((_, actions)) = geometry
                    .row_actions
                    .iter()
                    .find(|(candidate, _)| candidate == index)
            {
                for (action, action_rect) in actions {
                    let style = match action {
                        RowAction::Handled => ActionStyle::Primary,
                        RowAction::Snooze => ActionStyle::Secondary,
                        RowAction::Delete => ActionStyle::Destructive,
                        RowAction::Open
                        | RowAction::Edit
                        | RowAction::Recreate
                        | RowAction::PauseResume => ActionStyle::Ghost,
                    };
                    ActionButton::new(t(lang, action.key(row.rule_paused)), style, None).draw(
                        canvas,
                        root,
                        engine,
                        *action_rect,
                        self.hover == Some(Hit::RowAction(*index, *action))
                            || self.focus == Some(Hit::RowAction(*index, *action)),
                    );
                }
            }
        }
        if let Some(empty) = geometry.empty {
            let label = engine.simple_layout(t(lang, Key::ReminderAllEmpty), theme::role::ui());
            scene::draw_layout(
                canvas,
                root,
                &label,
                empty.x + (empty.w - label.width()) * 0.5,
                empty.y + (empty.h - label.height()) * 0.5,
                Some(theme::p().text_dim),
            );
        }
        if let Some(clear) = geometry.clear_history {
            ActionButton::new(t(lang, Key::ReminderClearHistory), ActionStyle::Ghost, None).draw(
                canvas,
                root,
                engine,
                clear,
                self.hover == Some(Hit::ClearHistory) || self.focus == Some(Hit::ClearHistory),
            );
        }
        if let Some([cancel, confirm]) = geometry.clear_confirm {
            ActionButton::new(t(lang, Key::ReminderCancel), ActionStyle::Ghost, None).draw(
                canvas,
                root,
                engine,
                cancel,
                self.hover == Some(Hit::CancelClearHistory)
                    || self.focus == Some(Hit::CancelClearHistory),
            );
            ActionButton::new(
                t(lang, Key::ReminderClearHistory),
                ActionStyle::Primary,
                None,
            )
            .draw(
                canvas,
                root,
                engine,
                confirm,
                self.hover == Some(Hit::ConfirmClearHistory)
                    || self.focus == Some(Hit::ConfirmClearHistory),
            );
        }
        if mutation_failure {
            let banner_rect = Rect::new(
                rect.x + OVERLAY_PAD,
                rect.y + 52.0,
                rect.w - OVERLAY_PAD * 2.0,
                banner_height(presentation.banner_text_h, health, mutation_failure),
            );
            scene::fill(
                canvas,
                root,
                theme::p().accent.multiply_alpha(0.08),
                &scene::rrect(
                    banner_rect.x,
                    banner_rect.y,
                    banner_rect.w,
                    banner_rect.h,
                    6.0,
                ),
            );
            draw_wrapped_text(
                canvas,
                root,
                engine,
                t(lang, Key::ReminderErrorUnavailable),
                banner_rect.x + 10.0,
                banner_rect.y + 8.0,
                banner_rect.w - 20.0,
                theme::role::hint(),
                theme::p().text,
            );
            if let Some([retry, discard]) = geometry.mutation_failure_actions {
                ActionButton::new(t(lang, Key::ReminderRetry), ActionStyle::Secondary, None).draw(
                    canvas,
                    root,
                    engine,
                    retry,
                    self.hover == Some(Hit::RetryFailed) || self.focus == Some(Hit::RetryFailed),
                );
                ActionButton::new(t(lang, Key::ReminderDiscard), ActionStyle::Ghost, None).draw(
                    canvas,
                    root,
                    engine,
                    discard,
                    self.hover == Some(Hit::DiscardFailed)
                        || self.focus == Some(Hit::DiscardFailed),
                );
            }
        }
        if !mutation_failure
            && let Some(health) = health
            && let Some(banner) = &health.banner
        {
            let banner_rect = Rect::new(
                rect.x + OVERLAY_PAD,
                rect.y + 52.0,
                rect.w - OVERLAY_PAD * 2.0,
                banner_height(presentation.banner_text_h, Some(health), mutation_failure),
            );
            scene::fill(
                canvas,
                root,
                theme::p().accent.multiply_alpha(0.08),
                &scene::rrect(
                    banner_rect.x,
                    banner_rect.y,
                    banner_rect.w,
                    banner_rect.h,
                    6.0,
                ),
            );
            draw_wrapped_text(
                canvas,
                root,
                engine,
                banner,
                banner_rect.x + 8.0,
                banner_rect.y + 7.0,
                banner_rect.w - 16.0,
                theme::role::hint(),
                theme::p().text,
            );
            if let (Some(action_rect), Some(label)) = (geometry.health_action, &health.action_label)
            {
                ActionButton::new(label, ActionStyle::Ghost, None).draw(
                    canvas,
                    root,
                    engine,
                    action_rect,
                    self.hover == Some(Hit::HealthAction) || self.focus == Some(Hit::HealthAction),
                );
            }
        }
        let footer = health
            .map(|health| health.footer.as_str())
            .unwrap_or_else(|| {
                if global.error.is_some() {
                    t(lang, Key::ReminderErrorUnavailable)
                } else if global.loading || global.updating {
                    t(lang, Key::ReminderUpdating)
                } else {
                    t(lang, Key::ReminderUpToDate)
                }
            });
        draw_text(
            canvas,
            root,
            engine,
            footer,
            rect.x + OVERLAY_PAD,
            rect.y + rect.h - 24.0,
            theme::role::hint(),
            if global.error.is_some() {
                theme::p().text
            } else {
                theme::p().row_hint
            },
        );
        if let (Some(chooser_rect), Some(_)) = (geometry.snooze, &self.snooze) {
            scene::floating_panel(
                canvas,
                root,
                chooser_rect.x,
                chooser_rect.y,
                chooser_rect.w,
                chooser_rect.h,
                &scene::Panel::popover(8.0).fill(theme::p().panel_bg),
            );
            draw_text(
                canvas,
                root,
                engine,
                t(lang, Key::ReminderSnooze),
                chooser_rect.x + 16.0,
                chooser_rect.y + 16.0,
                theme::role::section_heading(),
                theme::p().text,
            );
            for (position, (increment, [minus, label_rect, plus])) in
                geometry.snooze_increments.iter().enumerate()
            {
                ActionButton::new("−", ActionStyle::Ghost, None).draw(
                    canvas,
                    root,
                    engine,
                    *minus,
                    self.hover == Some(Hit::SnoozeAdjust(*increment, false))
                        || self.focus == Some(Hit::SnoozeAdjust(*increment, false)),
                );
                draw_text(
                    canvas,
                    root,
                    engine,
                    presentation
                        .snooze_increment_labels
                        .get(position)
                        .map_or("", String::as_str),
                    label_rect.x + 4.0,
                    label_rect.y + 4.0,
                    theme::role::meta(),
                    theme::p().text_dim,
                );
                ActionButton::new("+", ActionStyle::Ghost, None).draw(
                    canvas,
                    root,
                    engine,
                    *plus,
                    self.hover == Some(Hit::SnoozeAdjust(*increment, true))
                        || self.focus == Some(Hit::SnoozeAdjust(*increment, true)),
                );
            }
            if let Some(label) = &presentation.snooze_preview_label {
                draw_text(
                    canvas,
                    root,
                    engine,
                    label,
                    chooser_rect.x + 16.0,
                    chooser_rect.y + SNOOZE_PREVIEW_TOP,
                    theme::role::meta(),
                    theme::p().text_dim,
                );
            }
            if let Some([cancel, confirm]) = geometry.snooze_footer {
                ActionButton::new(t(lang, Key::ReminderCancel), ActionStyle::Ghost, None).draw(
                    canvas,
                    root,
                    engine,
                    cancel,
                    self.hover == Some(Hit::SnoozeCancel) || self.focus == Some(Hit::SnoozeCancel),
                );
                ActionButton::new(t(lang, Key::ReminderSnooze), ActionStyle::Primary, None).draw(
                    canvas,
                    root,
                    engine,
                    confirm,
                    self.hover == Some(Hit::SnoozeConfirm)
                        || self.focus == Some(Hit::SnoozeConfirm),
                );
            }
        }
    }

    fn row_actions_revealed(&self, index: usize) -> bool {
        matches!(
            self.hover,
            Some(Hit::Row(candidate) | Hit::RowAction(candidate, _)) if candidate == index
        ) || matches!(
            self.focus,
            Some(Hit::Row(candidate) | Hit::RowAction(candidate, _)) if candidate == index
        )
    }

    /// Hit-test the published snapshot. A row action the pointer has not
    /// revealed answers as its row, so the first click selects rather than
    /// fires.
    pub(super) fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        match self.geometry.hit(x, y) {
            Some(Hit::RowAction(index, _)) if !self.row_actions_revealed(index) => self
                .geometry
                .rows
                .iter()
                .find(|(candidate, rect)| *candidate == index && rect.contains(x, y))
                .map(|(index, _)| Hit::Row(*index)),
            hit => hit,
        }
    }

    pub(super) fn contains(&self, x: f32, y: f32) -> bool {
        self.geometry.rect.is_some_and(|rect| rect.contains(x, y))
    }

    pub(super) fn set_hover(&mut self, hit: Option<Hit>) -> bool {
        let changed = self.hover != hit;
        self.hover = hit;
        changed
    }

    pub(super) fn cursor_press(&self, x: f32, y: f32) -> bool {
        matches!(
            self.hit(x, y),
            Some(
                Hit::Back
                    | Hit::Filter(_)
                    | Hit::Close
                    | Hit::Row(_)
                    | Hit::RowAction(_, _)
                    | Hit::ClearHistory
                    | Hit::ConfirmClearHistory
                    | Hit::CancelClearHistory
                    | Hit::SnoozeAdjust(_, _)
                    | Hit::SnoozeConfirm
                    | Hit::SnoozeCancel
                    | Hit::HealthAction
                    | Hit::RetryFailed
                    | Hit::DiscardFailed
            )
        )
    }

    /// A primary press. Always handled: the host shows this surface modally.
    /// Navigation the component cannot perform itself comes back as a
    /// `Request`; everything Store-ward comes back as an `Intent`.
    pub(super) fn mouse_down(
        &mut self,
        x: f32,
        y: f32,
        now: Instant,
        global: &GlobalManagement,
        health: Option<&HealthPresentation>,
    ) -> (Option<Intent>, Option<Request>) {
        match self.hit(x, y) {
            Some(Hit::SnoozeAdjust(increment, add)) => {
                if let Some((_, chooser)) = &mut self.snooze {
                    if add {
                        chooser.add(increment);
                    } else {
                        chooser.subtract(increment);
                    }
                }
                self.clock_deadline = Some(now);
            }
            Some(Hit::SnoozeCancel) => {
                self.snooze = None;
                self.clock_deadline = None;
                self.geometry_dirty = true;
                return (Some(Intent::CancelSnooze), None);
            }
            Some(Hit::SnoozeConfirm) => {
                if let Some((row, chooser)) = &self.snooze
                    && let Some(until) = chooser.confirm(self.now_ms)
                {
                    let intent = Intent::Snooze(row.clone(), until);
                    self.snooze = None;
                    self.clock_deadline = None;
                    self.geometry_dirty = true;
                    return (Some(intent), None);
                }
            }
            Some(Hit::Filter(filter)) => {
                self.filter = filter;
                self.scroll_row = 0;
                self.clear_history_confirmation = false;
                self.geometry_dirty = true;
                return (Some(Intent::SetFilter(filter.into())), None);
            }
            Some(Hit::HealthAction) => {
                return (
                    health.and_then(|health| health.action).map(Intent::Health),
                    None,
                );
            }
            Some(Hit::RetryFailed) => return (Some(Intent::RetryFailed), None),
            Some(Hit::DiscardFailed) => return (Some(Intent::DiscardFailed), None),
            Some(Hit::Back) => return (None, Some(Request::Back)),
            Some(Hit::Close) => return (None, Some(Request::Close)),
            Some(Hit::ClearHistory) => {
                self.clear_history_confirmation = true;
                self.geometry_dirty = true;
            }
            Some(Hit::CancelClearHistory) => {
                self.clear_history_confirmation = false;
                self.geometry_dirty = true;
            }
            Some(Hit::ConfirmClearHistory) => {
                self.clear_history_confirmation = false;
                self.geometry_dirty = true;
                return (Some(Intent::ClearAllHistory), None);
            }
            Some(Hit::Row(index)) => {
                if let Some(row) = global.rows.get(index).cloned() {
                    return (Some(Intent::Open(row)), None);
                }
            }
            Some(Hit::RowAction(index, action)) => {
                if let Some(row) = global.rows.get(index).cloned() {
                    return (self.row_action(row, action, now), None);
                }
            }
            // A press outside the surface dismisses it when there is a
            // surface to dismiss; a page host's content rect is the page, so
            // an outside press is simply not this component's.
            _ if self.chrome.back_close() && !self.contains(x, y) => {
                return (None, Some(Request::Close));
            }
            _ => {}
        }
        (None, None)
    }

    /// A key while the surface owns input. Always handled: the host routes
    /// modally. `Request`s and `Intent`s mean the same as in `mouse_down`.
    pub(super) fn key(
        &mut self,
        key: &KeyboardKey,
        now: Instant,
        global: &GlobalManagement,
        presentation: &Presentation,
        health: Option<&HealthPresentation>,
    ) -> (Option<Intent>, Option<Request>) {
        if self.snooze.is_some() && matches!(key, KeyboardKey::Named(NamedKey::Escape)) {
            self.snooze = None;
            self.clock_deadline = None;
            self.focus = None;
            self.geometry_dirty = true;
            return (Some(Intent::CancelSnooze), None);
        }
        match key {
            KeyboardKey::Named(NamedKey::Escape) => {
                if self.chrome.back_close() {
                    return (None, Some(Request::Close));
                }
                // A page host has nothing to dismiss: Esc walks one level back
                // *inside* the surface — an armed confirmation, then the
                // focus — and never navigates. The tab strip's × is the close.
                if self.clear_history_confirmation {
                    self.clear_history_confirmation = false;
                    self.geometry_dirty = true;
                } else {
                    self.focus = None;
                }
                (None, None)
            }
            KeyboardKey::Named(NamedKey::PageUp) if self.snooze.is_none() => {
                let capacity = self.list_capacity() as isize;
                self.scroll_rows(-capacity, presentation);
                (None, None)
            }
            KeyboardKey::Named(NamedKey::PageDown) if self.snooze.is_none() => {
                let capacity = self.list_capacity() as isize;
                self.scroll_rows(capacity, presentation);
                (None, None)
            }
            KeyboardKey::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                self.move_focus_cached(-1);
                (None, None)
            }
            KeyboardKey::Named(NamedKey::ArrowRight | NamedKey::ArrowDown | NamedKey::Tab) => {
                self.move_focus_cached(1);
                (None, None)
            }
            KeyboardKey::Named(NamedKey::Enter | NamedKey::Space) => {
                if let Some(focus) = self.focus {
                    return self.activate(focus, now, global, health);
                }
                (None, None)
            }
            _ => (None, None),
        }
    }

    fn activate(
        &mut self,
        hit: Hit,
        now: Instant,
        global: &GlobalManagement,
        health: Option<&HealthPresentation>,
    ) -> (Option<Intent>, Option<Request>) {
        match hit {
            Hit::Row(index) => (global.rows.get(index).cloned().map(Intent::Open), None),
            Hit::RowAction(index, action) => (
                global
                    .rows
                    .get(index)
                    .cloned()
                    .and_then(|row| self.row_action(row, action, now)),
                None,
            ),
            Hit::Back => (None, Some(Request::Back)),
            Hit::Filter(filter) => {
                self.filter = filter;
                self.scroll_row = 0;
                self.geometry_dirty = true;
                (Some(Intent::SetFilter(filter.into())), None)
            }
            Hit::Close => (None, Some(Request::Close)),
            Hit::ClearHistory => {
                self.clear_history_confirmation = true;
                self.focus = Some(Hit::CancelClearHistory);
                self.geometry_dirty = true;
                (None, None)
            }
            Hit::CancelClearHistory => {
                self.clear_history_confirmation = false;
                self.focus = Some(Hit::ClearHistory);
                self.geometry_dirty = true;
                (None, None)
            }
            Hit::ConfirmClearHistory => {
                self.clear_history_confirmation = false;
                self.geometry_dirty = true;
                (Some(Intent::ClearAllHistory), None)
            }
            Hit::SnoozeAdjust(increment, add) => {
                if let Some((_, chooser)) = &mut self.snooze {
                    if add {
                        chooser.add(increment);
                    } else {
                        chooser.subtract(increment);
                    }
                }
                self.clock_deadline = Some(now);
                (None, None)
            }
            Hit::SnoozeCancel => {
                self.snooze = None;
                self.clock_deadline = None;
                self.focus = Some(Hit::Close);
                self.geometry_dirty = true;
                (Some(Intent::CancelSnooze), None)
            }
            Hit::SnoozeConfirm => {
                let intent = self.snooze.as_ref().and_then(|(row, chooser)| {
                    chooser
                        .confirm(self.now_ms)
                        .map(|until| Intent::Snooze(row.clone(), until))
                });
                if intent.is_some() {
                    self.snooze = None;
                    self.clock_deadline = None;
                    self.geometry_dirty = true;
                }
                (intent, None)
            }
            Hit::HealthAction => (
                health.and_then(|health| health.action).map(Intent::Health),
                None,
            ),
            Hit::RetryFailed => (Some(Intent::RetryFailed), None),
            Hit::DiscardFailed => (Some(Intent::DiscardFailed), None),
            Hit::Overlay => (None, None),
        }
    }

    pub(super) fn row_action(
        &mut self,
        row: GlobalItem,
        action: RowAction,
        now: Instant,
    ) -> Option<Intent> {
        match action {
            RowAction::Open => Some(Intent::Open(row)),
            RowAction::Snooze if row.occurrence.is_some() => {
                self.open_snooze(row, now);
                None
            }
            RowAction::Handled if row.occurrence.is_some() => Some(Intent::Handled(row)),
            RowAction::Edit => Some(Intent::Edit(row)),
            RowAction::Recreate => Some(Intent::Recreate(row)),
            RowAction::PauseResume if row.rule_paused => Some(Intent::Resume(row)),
            RowAction::PauseResume => Some(Intent::Pause(row)),
            RowAction::Delete => Some(Intent::Delete(row)),
            RowAction::Snooze | RowAction::Handled => None,
        }
    }

    fn move_focus_cached(&mut self, delta: isize) {
        let order = self.geometry.focus_order();
        if order.is_empty() {
            self.focus = None;
            return;
        }
        let at = self
            .focus
            .and_then(|focus| order.iter().position(|candidate| *candidate == focus))
            .map_or(if delta < 0 { order.len() } else { usize::MAX }, |at| at);
        let next = if at == usize::MAX {
            0
        } else {
            (at as isize + delta).rem_euclid(order.len() as isize) as usize
        };
        self.focus = Some(order[next]);
    }

    fn visible_indices<'a>(&self, presentation: &'a Presentation) -> &'a [usize] {
        &presentation.ordered[filter_slot(self.filter.into())]
    }

    /// A scroll wheel step. The host has already decided the surface is live;
    /// the chooser swallows scrolling because it owns the pointer's modal.
    pub(super) fn scroll_by(&mut self, delta: f32, presentation: &Presentation) -> bool {
        if self.snooze.is_some() || delta.abs() <= f32::EPSILON {
            return false;
        }
        let rows = (delta.abs() / 32.0).ceil().max(1.0) as isize;
        self.scroll_rows(if delta > 0.0 { rows } else { -rows }, presentation)
    }

    fn scroll_rows(&mut self, rows: isize, presentation: &Presentation) -> bool {
        let visible = self.visible_indices(presentation).len();
        let capacity = self.list_capacity();
        let maximum = visible.saturating_sub(capacity);
        let next = (self.scroll_row as isize + rows).clamp(0, maximum as isize) as usize;
        let changed = next != self.scroll_row;
        self.scroll_row = next;
        self.focus = None;
        self.geometry_dirty |= changed;
        changed
    }

    fn list_capacity(&self) -> usize {
        let Some(rect) = self.geometry.rect else {
            return 0;
        };
        let list_top = self
            .geometry
            .filters
            .iter()
            .map(|(_, chip)| chip.y + chip.h)
            .fold(rect.y + 52.0, f32::max)
            + 12.0;
        let list_bottom = rect.y + rect.h - OVERLAY_PAD - 34.0;
        ((list_bottom - list_top) / OVERLAY_ROW_H).floor().max(0.0) as usize
    }
}

fn row_source_location(row: &GlobalItem) -> &str {
    &row.source_location
}

fn state_key(state: GlobalItemState) -> Key {
    match state {
        GlobalItemState::NeedsAttention => Key::ReminderNeedsAttention,
        GlobalItemState::ComingUp => Key::ReminderComingUp,
        GlobalItemState::Snoozed => Key::ReminderFilterSnoozed,
        GlobalItemState::Paused => Key::ReminderFilterPaused,
        GlobalItemState::History => Key::ReminderHistory,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use chrono::NaiveDate;
    use koru_editor::reminders::GlobalFilter;
    use koru_editor::utils::zone::IanaZoneRules;
    use koru_i18n::time::LocaleFormat;

    use crate::mainview::reminder_shell::tests::{VIEWPORT, bell, item, opened_popover};
    use crate::mainview::reminder_shell::{PresentationEnvironment, Shell};
    use crate::reminder_settings::HealthAction;

    #[test]
    fn all_reminders_rows_expose_the_projected_source_breadcrumb() {
        let row = item("row", GlobalItemState::ComingUp, 1_700_000_000_000);
        assert_eq!(row_source_location(&row), "Acme / Notes");
    }

    #[test]
    fn page_host_supplies_a_content_rect_without_popover_chrome() {
        let rect = Rect::new(40.0, 60.0, 800.0, 600.0);
        let mut management = Management::default();
        management.rebuild_geometry(
            Some(rect),
            Chrome::Page,
            &GlobalManagement::default(),
            &Presentation::default(),
            None,
            false,
            false,
            false,
        );
        assert_eq!(management.content_rect(), Some(rect));
        assert!(management.geometry.back.is_none());
        assert!(management.geometry.close.is_none());
        assert!(!management.geometry.focus_order().contains(&Hit::Back));
        assert!(!management.geometry.focus_order().contains(&Hit::Close));
        assert_eq!(management.geometry.filters.len(), Filter::ALL.len());
        let (intent, request) = management.mouse_down(
            rect.x + rect.w - 4.0,
            rect.y + rect.h - 4.0,
            Instant::now(),
            &GlobalManagement::default(),
            None,
        );
        assert!(intent.is_none() && request.is_none());
    }

    #[test]
    fn all_reminders_back_returns_to_attention_in_the_same_popover() {
        let now = Instant::now();
        let mut shell = opened_popover(now);
        shell.apply(Intent::OpenAllReminders, now);
        shell.advance(now + Duration::from_millis(200));
        shell.maintain_geometry(VIEWPORT, bell());
        let back = shell.management.geometry.back.unwrap();
        let (handled, intent) = shell.mouse_down(
            VIEWPORT,
            bell(),
            back.x + 1.0,
            back.y + 1.0,
            now + Duration::from_millis(201),
        );
        assert!(handled);
        assert!(intent.is_none());
        assert!(shell.popover_live());
        shell.maintain_geometry(VIEWPORT, bell());
        assert!(shell.management.geometry.back.is_none());
    }

    #[test]
    fn card_pause_resume_uses_authored_rule_state_and_shared_hit_geometry() {
        let now = Instant::now();
        for rule_paused in [false, true] {
            let mut row = item(Default::default(), GlobalItemState::Paused, 1);
            row.rule_paused = rule_paused;
            let mut shell = Shell::default();
            shell.set_global(&GlobalManagement::new(vec![row]), Lang::En);
            shell.apply(Intent::OpenAllReminders, now);
            shell.advance(now + Duration::from_millis(200));
            shell.maintain_geometry(VIEWPORT, bell());
            let row_rect = shell.management.geometry.rows[0].1;
            shell.pointer_move(VIEWPORT, bell(), row_rect.x + 1.0, row_rect.y + 1.0);
            shell.maintain_geometry(VIEWPORT, bell());
            let action = shell.management.geometry.row_actions[0]
                .1
                .iter()
                .find_map(|(kind, rect)| (*kind == RowAction::PauseResume).then_some(*rect))
                .unwrap();
            let (_, intent) = shell.mouse_down(
                VIEWPORT,
                bell(),
                action.x + 1.0,
                action.y + 1.0,
                now + Duration::from_millis(201),
            );
            assert!(matches!(
                (rule_paused, intent),
                (false, Some(Intent::Pause(_))) | (true, Some(Intent::Resume(_)))
            ));
        }
    }

    #[test]
    fn popover_snooze_opens_visible_exclusive_chooser_and_emits_exact_row() {
        let now = Instant::now();
        let row = item("fired", GlobalItemState::ComingUp, 1_700_000_000_000);
        let mut global = GlobalManagement::new(vec![row.clone()]);
        global.set_needs_attention([koru_editor::reminders::GlobalAttention {
            reminder_id: row.reminder_id.clone(),
            occurrence: row.occurrence.clone().unwrap(),
            fired_at_ms: 1_700_000_000_000,
        }]);
        let mut shell = Shell::default();
        shell.set_now_ms(1_700_000_100_000);
        shell.set_global(&global, Lang::En);
        assert!(shell.attention.fired[0].context.contains("Notes"));
        assert!(!shell.attention.fired[0].context.contains("fired"));
        assert!(!shell.attention.fired[0].fired_at.is_empty());
        shell.maintain_geometry(VIEWPORT, bell());
        shell.mouse_down(VIEWPORT, bell(), 810.0, 10.0, now);
        shell.advance(now + Duration::from_millis(100));
        let snooze = shell.geometry(VIEWPORT, bell()).fired_actions[0][1];
        assert!(
            shell
                .mouse_down(
                    VIEWPORT,
                    bell(),
                    snooze.x + 1.0,
                    snooze.y + 1.0,
                    now + Duration::from_millis(101),
                )
                .0
        );
        shell.advance(now + Duration::from_millis(300));
        shell.maintain_geometry(VIEWPORT, bell());
        let (minus, plus) = {
            let geometry = &shell.management.geometry;
            assert!(geometry.snooze.is_some());
            assert_eq!(geometry.snooze_increments.len(), 6);
            let chooser = geometry.snooze.unwrap();
            for (_, [minus, label, plus]) in &geometry.snooze_increments {
                assert!(label.w >= 80.0, "duration and count need a readable lane");
                assert!(minus.x + minus.w <= label.x);
                assert!(label.x + label.w <= plus.x);
            }
            assert_eq!(
                geometry.snooze_increments[0].1[1].y, geometry.snooze_increments[1].1[1].y,
                "two increments share each row"
            );
            assert!(
                geometry.snooze_increments[2].1[1].y > geometry.snooze_increments[0].1[1].y,
                "the third increment starts a new row"
            );
            let last_label = geometry.snooze_increments.last().unwrap().1[1];
            assert!(last_label.y + last_label.h <= chooser.y + SNOOZE_PREVIEW_TOP);
            let filter = geometry.filters[0].1;
            assert_eq!(
                geometry.hit(filter.x + 1.0, filter.y + 1.0),
                Some(Hit::Overlay)
            );
            let (_, [minus, _, plus]) = geometry.snooze_increments[0];
            (minus, plus)
        };
        shell.mouse_down(VIEWPORT, bell(), plus.x + 1.0, plus.y + 1.0, now);
        assert_eq!(
            shell
                .management
                .snooze
                .as_ref()
                .unwrap()
                .1
                .count(SnoozeIncrement::OneMinute),
            1
        );
        shell.mouse_down(VIEWPORT, bell(), minus.x + 1.0, minus.y + 1.0, now);
        assert_eq!(
            shell
                .management
                .snooze
                .as_ref()
                .unwrap()
                .1
                .count(SnoozeIncrement::OneMinute),
            0
        );
        shell.mouse_down(VIEWPORT, bell(), plus.x + 1.0, plus.y + 1.0, now);
        shell.maintain_geometry(VIEWPORT, bell());
        let confirm = shell.management.geometry.snooze_footer.unwrap()[1];
        let (_, intent) = shell.mouse_down(VIEWPORT, bell(), confirm.x + 1.0, confirm.y + 1.0, now);
        assert!(matches!(
            intent,
            Some(Intent::Snooze(actual, 1_700_000_160_000))
                if actual.reminder_id == row.reminder_id && actual.occurrence == row.occurrence
        ));
    }

    #[test]
    fn shared_focus_geometry_reaches_rows_actions_filters_close_and_modal_controls() {
        let now = Instant::now();
        let global = GlobalManagement::new(vec![item(
            "row",
            GlobalItemState::NeedsAttention,
            1_700_000_000_000,
        )]);
        let mut shell = Shell::default();
        shell.set_global(&global, Lang::En);
        shell.apply(Intent::OpenAllReminders, now);
        shell.advance(now + Duration::from_millis(200));
        shell.maintain_geometry(VIEWPORT, bell());
        assert!(
            !shell.management.geometry.row_actions[0].1.is_empty(),
            "maintenance publishes complete action geometry"
        );
        let order = shell.management.geometry.focus_order().to_vec();
        assert!(order.contains(&Hit::Close));
        assert_eq!(
            order
                .iter()
                .filter(|hit| matches!(hit, Hit::Filter(_)))
                .count(),
            6
        );
        assert!(order.iter().any(|hit| matches!(hit, Hit::Row(_))));
        assert!(
            !order
                .iter()
                .any(|hit| matches!(hit, Hit::RowAction(_, RowAction::Edit)))
        );
        shell.management.focus = Some(Hit::Row(0));
        shell.maintain_geometry(VIEWPORT, bell());
        {
            let revealed = &shell.management.geometry;
            assert!(!revealed.row_actions[0].1.is_empty());
            assert!(
                revealed.row_actions[0]
                    .1
                    .iter()
                    .all(|(_, action)| { action.y > revealed.rows[0].1.y + 42.0 })
            );
        }
        shell.set_edit_available(true);
        shell.maintain_geometry(VIEWPORT, bell());
        assert!(
            shell
                .management
                .geometry
                .focus_order()
                .iter()
                .any(|hit| matches!(hit, Hit::RowAction(_, RowAction::Edit)))
        );
        let row = global.rows[0].clone();
        shell.management.row_action(row, RowAction::Snooze, now);
        shell.maintain_geometry(VIEWPORT, bell());
        let modal = shell.management.geometry.focus_order().to_vec();
        assert_eq!(
            modal
                .iter()
                .filter(|hit| matches!(hit, Hit::SnoozeAdjust(_, _)))
                .count(),
            12
        );
        assert!(modal.contains(&Hit::SnoozeCancel) && modal.contains(&Hit::SnoozeConfirm));

        shell.management.snooze = None;
        let history_global = GlobalManagement::new(vec![item(
            "history",
            GlobalItemState::History,
            1_700_000_000_000,
        )]);
        shell.set_global(&history_global, Lang::En);
        shell.apply(Intent::SetFilter(GlobalFilter::History), now);
        shell.maintain_geometry(VIEWPORT, bell());
        let history = shell.management.geometry.focus_order().to_vec();
        assert!(history.contains(&Hit::ClearHistory));
        shell.management.focus = Some(Hit::Row(0));
        shell.maintain_geometry(VIEWPORT, bell());
        let history = shell.management.geometry.focus_order().to_vec();
        assert_eq!(
            history
                .iter()
                .filter(|hit| matches!(hit, Hit::RowAction(0, _)))
                .count(),
            1,
            "History exposes Open only until creation owns past-input handling"
        );
        shell.set_recreate_available(true);
        shell.maintain_geometry(VIEWPORT, bell());
        let history = shell.management.geometry.focus_order().to_vec();
        assert_eq!(
            history
                .iter()
                .filter(|hit| matches!(hit, Hit::RowAction(0, _)))
                .count(),
            2
        );
        assert!(history.contains(&Hit::RowAction(0, RowAction::Recreate)));
        assert!(matches!(
            shell
                .management
                .activate(Hit::RowAction(0, RowAction::Recreate), now, &history_global, None)
                .0,
            Some(Intent::Recreate(row)) if row.state == GlobalItemState::History
        ));
        shell
            .management
            .activate(Hit::ClearHistory, now, &history_global, None);
        shell.maintain_geometry(VIEWPORT, bell());
        let confirm = shell.management.geometry.focus_order().to_vec();
        assert!(confirm.contains(&Hit::CancelClearHistory));
        assert!(confirm.contains(&Hit::ConfirmClearHistory));
    }

    #[test]
    fn long_overlay_scrolls_clamps_and_resets_for_filter_and_profile() {
        let now = Instant::now();
        let rows = (0..20)
            .map(|index| item(&format!("row-{index:02}"), GlobalItemState::ComingUp, index))
            .collect();
        let mut shell = Shell::default();
        shell.set_global(&GlobalManagement::new(rows), Lang::En);
        shell.apply(Intent::OpenAllReminders, now);
        shell.advance(now + Duration::from_millis(200));
        shell.maintain_geometry(VIEWPORT, bell());
        let first = shell.management.geometry.rows[0].0;
        assert!(shell.scroll_by(VIEWPORT, 100.0));
        shell.maintain_geometry(VIEWPORT, bell());
        assert_ne!(shell.management.geometry.rows[0].0, first);
        shell.scroll_by(VIEWPORT, 100_000.0);
        let maximum = 20usize.saturating_sub(shell.management.list_capacity());
        assert_eq!(shell.management.scroll_row, maximum);
        shell.apply(Intent::SetFilter(GlobalFilter::Snoozed), now);
        assert_eq!(shell.management.scroll_row, 0);
        shell.reset_profile();
        assert_eq!(shell.management.scroll_row, 0);
    }

    #[test]
    fn health_banner_action_appears_and_recovery_clears_it() {
        let now = Instant::now();
        let mut shell = Shell::default();
        shell.apply(Intent::OpenAllReminders, now);
        shell.advance(now + Duration::from_millis(200));
        shell.set_health(HealthPresentation {
            footer: "Unavailable".into(),
            banner: Some("Notifications are blocked".into()),
            action_label: Some("Open settings".into()),
            action: Some(HealthAction::OpenNotificationSettings),
        });
        shell.maintain_geometry(VIEWPORT, bell());
        let action = shell.management.geometry.health_action.unwrap();
        let (_, intent) = shell.mouse_down(VIEWPORT, bell(), action.x + 1.0, action.y + 1.0, now);
        assert_eq!(
            intent,
            Some(Intent::Health(HealthAction::OpenNotificationSettings))
        );
        shell.set_health(HealthPresentation {
            footer: "Up to date".into(),
            banner: None,
            action_label: None,
            action: None,
        });
        shell.maintain_geometry(VIEWPORT, bell());
        assert!(shell.management.geometry.health_action.is_none());
    }

    #[test]
    fn every_localized_health_warning_wraps_above_its_action_and_filters() {
        for lang in Lang::ALL {
            let mut shell = Shell::default();
            shell.apply(Intent::OpenAllReminders, Instant::now());
            let health = HealthPresentation {
                footer: String::new(),
                banner: Some(t(lang, Key::ReminderWarningDeliveryUnavailable).to_owned()),
                action_label: Some(t(lang, Key::ReminderRepairDelivery).to_owned()),
                action: Some(HealthAction::Repair),
            };
            shell.set_health(health.clone());
            let mut engine = TextEngine::new();
            shell.prepare_action_widths(&mut engine, lang);
            let banner_h = banner_height(shell.presentation.banner_text_h, Some(&health), false);
            shell.maintain_geometry(VIEWPORT, bell());
            let geometry = &shell.management.geometry;
            let panel = geometry.rect.unwrap();
            let action = geometry.health_action.unwrap();
            assert!(action.y + action.h <= panel.y + 52.0 + banner_h);
            assert!(geometry.filters[0].1.y >= panel.y + 52.0 + banner_h);
            let text_w = panel.w - OVERLAY_PAD * 2.0 - 16.0;
            let layout = engine.simple_layout_wrapped(
                t(lang, Key::ReminderWarningDeliveryUnavailable),
                theme::role::hint(),
                text_w,
            );
            assert!(layout.width() <= text_w + 0.01);
        }
    }

    #[test]
    fn every_localized_filter_chip_flows_inside_the_popover() {
        for lang in Lang::ALL {
            let mut shell = Shell::default();
            shell.apply(Intent::OpenAllReminders, Instant::now());
            let mut engine = TextEngine::new();
            shell.prepare_action_widths(&mut engine, lang);
            shell.maintain_geometry(VIEWPORT, bell());
            let geometry = &shell.management.geometry;
            let panel = geometry.rect.unwrap();
            assert_eq!(geometry.filters.len(), Filter::ALL.len());
            for (_, chip) in &geometry.filters {
                assert!(chip.x >= panel.x + OVERLAY_PAD, "{}: {chip:?}", lang.tag());
                assert!(
                    chip.x + chip.w <= panel.x + panel.w - OVERLAY_PAD + 0.01,
                    "{}: {chip:?}",
                    lang.tag()
                );
            }
            for left in 0..geometry.filters.len() {
                for right in left + 1..geometry.filters.len() {
                    let a = geometry.filters[left].1;
                    let b = geometry.filters[right].1;
                    assert!(
                        a.x + a.w <= b.x
                            || b.x + b.w <= a.x
                            || a.y + a.h <= b.y
                            || b.y + b.h <= a.y,
                        "{}: {a:?} overlaps {b:?}",
                        lang.tag()
                    );
                }
            }
        }
    }

    #[test]
    fn failed_mutation_is_visible_focusable_and_recoverable() {
        let now = Instant::now();
        let mut shell = Shell::default();
        shell.apply(Intent::OpenAllReminders, now);
        shell.advance(now + Duration::from_millis(200));
        shell.set_mutation_failure(true);
        shell.maintain_geometry(VIEWPORT, bell());
        let [retry, discard] = shell.management.geometry.mutation_failure_actions.unwrap();
        assert!(
            shell
                .management
                .geometry
                .focus_order()
                .contains(&Hit::RetryFailed)
        );
        assert!(
            shell
                .management
                .geometry
                .focus_order()
                .contains(&Hit::DiscardFailed)
        );
        assert_eq!(
            shell
                .mouse_down(VIEWPORT, bell(), retry.x + 1.0, retry.y + 1.0, now)
                .1,
            Some(Intent::RetryFailed)
        );
        assert_eq!(
            shell
                .mouse_down(VIEWPORT, bell(), discard.x + 1.0, discard.y + 1.0, now)
                .1,
            Some(Intent::DiscardFailed)
        );
    }

    #[test]
    fn snooze_cache_refreshes_on_its_own_minute_deadline_and_confirm_uses_it() {
        let now = Instant::now();
        let now_ms = 1_700_000_100_000;
        let mut shell = Shell::default();
        let row = item("due", GlobalItemState::NeedsAttention, now_ms);
        shell.set_now_ms(now_ms);
        shell.management.row_action(row, RowAction::Snooze, now);
        assert_eq!(shell.maintenance_deadline(), Some(now));
        assert!(shell.maintain_clock(now, now_ms));
        let deadline = shell.maintenance_deadline().unwrap();
        assert!(deadline > now && deadline <= now + Duration::from_secs(60));

        for _ in 0..2 {
            shell.management.activate(
                Hit::SnoozeAdjust(SnoozeIncrement::OneHour, true),
                now,
                &GlobalManagement::default(),
                None,
            );
        }
        for _ in 0..3 {
            shell.management.activate(
                Hit::SnoozeAdjust(SnoozeIncrement::FifteenMinutes, true),
                now,
                &GlobalManagement::default(),
                None,
            );
        }
        assert!(shell.maintain_clock(now, now_ms));
        let preview = shell.presentation.snooze_preview_label.as_deref().unwrap();
        let expected_duration = format!(
            "{} {}",
            t(Lang::En, Key::ReminderSnoozeHours).replacen("{}", "2", 1),
            t(Lang::En, Key::ReminderSnoozeMinutes).replacen("{}", "45", 1)
        );
        assert!(preview.starts_with(&expected_duration), "{preview}");
        assert!(matches!(
            shell
                .management
                .activate(Hit::SnoozeConfirm, now, &GlobalManagement::default(), None)
                .0,
            Some(Intent::Snooze(_, until)) if until == now_ms + 165 * 60_000
        ));
        assert!(shell.maintenance_deadline().is_none());
    }

    #[test]
    fn snooze_preview_uses_cached_hour_cycle_and_travel_zone() {
        let now = Instant::now();
        let now_ms = NaiveDate::from_ymd_opt(2026, 9, 3)
            .unwrap()
            .and_hms_opt(20, 30, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let row = item("due", GlobalItemState::NeedsAttention, now_ms);
        let global = GlobalManagement::new(vec![row.clone()]);
        let mut shell = Shell::default();
        shell.set_global(
            &global,
            PresentationEnvironment::new(
                LocaleFormat::for_system(Some("en-US"), Some(koru_i18n::time::HourCycle::H12)),
                IanaZoneRules::parse("America/New_York").unwrap(),
            ),
        );
        shell.set_now_ms(now_ms);
        shell.management.row_action(row, RowAction::Snooze, now);
        shell.management.activate(
            Hit::SnoozeAdjust(SnoozeIncrement::OneHour, true),
            now,
            &global,
            None,
        );
        shell.maintain_clock(now, now_ms);
        let h12 = shell.presentation.snooze_preview_label.as_deref().unwrap();
        assert!(h12.contains("5:30 PM"), "{h12}");

        shell.set_global(
            &global,
            PresentationEnvironment::new(
                LocaleFormat::for_system(Some("en-US"), Some(koru_i18n::time::HourCycle::H24)),
                IanaZoneRules::parse("America/New_York").unwrap(),
            ),
        );
        let h24 = shell.presentation.snooze_preview_label.as_deref().unwrap();
        assert!(h24.contains("17:30"), "{h24}");

        shell.set_global(
            &global,
            PresentationEnvironment::new(
                LocaleFormat::for_system(Some("en-US"), Some(koru_i18n::time::HourCycle::H24)),
                IanaZoneRules::parse("America/Los_Angeles").unwrap(),
            ),
        );
        let travelled = shell.presentation.snooze_preview_label.as_deref().unwrap();
        assert!(travelled.contains("14:30"), "{travelled}");
    }

    #[test]
    fn localized_row_actions_are_measured_and_never_overlap() {
        let now = Instant::now();
        for lang in Lang::ALL {
            let mut shell = Shell::default();
            shell.set_edit_available(true);
            let mut global = GlobalManagement::default();
            global
                .rows
                .push(item("one", GlobalItemState::NeedsAttention, 1));
            shell.set_global(&global, lang);
            shell.apply(Intent::OpenAllReminders, now);
            shell.advance(now + Duration::from_millis(200));
            shell.management.focus = Some(Hit::Row(0));
            shell.prepare_action_widths(&mut TextEngine::new(), lang);
            shell.maintain_geometry(VIEWPORT, bell());
            let geometry = &shell.management.geometry;
            let row = geometry.rows[0].1;
            let actions = &geometry.row_actions[0].1;
            assert_eq!(actions.len(), 5, "{}", lang.tag());
            for (_, action) in actions {
                assert!(action.x >= row.x && action.x + action.w <= row.x + row.w);
                assert!(action.y >= row.y + 60.0, "{}: {action:?}", lang.tag());
            }
            for left in 0..actions.len() {
                for right in left + 1..actions.len() {
                    let a = actions[left].1;
                    let b = actions[right].1;
                    assert!(
                        a.x + a.w <= b.x
                            || b.x + b.w <= a.x
                            || a.y + a.h <= b.y
                            || b.y + b.h <= a.y,
                        "{}: {a:?} overlaps {b:?}",
                        lang.tag()
                    );
                }
            }
        }
    }

    #[test]
    fn draw_and_input_borrow_one_maintenance_geometry_snapshot() {
        let now = Instant::now();
        let mut shell = Shell::default();
        shell.set_global(
            &GlobalManagement::new(vec![item(
                "one",
                GlobalItemState::NeedsAttention,
                1_700_000_000_000,
            )]),
            Lang::En,
        );
        shell.prepare_action_widths(&mut TextEngine::new(), Lang::En);
        shell.apply(Intent::OpenAllReminders, now);
        shell.advance(now + Duration::from_millis(200));
        shell.maintain_geometry(VIEWPORT, bell());
        let rows = shell.management.geometry.rows.as_ptr();
        let actions = shell.management.geometry.row_actions[0].1.as_ptr();
        let focus = shell.management.geometry.focus_order().as_ptr();
        let row = shell.management.geometry.rows[0].1;

        shell.pointer_move(VIEWPORT, bell(), row.x + 10.0, row.y + 10.0);
        shell.key(
            VIEWPORT,
            bell(),
            &KeyboardKey::Named(NamedKey::ArrowDown),
            now,
        );
        let mut scene = Scene::new();
        shell.draw_surfaces(
            &mut scene,
            Affine::IDENTITY,
            &mut TextEngine::new(),
            VIEWPORT,
            bell(),
            Lang::En,
        );

        assert_eq!(shell.management.geometry.rows.as_ptr(), rows);
        assert_eq!(shell.management.geometry.row_actions[0].1.as_ptr(), actions);
        assert_eq!(shell.management.geometry.focus_order().as_ptr(), focus);
    }
}
