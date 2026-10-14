// SPDX-License-Identifier: MPL-2.0

//! Cutting a window into four: the caption row, the two side columns, and the
//! active view between them.
//!
//! One function does the arithmetic and one function reads it back, which is
//! `DESIGN.md`'s rule for chrome and the reason the document toolbar has
//! `buttons()` and `hit()` rather than two copies of the same offsets.

use koru_draw::geometry::Rect;
use koru_draw::theme;

use crate::{banner, caption, tabs};

/// The four regions of a container window, in logical pixels.
///
/// A collapsed column is a **zero-width rect**, not an `Option` and not an
/// absence: every caller then does the same arithmetic whether the column is
/// open, shut, or halfway through sliding, and there is no second code path to
/// get wrong at the ends of an animation.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Regions {
    /// The full-width strip along the top. Carries the panel toggles, the tabs,
    /// and — once the window is undecorated — the minimise/maximise/close
    /// buttons at its right end.
    pub caption: Rect,
    /// The explore column, on the left.
    pub sidebar: Rect,
    /// The active tab's view.
    pub content: Rect,
    /// The context column, on the right.
    pub context: Rect,
}

/// Cut a `w` × `h` window given the two columns' current widths.
///
/// Takes **widths rather than open/shut flags** so that sliding a column open is
/// nothing but interpolating a number, and this function stays pure and
/// testable rather than growing an animation clock.
///
/// Content is protected. When the window is too narrow to give everyone their
/// width, the context column is dropped first and the sidebar second, because
/// the thing being edited is never what should be squeezed. Below that, content
/// simply runs out of room — a window that small is mid-drag or mid-restore, and
/// the only requirement is that every rect stays finite and non-negative.
pub fn solve(w: f32, h: f32, sidebar_w: f32, context_w: f32) -> Regions {
    let w = w.max(0.0);
    let h = h.max(0.0);
    let caption_h = theme::CAPTION_H.min(h);

    // Ask for what was wanted, then give way in order.
    let mut sw = sidebar_w.max(0.0).min(w);
    let mut cw = context_w.max(0.0).min(w);
    if w - sw - cw < theme::CONTENT_MIN_W {
        cw = (w - sw - theme::CONTENT_MIN_W).max(0.0).min(cw);
    }
    if w - sw - cw < theme::CONTENT_MIN_W {
        sw = (w - cw - theme::CONTENT_MIN_W).max(0.0).min(sw);
    }
    // Still too narrow: the columns are already gone, so content takes what is
    // left rather than going negative.
    let content_w = (w - sw - cw).max(0.0);
    let below = (h - caption_h).max(0.0);

    Regions {
        // **The caption starts after the pane.** The explore column owns the
        // full height of its own strip, so the logo, the tabs and the window
        // buttons all shift right with it rather than sitting above it — which
        // is what makes the pane read as a place rather than a drawer.
        //
        // Everything in `caption` derives from `caption.x`, so `tab_strip`,
        // `context_toggle` and `koru_ui::caption::controls` follow from this
        // one line.
        caption: Rect::new(sw, 0.0, (w - sw).max(0.0), caption_h),
        sidebar: Rect::new(0.0, 0.0, sw, h),
        content: Rect::new(sw, caption_h, content_w, below),
        context: Rect::new(w - cw, caption_h, cw, below),
    }
}

/// What is under a window-space point.
///
/// Ordered by precedence rather than by position: the caption's furniture is
/// small and sits inside the strip, so each piece claims itself before the
/// strip claims what is left.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Hit {
    /// The explore column. Which row is the column's own question.
    Sidebar,
    /// The context column.
    Context,
    /// The active view.
    Content,

    Tab(usize),
    /// The close box inside a tab. Outranks [`Hit::Tab`], which is why it is a
    /// variant of its own rather than a flag on one.
    TabClose(usize),
    NewTab,
    /// Horizontal scrollbar under a crowded tab strip.
    TabScroll,

    ToggleSidebar,
    ToggleContext,
    Search,
    /// An optional product-owned caption action. Chrome owns only its slot.
    HostAction,

    /// The caption, but none of its furniture — the part you drag the window by.
    Caption,
    /// A host-neutral control from `koru-ui`; the container maps it to its
    /// own window request or menu behavior.
    Window(koru_ui::caption::Action),
}

/// Resolve a window-space point.
///
/// `window_buttons` is false while the OS still draws its own titlebar, so the
/// right end of the caption is empty space rather than three buttons.
/// `context` is false while the context column is not offered, and its toggle
/// is then a zero-width rect the tab strip has already taken. Both are
/// parameters rather than build flags, because which furniture a window has is
/// the container's decision — and because the tests drive every combination.
pub fn hit(
    r: &Regions,
    tab_widths: &[f32],
    tab_scroll: f32,
    window_buttons: bool,
    context: bool,
    // When true, reserve and hit-test the reusable search button between the
    // tabs and the optional context toggle.
    search: bool,
    // When true, reserve and hit-test one host-owned action immediately before
    // the window controls.
    host_action: bool,
    // When false, the overflow thumb is hidden and must not steal hits from
    // the tabs sitting above its band.
    tab_scrollbar: bool,
    x: f32,
    y: f32,
) -> Hit {
    hit_with_banner(
        r,
        tab_widths,
        tab_scroll,
        window_buttons,
        context,
        search,
        host_action,
        tab_scrollbar,
        banner::Slots::NONE,
        x,
        y,
    )
}

/// The same, for a window whose caption carries a banner.
///
/// The banner's own slots are **not** resolved here: they are asked first, by
/// the container, through [`banner::hit`], which is the precedence
/// `DESIGN.md` states (window controls → sidebar toggle → profile → workspace →
/// host action → …). Nothing overlaps, so asking the banner before this is the
/// same answer as a fifth arm inside it — and it keeps `Hit` free of variants
/// that mean nothing to a window with no banner. What this needs the slots for
/// is the *lane*: the tabs begin at [`banner::end`], and an index resolved
/// against the wrong start would select a different tab from the one under the
/// pointer.
#[allow(clippy::too_many_arguments)]
pub fn hit_with_banner(
    r: &Regions,
    tab_widths: &[f32],
    tab_scroll: f32,
    window_buttons: bool,
    context: bool,
    search: bool,
    host_action: bool,
    tab_scrollbar: bool,
    slots: banner::Slots,
    x: f32,
    y: f32,
) -> Hit {
    // Caption first, and that ordering is now doing real work in both
    // directions. With the pane **open** its left edge is `sidebar_w`, so a
    // point in the top-left fails this test and falls through to the pane —
    // which is the wanted answer, since the pane is what is drawn there. With
    // the pane **shut** the sidebar is a zero-width rect, and `contains` is
    // inclusive on every edge, so asking it first would swallow `x == 0` for
    // the whole height of the window and the logo would stop responding.
    //
    // At exactly `x == sidebar_w` both rects contain the point and the caption
    // wins, which matches what the toggle did at `x == 0` before: the edge of a
    // small control belongs to the control.
    if r.caption.contains(x, y) {
        if window_buttons {
            if let Some(action) = koru_ui::caption::hit(r.caption, x, y) {
                return Hit::Window(action);
            }
        }
        if caption::sidebar_toggle(r.caption).contains(x, y) {
            return Hit::ToggleSidebar;
        }
        let host = caption::host_action_with_banner(
            r.caption,
            window_buttons,
            context,
            host_action,
            slots,
        );
        if host.w > 0.0 && host.contains(x, y) {
            return Hit::HostAction;
        }
        // A hidden toggle is a zero-width rect sitting on the strip's right
        // edge, and `contains` is inclusive — so it has to be excluded here
        // explicitly, exactly as a collapsed column is below.
        //
        // **The banner is what the toggle's rect depends on**, because Settings
        // sits outside it (`caption::context_toggle_with_banner`). The container
        // still asks `banner::hit` before this, so the Settings square itself
        // never reaches here.
        let context_toggle =
            caption::context_toggle_with_banner(r.caption, window_buttons, context, slots);
        if context_toggle.w > 0.0 && context_toggle.contains(x, y) {
            return Hit::ToggleContext;
        }
        if search
            && caption::search_button_with_banner(
                r.caption,
                window_buttons,
                context,
                host_action,
                slots,
            )
            .contains(x, y)
        {
            return Hit::Search;
        }
        let strip = if search {
            caption::tab_strip_with_search_and_banner(
                r.caption,
                window_buttons,
                context,
                host_action,
                slots,
            )
        } else {
            caption::tab_strip_with_banner(r.caption, window_buttons, context, host_action, slots)
        };
        if tabs::new_tab(strip, tab_widths, tab_scroll).contains(x, y) {
            return Hit::NewTab;
        }
        if tab_scrollbar && tabs::scroll_track(strip, tab_widths).is_some_and(|t| t.contains(x, y))
        {
            return Hit::TabScroll;
        }
        // A tab is only hittable where it is *drawn*, and it is drawn clipped to
        // the lane — so a point in the `+`'s lane belongs to no tab even when a
        // scrolled tab's rect reaches into it.
        let lane = tabs::lane(strip);
        let view_right = lane.x + lane.w;
        if x >= view_right {
            return Hit::Caption;
        }
        for t in tabs::layout(strip, tab_widths, tab_scroll) {
            if t.body.x + t.body.w <= strip.x || t.body.x >= view_right {
                continue;
            }
            // The close box is inside the tab body, so it has to be asked first.
            if t.close.contains(x, y) {
                return Hit::TabClose(t.index);
            }
            if t.body.contains(x, y) {
                return Hit::Tab(t.index);
            }
        }
        return Hit::Caption;
    }
    // A zero-width column cannot be hit: `contains` is inclusive on every edge,
    // so a collapsed sidebar would otherwise swallow the content's left edge.
    if r.sidebar.w > 0.0 && r.sidebar.contains(x, y) {
        return Hit::Sidebar;
    }
    if r.context.w > 0.0 && r.context.contains(x, y) {
        return Hit::Context;
    }
    Hit::Content
}

#[cfg(test)]
mod tests;
