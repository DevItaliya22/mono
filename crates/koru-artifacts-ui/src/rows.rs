// SPDX-License-Identifier: MPL-2.0

//! **The one geometry function.** Drawing, hit-testing, the hover, the
//! keyboard and the scroll clamp all come from [`ArtifactsSurface::laid_out`],
//! so a row cannot be clickable somewhere other than where it is drawn
//! (`DESIGN.md`'s first rule for chrome).
//!
//! Rows outside the band are not returned at all, which is both the cull
//! and the hit test's bounds check — the tasks rail's contract, kept the
//! same so the two modes of one column cannot disagree about what a shut
//! column offers (nothing).
//!
//! ## Opening a family costs what is shown
//!
//! A family contributes at most [`VERSIONS_SHOWN`] version rows plus one
//! *N earlier versions* line, whatever its history — which is the roadmap's
//! requirement stated as arithmetic rather than as an intention, and what
//! `tests::ratio` asserts.

use koru_draw::geometry::Rect;
use koru_draw::theme;

use crate::ArtifactsSurface;
use crate::model::Load;

/// How many of a family's versions are rows when it is opened. One screen's
/// worth: enough that the newest history reads without a click, few enough
/// that opening a family with five thousand versions costs the same as
/// opening one with five.
pub const VERSIONS_SHOWN: usize = 12;

/// Air above a section heading.
const SECTION_GAP: f32 = 10.0;
/// A section heading's own height.
const SECTION_H: f32 = 22.0;
/// A sentence line — a provenance clause, a run's note, a refusal.
const NOTE_H: f32 = 17.0;
/// The glyph lane's width, the tasks rail's own.
const ICON_W: f32 = 16.0;
/// The caret lane in front of a family row.
const CARET_W: f32 = 14.0;
/// The trailing lane a count, a size or *Register* is right-aligned into.
/// Reserved on every row whether or not anything is drawn in it, so a label
/// never reflows as a number arrives.
const TRAILING_W: f32 = 78.0;
/// Air under the last row, so the column does not end flush against the
/// window edge.
const FOOTER_PAD: f32 = 8.0;
/// How far a version row is indented under its family.
const INDENT: f32 = 14.0;

/// Which state the column is in, when it is in one rather than showing
/// rows.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Notice {
    /// Asked, nothing back yet.
    Loading,
    /// The store is not answering. Never drawn as an empty column.
    Unavailable,
    /// The store answered, and this workspace has nothing.
    Empty,
}

/// One line of the column.
///
/// Indices rather than ids because a row is a *position*: the ids that
/// survive a refresh — the expanded set, the selection — are held on the
/// surface, and these are rebuilt from the catalog every layout.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Row {
    /// The state the column is in. Drawn where the rows would be.
    Notice(Notice),
    ResultsHeading,
    Family(usize),
    Version {
        family: usize,
        version: usize,
    },
    /// One clause of the selected version's provenance sentence.
    Provenance {
        family: usize,
        version: usize,
        clause: usize,
    },
    /// *N earlier versions* — the rest of a family's history, one page at a
    /// time.
    Earlier(usize),
    UnregisteredHeading,
    Run(usize),
    /// One line of what a run's heading says underneath it (D29).
    RunNote {
        run: usize,
        note: usize,
    },
    File {
        run: usize,
        file: usize,
    },
    /// Why a Register was refused, under the row it was pressed on.
    FileRefusal {
        run: usize,
        file: usize,
    },

    /// *Open* — under the selected version, beside its provenance. It asks
    /// the host for a tab; nothing opens inside the column (ADR-369).
    Open {
        family: usize,
        version: usize,
    },
}

impl Row {
    /// Whether a press on this row does anything. A heading, a note and a
    /// clause are prose; the keyboard steps over them.
    pub fn pressable(&self) -> bool {
        matches!(
            self,
            Self::Family(_)
                | Self::Version { .. }
                | Self::Earlier(_)
                | Self::File { .. }
                | Self::Open { .. }
        )
    }

    fn height(&self) -> f32 {
        match self {
            Self::ResultsHeading | Self::UnregisteredHeading => SECTION_H,
            Self::Provenance { .. } | Self::RunNote { .. } | Self::FileRefusal { .. } => NOTE_H,
            Self::Notice(_) => NOTE_H * 2.0,
            _ => theme::NAV_ROW_H,
        }
    }

    /// Air above this row, when it opens a section.
    fn gap_above(&self) -> f32 {
        match self {
            Self::UnregisteredHeading => SECTION_GAP,
            _ => 0.0,
        }
    }

    fn indent(&self) -> f32 {
        match self {
            Self::Version { .. } | Self::Earlier(_) | Self::File { .. } => INDENT,
            Self::Provenance { .. } | Self::FileRefusal { .. } | Self::Open { .. } => INDENT * 2.0,
            Self::RunNote { .. } => INDENT,
            _ => 0.0,
        }
    }
}

/// Every rect inside a row. See [`ArtifactsSurface::row_parts`].
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct RowParts {
    /// Zero-sized for a row with no children, so pressing where a caret
    /// would be does not expand nothing.
    pub caret: Rect,
    pub icon: Rect,
    /// What is left for the label, between the icon and the trailing lane.
    pub label: Rect,
    /// The count, the size, or **Register**. Reserved always.
    pub trailing: Rect,
}

impl ArtifactsSurface {
    /// Every row the column has, in order, with its rect — **before** the
    /// band is applied. The scroll extent needs the ones that are off
    /// screen, which is the only reason this is separate from
    /// [`rows`](Self::rows).
    pub(crate) fn laid_out(&mut self, area: Rect, top: f32) -> Vec<(Row, Rect)> {
        let plan = self.plan();
        self.work.rows = plan.len();
        let mut out = Vec::with_capacity(plan.len());
        let mut y = top - self.scroll;
        for row in plan {
            y += row.gap_above();
            let indent = row.indent();
            let h = row.height();
            out.push((
                row,
                Rect::new(area.x + indent, y, (area.w - indent).max(0.0), h),
            ));
            y += h;
        }
        out
    }

    /// The rows, in order, with nothing said about where they are. The
    /// whole of what the column decides to show.
    fn plan(&self) -> Vec<Row> {
        let mut plan = Vec::new();
        match self.load {
            Load::Cold | Load::Loading if self.catalog.is_empty() => {
                plan.push(Row::Notice(Notice::Loading));
                return plan;
            }
            Load::Unavailable if self.catalog.is_empty() => {
                plan.push(Row::Notice(Notice::Unavailable));
                return plan;
            }
            Load::Unavailable => plan.push(Row::Notice(Notice::Unavailable)),
            _ => {}
        }
        if self.catalog.is_empty() {
            plan.push(Row::Notice(Notice::Empty));
            return plan;
        }
        if !self.catalog.families.is_empty() {
            plan.push(Row::ResultsHeading);
        }
        for (index, entry) in self.catalog.families.iter().enumerate() {
            plan.push(Row::Family(index));
            if !self.expanded.contains(&entry.family.id) {
                continue;
            }
            let shown = self.shown(entry.family.id).min(entry.versions.len());
            // The family row is the latest artifact itself. Disclosure shows
            // history only, never a second copy of the primary result.
            for version in 1..shown {
                plan.push(Row::Version {
                    family: index,
                    version,
                });
                let Some(found) = entry.version(version) else {
                    continue;
                };
                if self.selected != Some(found.id) {
                    continue;
                }
                let clauses = crate::text::provenance_clauses(
                    self.lang,
                    found,
                    self.provenances.get(found.id),
                    self.holder(found),
                    self.now_ms,
                )
                .len();
                for clause in 0..clauses {
                    plan.push(Row::Provenance {
                        family: index,
                        version,
                        clause,
                    });
                }
                // **The one the maintainers asked for.** Selecting a version says
                // where it came from; this reads what it says.
                plan.push(Row::Open {
                    family: index,
                    version,
                });
            }
            if entry.versions.len() > shown {
                plan.push(Row::Earlier(index));
            }
        }
        if !self.catalog.runs.is_empty() {
            plan.push(Row::UnregisteredHeading);
        }
        for (index, entry) in self.catalog.runs.iter().enumerate() {
            plan.push(Row::Run(index));
            for note in 0..crate::text::run_notes(self.lang, entry).len() {
                plan.push(Row::RunNote { run: index, note });
            }
            for file in 0..entry.files.len() {
                plan.push(Row::File { run: index, file });
                if matches!(
                    self.registering
                        .get(&(entry.run, entry.files[file].path.clone())),
                    Some(crate::model::Registering::Refused(_))
                ) {
                    plan.push(Row::FileRefusal { run: index, file });
                }
            }
        }
        plan
    }

    /// The rows inside the band — the cull, and the hit test's bounds.
    pub fn rows(&mut self, area: Rect, top: f32) -> Vec<(Row, Rect)> {
        if area.w <= 0.0 {
            self.work.rows = 0;
            return Vec::new();
        }
        let bottom = area.y + area.h - FOOTER_PAD;
        self.laid_out(area, top)
            .into_iter()
            .filter(|(_, r)| r.y + r.h > top && r.y < bottom)
            .collect()
    }

    /// Every rect inside a row, derived from the row and its band alone.
    pub fn row_parts(&self, row: Row, r: Rect) -> RowParts {
        let caret_w = match row {
            Row::Family(index)
                if self
                    .catalog
                    .families
                    .get(index)
                    .is_some_and(|entry| entry.versions.len() > 1) =>
            {
                CARET_W
            }
            _ => 0.0,
        };
        let caret = Rect::new(r.x + theme::NAV_PAD, r.y, caret_w, r.h);
        let icon_x = caret.x + caret_w;
        let icon_w = match row {
            Row::Family(_) | Row::Version { .. } | Row::File { .. } | Row::Run(_) => ICON_W,
            _ => 0.0,
        };
        let icon = Rect::new(icon_x, r.y, icon_w, r.h);
        let trailing_w = match row {
            Row::Family(_) | Row::Version { .. } | Row::File { .. } => TRAILING_W,
            _ => 0.0,
        };

        let trailing = Rect::new(
            (r.x + r.w - theme::NAV_PAD - trailing_w).max(r.x),
            r.y,
            trailing_w,
            r.h,
        );
        let label_x = icon_x + icon_w + if icon_w > 0.0 { 6.0 } else { 0.0 };
        let label = Rect::new(label_x, r.y, (trailing.x - label_x - 4.0).max(0.0), r.h);
        RowParts {
            caret,
            icon,
            label,
            trailing,
        }
    }

    /// How tall the column is at this width, unscrolled.
    pub(crate) fn content_h(&mut self, area: Rect, top: f32) -> f32 {
        let scroll = self.scroll;
        self.laid_out(area, top)
            .last()
            .map(|(_, r)| r.y + r.h + scroll - top)
            .unwrap_or(0.0)
    }

    pub(crate) fn clamp_scroll(&mut self, area: Rect, top: f32) {
        let band = (area.y + area.h - FOOTER_PAD - top).max(0.0);
        let max = (self.content_h(area, top) - band).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    pub fn scroll_by(&mut self, area: Rect, top: f32, dy: f32) {
        self.scroll -= dy;
        self.clamp_scroll(area, top);
    }

    /// Bring one row into the band, moving as little as it takes — nothing
    /// at all when it is already there. A focus ring below the fold is a
    /// focus a person cannot see; the explore tree keeps the same rule.
    pub(crate) fn scroll_into_view(&mut self, row: Row, area: Rect, top: f32) {
        let Some((_, r)) = self
            .laid_out(area, top)
            .into_iter()
            .find(|(candidate, _)| *candidate == row)
        else {
            return;
        };
        let bottom = area.y + area.h - FOOTER_PAD;
        if r.y < top {
            self.scroll -= top - r.y;
        } else if r.y + r.h > bottom {
            self.scroll += r.y + r.h - bottom;
        }
        self.clamp_scroll(area, top);
    }

    /// Which row is under a point, if any.
    pub fn row_at(&mut self, area: Rect, top: f32, x: f32, y: f32) -> Option<(Row, Rect)> {
        self.rows(area, top).into_iter().find(|(_, r)| {
            r.contains(x, y) || (y >= r.y && y < r.y + r.h && x >= area.x && x < area.x + area.w)
        })
    }
}
