// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};

use alacritty_terminal::grid::Dimensions as AlacrittyDimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52, Term, TermDamage, TermMode};
use koru_session_protocol::{
    ColumnIndex, CursorState, GridSize, MouseEncoding, MouseTrackingMode, RowIndex, RowRange,
    TerminalModes, TerminalRow,
};

use crate::MAX_SCROLLBACK_ROWS;
use crate::convert;
use crate::event::{EmulatorEvent, Listener};
use crate::parser::BoundedProcessor;
use crate::window::{
    EmulatorError, RetainedRange, RowWindow, SelectionKind, SelectionPoint, validate_range,
};

/// Changes caused by exactly one parse batch or resize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmulatorUpdate {
    pub damaged_rows: Vec<RowIndex>,
    /// New active-screen coordinates when output or resize moved the tail.
    /// A follow-tail controller recovers this bounded range rather than
    /// treating rows outside its previous viewport as an ordinary delta.
    pub visible_range: Option<RowRange>,
    pub cursor: Option<CursorState>,
    pub modes: Option<TerminalModes>,
    pub events: Vec<EmulatorEvent>,
    /// The history cap evicted retained coordinates. The owner must advance
    /// its terminal generation and recover with a bounded snapshot.
    pub retention_rebased: bool,
}

pub struct TerminalEmulator {
    term: Term<Listener>,
    parser: BoundedProcessor,
    listener: Listener,
    first_retained: u64,
    visible_cache: BTreeMap<RowIndex, TerminalRow>,
    visible_range_cache: RowRange,
    cursor_cache: CursorState,
    modes_cache: TerminalModes,
}

#[derive(Clone, Copy)]
struct EmulatorSize {
    columns: usize,
    rows: usize,
}

impl From<GridSize> for EmulatorSize {
    fn from(size: GridSize) -> Self {
        Self {
            columns: usize::from(size.columns),
            rows: usize::from(size.rows),
        }
    }
}

impl AlacrittyDimensions for EmulatorSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

impl TerminalEmulator {
    pub fn new(size: GridSize) -> Result<Self, EmulatorError> {
        validate_size(size)?;
        let listener = Listener::default();
        let dimensions = EmulatorSize::from(size);
        let config = Config {
            scrolling_history: MAX_SCROLLBACK_ROWS,
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let mut term = Term::new(config, &dimensions, listener.clone());
        term.reset_damage();
        let cursor_cache = cursor(&term, 0);
        let modes_cache = modes(&term);
        let visible_range_cache = RowRange {
            first: RowIndex(term.history_size() as u64),
            count: term.screen_lines() as u16,
        };
        let mut emulator = Self {
            term,
            parser: BoundedProcessor::default(),
            listener,
            first_retained: 0,
            visible_cache: BTreeMap::new(),
            visible_range_cache,
            cursor_cache,
            modes_cache,
        };
        emulator.replace_visible_cache();
        Ok(emulator)
    }

    /// Parse a borrowed PTY read batch synchronously; no byte queue is created.
    pub fn parse(&mut self, bytes: &[u8]) -> EmulatorUpdate {
        let history_before = self.history_size();
        self.parser.advance(&mut self.term, bytes);
        self.finish_update(history_before)
    }

    pub fn resize(&mut self, size: GridSize) -> Result<EmulatorUpdate, EmulatorError> {
        validate_size(size)?;
        let history_before = self.history_size();
        self.term.resize(EmulatorSize::from(size));
        Ok(self.finish_update(history_before))
    }

    pub fn size(&self) -> GridSize {
        GridSize {
            columns: self.term.columns() as u16,
            rows: self.term.screen_lines() as u16,
        }
    }

    pub fn retained_range(&self) -> RetainedRange {
        let count = self.term.total_lines() as u64;
        RetainedRange {
            first: RowIndex(self.first_retained),
            end_exclusive: RowIndex(self.first_retained + count),
        }
    }

    pub fn visible_range(&self) -> RowRange {
        RowRange {
            first: RowIndex(self.first_retained + self.history_size() as u64),
            count: self.term.screen_lines() as u16,
        }
    }

    pub fn cursor(&self) -> CursorState {
        cursor(&self.term, self.first_retained)
    }

    pub fn modes(&self) -> TerminalModes {
        modes(&self.term)
    }

    /// Read complete rows only from the active screen.
    pub fn visible_rows(&self, range: RowRange) -> Result<RowWindow, EmulatorError> {
        validate_range(range)?;
        let visible = self.visible_range();
        let visible_end = visible.first.0 + u64::from(visible.count);
        let requested_end = range.first.0 + u64::from(range.count);
        if range.first < visible.first || requested_end > visible_end {
            return Err(EmulatorError::RowOutsideRetention);
        }
        self.read_rows(range)
    }

    /// Read one bounded page anywhere in retained history or the active screen.
    pub fn scrollback_page(&self, range: RowRange) -> Result<RowWindow, EmulatorError> {
        self.read_rows(range)
    }

    pub fn select(
        &mut self,
        kind: SelectionKind,
        start: SelectionPoint,
        end: SelectionPoint,
    ) -> Result<Option<String>, EmulatorError> {
        let start = self.selection_point(start)?;
        let end = self.selection_point(end)?;
        let selection_type = match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Block => SelectionType::Block,
            SelectionKind::Lines => SelectionType::Lines,
            SelectionKind::Semantic => SelectionType::Semantic,
        };
        let mut selection = Selection::new(selection_type, start, Side::Left);
        selection.update(end, Side::Right);
        self.term.selection = Some(selection);
        Ok(self.term.selection_to_string())
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    fn read_rows(&self, range: RowRange) -> Result<RowWindow, EmulatorError> {
        validate_range(range)?;
        let retained = self.retained_range();
        let requested_end = range.first.0 + u64::from(range.count);
        if range.first < retained.first || requested_end > retained.end_exclusive.0 {
            return Err(EmulatorError::RowOutsideRetention);
        }

        let rows = (0..range.count)
            .map(|offset| self.row(RowIndex(range.first.0 + u64::from(offset))))
            .collect();
        Ok(RowWindow {
            requested: range,
            retained,
            visited_cells: usize::from(range.count) * self.term.columns(),
            rows,
        })
    }

    fn row(&self, index: RowIndex) -> TerminalRow {
        let line = self.line_for(index);
        let cells = (0..self.term.columns())
            .map(|column| convert::cell(column, &self.term.grid()[line][Column(column)]))
            .collect();
        let wrapped_to_next = self.term.grid()[line][Column(self.term.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE);
        TerminalRow {
            index,
            cells,
            wrapped_to_next,
        }
    }

    fn line_for(&self, index: RowIndex) -> Line {
        let relative = index.0 - self.first_retained;
        Line(relative as i32 - self.history_size() as i32)
    }

    fn selection_point(&self, point: SelectionPoint) -> Result<Point, EmulatorError> {
        let retained = self.retained_range();
        if point.row < retained.first
            || point.row >= retained.end_exclusive
            || usize::from(point.column.0) >= self.term.columns()
        {
            return Err(EmulatorError::RowOutsideRetention);
        }
        Ok(Point::new(
            self.line_for(point.row),
            Column(usize::from(point.column.0)),
        ))
    }

    fn history_size(&self) -> usize {
        self.term.history_size()
    }

    fn finish_update(&mut self, history_before: usize) -> EmulatorUpdate {
        let (full_damage, candidates) = self.damage_candidates();
        let history_after = self.history_size();
        let retention_rebased = history_before == MAX_SCROLLBACK_ROWS
            && history_after == MAX_SCROLLBACK_ROWS
            && full_damage;
        if retention_rebased {
            // The exact number of rows evicted inside a batch is intentionally
            // not guessed. Rebase atomically and make the owner change epoch.
            self.first_retained = 0;
            self.visible_cache.clear();
        }

        let candidate_lines: BTreeSet<usize> = if full_damage || retention_rebased {
            (0..self.term.screen_lines()).collect()
        } else {
            candidates
        };
        let mut damaged_rows = Vec::new();
        for line in candidate_lines {
            let index = RowIndex(self.first_retained + history_after as u64 + line as u64);
            let row = self.row(index);
            if self.visible_cache.get(&index) != Some(&row) {
                self.visible_cache.insert(index, row);
                damaged_rows.push(index);
            }
        }
        let visible = self.visible_range();
        let end = visible.first.0 + u64::from(visible.count);
        self.visible_cache
            .retain(|index, _| index.0 >= visible.first.0 && index.0 < end);

        let current_cursor = self.cursor();
        let cursor = (current_cursor != self.cursor_cache).then_some(current_cursor);
        self.cursor_cache = current_cursor;
        let current_modes = self.modes();
        let modes = (current_modes != self.modes_cache).then_some(current_modes);
        self.modes_cache = current_modes;
        let current_visible_range = self.visible_range();
        let visible_range =
            (current_visible_range != self.visible_range_cache).then_some(current_visible_range);
        self.visible_range_cache = current_visible_range;

        EmulatorUpdate {
            damaged_rows,
            visible_range,
            cursor,
            modes,
            events: self.listener.drain(),
            retention_rebased,
        }
    }

    fn damage_candidates(&mut self) -> (bool, BTreeSet<usize>) {
        let result = match self.term.damage() {
            TermDamage::Full => (true, BTreeSet::new()),
            TermDamage::Partial(lines) => {
                let candidates = lines.map(|line| line.line).collect();
                (false, candidates)
            }
        };
        self.term.reset_damage();
        result
    }

    fn replace_visible_cache(&mut self) {
        let range = self.visible_range();
        self.visible_cache = (0..range.count)
            .map(|offset| {
                let index = RowIndex(range.first.0 + u64::from(offset));
                (index, self.row(index))
            })
            .collect();
    }
}

fn validate_size(size: GridSize) -> Result<(), EmulatorError> {
    if size.columns < 2 || size.rows == 0 {
        Err(EmulatorError::InvalidSize)
    } else {
        Ok(())
    }
}

fn cursor(term: &Term<Listener>, first_retained: u64) -> CursorState {
    let content = term.renderable_content();
    CursorState {
        row: RowIndex(
            first_retained + term.history_size() as u64 + content.cursor.point.line.0 as u64,
        ),
        column: ColumnIndex(content.cursor.point.column.0 as u16),
        shape: convert::cursor_shape(content.cursor.shape),
        blinking: term.cursor_style().blinking,
    }
}

fn modes(term: &Term<Listener>) -> TerminalModes {
    let mode = term.mode();
    let mouse_tracking = if mode.contains(TermMode::MOUSE_MOTION) {
        MouseTrackingMode::Motion
    } else if mode.contains(TermMode::MOUSE_DRAG) {
        MouseTrackingMode::Drag
    } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        MouseTrackingMode::Click
    } else {
        MouseTrackingMode::Off
    };
    TerminalModes {
        alternate_screen: mode.contains(TermMode::ALT_SCREEN),
        application_cursor: mode.contains(TermMode::APP_CURSOR),
        bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        mouse_reporting: mouse_tracking != MouseTrackingMode::Off,
        mouse_tracking,
        mouse_encoding: if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Legacy
        },
        focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
        origin_mode: mode.contains(TermMode::ORIGIN),
        auto_wrap: mode.contains(TermMode::LINE_WRAP),
    }
}
