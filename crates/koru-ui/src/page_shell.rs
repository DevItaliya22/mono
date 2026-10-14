// SPDX-License-Identifier: MPL-2.0

//! **Where a nav-and-page surface puts its two halves** — Settings and Manage.
//!
//! A surface of this shape is a short nav of destinations on the left and one
//! page of title, lead and cards beside it. This module is only its geometry:
//! where the nav's column is, where the page's column is, and — once the
//! window is too narrow for two columns — the band across the top that holds
//! the nav as [`ChipNav`](crate::ChipNav) chips instead. It measures no text
//! and draws nothing; what goes in the nav (a search box, captions, rows) and
//! what goes on the page are the caller's.
//!
//! ```text
//!        group_x
//!        │← nav_w →│← margin →│←──── column_w ────→│← margin →│
//!        │  [nav]  │          │ Title               │          │
//!   TOP ─┤  rows   │          │ lead                │          │
//!        │         │          │ caption             │          │
//!        │         │          │ ┌ card ───────────┐ │          │
//! ```
//!
//! **The nav and the page are one group, centred together**: the page starts
//! right after the nav rather than being centred in what is left of the
//! window, which left a wide window with the two far apart. Narrower than
//! [`COMPACT_BELOW`](PageShell::COMPACT_BELOW), the nav has no column of its
//! own: the caller lays its chips into [`chips_area`](PageShell::chips_area),
//! says how tall they came out, and the page starts under the
//! [`band_h`](PageShell::band_h) that makes.
//!
//! Every rect is a pure function of the viewport, the band's height and the
//! shell's two widths, so drawing and the hit test read the same answer.

use koru_draw::geometry::Rect;

/// A nav-and-page surface's two widths. Everything else is shared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageShell {
    /// The nav's column, its left air included.
    pub nav_w: f32,
    /// The widest the page's column draws. A page stretched across a
    /// maximised window is a page of whitespace.
    pub column_w: f32,
}

impl Default for PageShell {
    fn default() -> Self {
        Self::SETTINGS
    }
}

impl PageShell {
    /// Settings' widths, which every surface of this shape starts from.
    pub const SETTINGS: Self = Self {
        nav_w: 148.0,
        column_w: 624.0,
    };

    /// The nav's left air: its rows start this far into its column.
    pub const NAV_PAD_X: f32 = 16.0;
    /// Where the nav's first line and the page's title sit under the top
    /// edge — level with each other.
    pub const TOP: f32 = 48.0;
    /// The page column's side air.
    pub const PAGE_MARGIN: f32 = 24.0;
    /// **Below this width the nav has no column of its own**: it sits above
    /// the page as chips, so a narrow window or a split pane keeps the page's
    /// full width.
    pub const COMPACT_BELOW: f32 = 520.0;
    /// The compact band's top air, and its air under the chips.
    pub const COMPACT_TOP: f32 = 16.0;
    pub const COMPACT_BOTTOM: f32 = 8.0;
    /// Between a page's title and its lead, and between the lead and the
    /// first thing under it.
    pub const TITLE_GAP: f32 = 8.0;
    pub const LEAD_GAP: f32 = 28.0;
    /// The air below the last card, so it does not sit on the window's edge.
    pub const BOTTOM_PAD: f32 = 56.0;

    /// Whether the nav sits above the page as chips rather than beside it.
    pub fn compact(&self, viewport_w: f32) -> bool {
        viewport_w < Self::COMPACT_BELOW
    }

    /// Where the centred group starts; zero once the window is too narrow to
    /// centre it.
    pub fn group_x(&self, viewport_w: f32) -> f32 {
        let group = self.nav_w + Self::PAGE_MARGIN * 2.0 + self.column_w;
        ((viewport_w - group) * 0.5).max(0.0)
    }

    /// The nav's own region: a full-height column at the group's left, or —
    /// compact — the band across the top, `band_h` tall.
    pub fn sidebar(&self, viewport: (f32, f32), band_h: f32) -> Rect {
        let (vw, vh) = viewport;
        if self.compact(vw) {
            Rect::new(0.0, 0.0, vw, band_h)
        } else {
            Rect::new(self.group_x(vw), 0.0, self.nav_w.min(vw), vh)
        }
    }

    /// The nav's content from `top` down: the sidebar less its left air.
    pub fn nav_area(&self, viewport: (f32, f32), band_h: f32, top: f32) -> Rect {
        let s = self.sidebar(viewport, band_h);
        Rect::new(
            s.x + Self::NAV_PAD_X,
            top,
            (s.w - Self::NAV_PAD_X).max(0.0),
            (s.h - top).max(0.0),
        )
    }

    /// Where compact chips go, starting at `top`: the window's width less
    /// the page margin either side. Its height is the chips', so zero here.
    pub fn chips_area(&self, viewport_w: f32, top: f32) -> Rect {
        Rect::new(
            Self::PAGE_MARGIN,
            top,
            (viewport_w - Self::PAGE_MARGIN * 2.0).max(0.0),
            0.0,
        )
    }

    /// The compact band's height for chips laid into `area` that came out
    /// `chips_h` tall — the chips, their air, and half the page's top.
    pub fn band_h(area: Rect, chips_h: f32) -> f32 {
        area.y + chips_h + Self::COMPACT_BOTTOM + Self::TOP * 0.5
    }

    /// The page area: right of the nav column, or under the compact band.
    pub fn page_area(&self, viewport: (f32, f32), band_h: f32) -> Rect {
        let (vw, vh) = viewport;
        let s = self.sidebar(viewport, band_h);
        if self.compact(vw) {
            Rect::new(0.0, s.h, vw, (vh - s.h).max(0.0))
        } else {
            let x = s.x + s.w;
            Rect::new(x, 0.0, (vw - x).max(0.0), vh)
        }
    }

    /// The page's column: right after the nav (wide) or centred under the
    /// band (compact), never wider than [`column_w`](Self::column_w). Its
    /// top is where the title goes.
    pub fn column(&self, viewport: (f32, f32), band_h: f32) -> Rect {
        let area = self.page_area(viewport, band_h);
        let compact = self.compact(viewport.0);
        let top = if compact { 0.0 } else { Self::TOP };
        let w = self
            .column_w
            .min((area.w - Self::PAGE_MARGIN * 2.0).max(0.0));
        let x = if compact {
            area.x + ((area.w - w) * 0.5).max(0.0)
        } else {
            area.x + Self::PAGE_MARGIN
        };
        Rect::new(x, area.y + top, w, (area.h - top).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELL: PageShell = PageShell::SETTINGS;

    #[test]
    fn wide_the_nav_and_the_column_are_one_centred_group() {
        let viewport = (1600.0, 900.0);
        let nav = SHELL.sidebar(viewport, 0.0);
        let column = SHELL.column(viewport, 0.0);
        assert_eq!(nav.x, SHELL.group_x(1600.0));
        assert_eq!(column.x, nav.x + nav.w + PageShell::PAGE_MARGIN);
        assert_eq!(column.w, SHELL.column_w);
        assert_eq!(column.y, PageShell::TOP);
        // The same air either side of the group.
        let right = 1600.0 - (column.x + column.w + PageShell::PAGE_MARGIN);
        assert!((right - nav.x).abs() < 0.01);
    }

    #[test]
    fn narrower_than_the_group_it_pins_to_the_left_and_the_column_shrinks() {
        let viewport = (700.0, 600.0);
        assert_eq!(SHELL.group_x(700.0), 0.0);
        let column = SHELL.column(viewport, 0.0);
        assert_eq!(column.w, 700.0 - SHELL.nav_w - PageShell::PAGE_MARGIN * 2.0);
    }

    #[test]
    fn compact_the_nav_is_a_band_and_the_page_starts_under_it() {
        let viewport = (400.0, 600.0);
        assert!(SHELL.compact(400.0));
        let chips = SHELL.chips_area(400.0, PageShell::COMPACT_TOP);
        assert_eq!(chips.x, PageShell::PAGE_MARGIN);
        assert_eq!(chips.w, 400.0 - PageShell::PAGE_MARGIN * 2.0);
        let band = PageShell::band_h(chips, 30.0);
        assert_eq!(
            SHELL.sidebar(viewport, band),
            Rect::new(0.0, 0.0, 400.0, band)
        );
        let area = SHELL.page_area(viewport, band);
        assert_eq!((area.y, area.h), (band, 600.0 - band));
        let column = SHELL.column(viewport, band);
        assert_eq!(column.y, band, "no second top margin under the band");
        assert!((column.x - (400.0 - column.w) * 0.5).abs() < 0.01);
    }

    #[test]
    fn the_nav_area_is_the_sidebar_less_its_left_air() {
        let viewport = (1200.0, 800.0);
        let s = SHELL.sidebar(viewport, 0.0);
        let nav = SHELL.nav_area(viewport, 0.0, PageShell::TOP);
        assert_eq!(nav.x, s.x + PageShell::NAV_PAD_X);
        assert_eq!(nav.w, s.w - PageShell::NAV_PAD_X);
        assert_eq!((nav.y, nav.h), (PageShell::TOP, 800.0 - PageShell::TOP));
    }
}
