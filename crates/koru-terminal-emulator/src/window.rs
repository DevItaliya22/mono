// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::{RowIndex, RowRange, TerminalRow};

use crate::MAX_PAGE_ROWS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmulatorError {
    InvalidSize,
    EmptyRange,
    PageTooLarge { maximum: u16 },
    RowOutsideRetention,
}

impl std::fmt::Display for EmulatorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSize => {
                formatter.write_str("terminal size must be at least 2 columns by 1 row")
            }
            Self::EmptyRange => formatter.write_str("terminal row range cannot be empty"),
            Self::PageTooLarge { maximum } => {
                write!(
                    formatter,
                    "terminal row request exceeds the {maximum}-row limit"
                )
            }
            Self::RowOutsideRetention => formatter.write_str("terminal row is no longer retained"),
        }
    }
}

impl std::error::Error for EmulatorError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedRange {
    pub first: RowIndex,
    pub end_exclusive: RowIndex,
}

impl RetainedRange {
    pub fn len(self) -> u64 {
        self.end_exclusive.0.saturating_sub(self.first.0)
    }

    pub fn is_empty(self) -> bool {
        self.first == self.end_exclusive
    }
}

/// Complete rows for one explicitly bounded request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowWindow {
    pub requested: RowRange,
    pub retained: RetainedRange,
    pub rows: Vec<TerminalRow>,
    /// Testable proof that reads are proportional to this window, not history.
    pub visited_cells: usize,
}

impl RowWindow {
    pub fn has_earlier(&self) -> bool {
        self.rows
            .first()
            .is_some_and(|row| row.index > self.retained.first)
    }

    pub fn has_later(&self) -> bool {
        self.rows
            .last()
            .is_some_and(|row| row.index.0 + 1 < self.retained.end_exclusive.0)
    }
}

pub(crate) fn validate_range(range: RowRange) -> Result<(), EmulatorError> {
    if range.count == 0 {
        Err(EmulatorError::EmptyRange)
    } else if range.count > MAX_PAGE_ROWS {
        Err(EmulatorError::PageTooLarge {
            maximum: MAX_PAGE_ROWS,
        })
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Simple,
    Block,
    Lines,
    Semantic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionPoint {
    pub row: RowIndex,
    pub column: koru_session_protocol::ColumnIndex,
}
