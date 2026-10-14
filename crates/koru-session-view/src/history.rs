// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeSet, VecDeque};

use koru_session_protocol::{
    ErrorCode, RowIndex, RowRange, ScrollbackPage, SessionId, TerminalGeneration, TerminalRow,
};

pub const PAGE_CACHE_ROW_BUDGET: usize = 2_048;
pub const PAGE_CACHE_BYTE_BUDGET: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PageKey {
    generation: u64,
    first: u64,
    count: u16,
}

struct CachedPage {
    key: PageKey,
    rows: Vec<TerminalRow>,
    bytes: usize,
}

#[derive(Default)]
pub(crate) struct PageCache {
    pages: VecDeque<CachedPage>,
    pending: BTreeSet<PageKey>,
    rows: usize,
    bytes: usize,
}

impl PageCache {
    pub(crate) fn clear(&mut self) {
        self.pages.clear();
        self.pending.clear();
        self.rows = 0;
        self.bytes = 0;
    }

    pub(crate) fn request_missing(
        &mut self,
        generation: TerminalGeneration,
        range: RowRange,
    ) -> bool {
        if self.covers(generation, range) {
            return false;
        }
        self.pending.insert(key(generation, range))
    }

    pub(crate) fn insert(
        &mut self,
        page: ScrollbackPage,
        session_id: SessionId,
        generation: TerminalGeneration,
    ) -> Result<(), ErrorCode> {
        page.validate()?;
        if page.session_id != session_id || page.terminal_generation != generation {
            return Err(ErrorCode::StaleGeneration);
        }
        let page_key = key(generation, page.range);
        self.pending.remove(&page_key);
        if self.pages.iter().any(|cached| cached.key == page_key) {
            return Ok(());
        }
        let bytes = page.rows.iter().map(row_bytes).sum();
        self.rows += page.rows.len();
        self.bytes += bytes;
        self.pages.push_back(CachedPage {
            key: page_key,
            rows: page.rows,
            bytes,
        });
        while self.rows > PAGE_CACHE_ROW_BUDGET || self.bytes > PAGE_CACHE_BYTE_BUDGET {
            let Some(evicted) = self.pages.pop_front() else {
                break;
            };
            self.rows = self.rows.saturating_sub(evicted.rows.len());
            self.bytes = self.bytes.saturating_sub(evicted.bytes);
        }
        Ok(())
    }

    pub(crate) fn row(
        &self,
        generation: TerminalGeneration,
        index: RowIndex,
    ) -> Option<&TerminalRow> {
        self.pages.iter().rev().find_map(|page| {
            (page.key.generation == generation.get())
                .then(|| page.rows.iter().find(|row| row.index == index))
                .flatten()
        })
    }

    pub(crate) fn covers(&self, generation: TerminalGeneration, range: RowRange) -> bool {
        let end = range.first.0 + u64::from(range.count);
        (range.first.0..end).all(|index| self.row(generation, RowIndex(index)).is_some())
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, usize, usize) {
        (self.pages.len(), self.rows, self.bytes)
    }
}

fn key(generation: TerminalGeneration, range: RowRange) -> PageKey {
    PageKey {
        generation: generation.get(),
        first: range.first.0,
        count: range.count,
    }
}

fn row_bytes(row: &TerminalRow) -> usize {
    std::mem::size_of::<TerminalRow>()
        + row.cells.len() * std::mem::size_of::<koru_session_protocol::TerminalCell>()
        + row.cells.iter().map(|cell| cell.text.len()).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_session_protocol::{StreamSequence, ViewportGeneration};

    fn generation() -> TerminalGeneration {
        TerminalGeneration::new(1).unwrap()
    }

    fn page(session_id: SessionId, first: u64) -> ScrollbackPage {
        let range = RowRange {
            first: RowIndex(first),
            count: 512,
        };
        ScrollbackPage {
            session_id,
            terminal_generation: generation(),
            viewport_generation: ViewportGeneration::new(1).unwrap(),
            sequence: StreamSequence::new(1).unwrap(),
            range,
            rows: (first..first + 512)
                .map(|index| TerminalRow {
                    index: RowIndex(index),
                    cells: Vec::new(),
                    wrapped_to_next: false,
                })
                .collect(),
            has_earlier: first > 0,
            has_later: true,
        }
    }

    #[test]
    fn cache_evicts_by_fixed_row_and_byte_budgets() {
        let session_id = SessionId::mint();
        let mut cache = PageCache::default();
        for first in (0..512 * 6).step_by(512) {
            cache
                .insert(page(session_id, first), session_id, generation())
                .unwrap();
        }
        let (pages, rows, bytes) = cache.usage();
        assert!(pages < 6);
        assert!(rows <= PAGE_CACHE_ROW_BUDGET);
        assert!(bytes <= PAGE_CACHE_BYTE_BUDGET);
        assert!(cache.row(generation(), RowIndex(0)).is_none());
        assert!(cache.row(generation(), RowIndex(512 * 5)).is_some());
    }

    #[test]
    fn one_missing_range_has_one_in_flight_request() {
        let mut cache = PageCache::default();
        let range = RowRange {
            first: RowIndex(10),
            count: 40,
        };
        assert!(cache.request_missing(generation(), range));
        assert!(!cache.request_missing(generation(), range));
    }
}
