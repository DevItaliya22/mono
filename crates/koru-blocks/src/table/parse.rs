// SPDX-License-Identifier: MPL-2.0

use std::ops::Range;

use koru_core::{Mark, MarkSet};
use pulldown_cmark::{Alignment as CmarkAlignment, Event, Options, Parser, Tag, TagEnd};

#[derive(Copy, Clone, PartialEq, Eq, Default, Debug)]
pub enum Alignment {
    #[default]
    Start,
    Center,
    End,
}

impl From<CmarkAlignment> for Alignment {
    fn from(value: CmarkAlignment) -> Self {
        match value {
            CmarkAlignment::None | CmarkAlignment::Left => Self::Start,
            CmarkAlignment::Center => Self::Center,
            CmarkAlignment::Right => Self::End,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InlineSpan {
    /// UTF-8 byte range in [`TableCell::text`].
    pub range: Range<usize>,
    pub marks: MarkSet,
    pub link: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TableCell {
    /// Display text with Markdown delimiters removed.
    pub text: String,
    /// The exact Markdown inside this cell, excluding its separator pipes.
    pub markdown: String,
    /// UTF-8 byte range in the table node's canonical source.
    pub source_range: Range<usize>,
    pub spans: Vec<InlineSpan>,
    /// One source token for each displayed Unicode scalar.
    ///
    /// Markdown delimiters do not appear in `text`, and escapes can make one
    /// displayed scalar occupy two source scalars. Keeping the correspondence
    /// here lets a projection select or replace rendered text without guessing
    /// its way back into the canonical Markdown.
    pub(crate) source_units: Vec<DisplaySourceUnit>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct DisplaySourceUnit {
    display: Range<usize>,
    source: Range<usize>,
}

impl TableCell {
    /// Map a UTF-8 byte range in rendered [`text`](Self::text) back to the
    /// smallest source range that produces it.
    pub fn source_range_for_display(&self, display: Range<usize>) -> Option<Range<usize>> {
        if display.start >= display.end
            || display.end > self.text.len()
            || !self.text.is_char_boundary(display.start)
            || !self.text.is_char_boundary(display.end)
        {
            return None;
        }
        let first = self
            .source_units
            .iter()
            .find(|unit| unit.display.start == display.start)?;
        let last = self
            .source_units
            .iter()
            .find(|unit| unit.display.end == display.end)?;
        Some(first.source.start..last.source.end)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedTable {
    pub alignments: Vec<Alignment>,
    pub header: Vec<TableCell>,
    pub rows: Vec<Vec<TableCell>>,
    /// Range occupied by the table itself. Reference definitions may follow.
    pub source_range: Range<usize>,
    /// Source after the table, preserved verbatim (normally link definitions).
    pub trailing: String,
}

impl ParsedTable {
    pub fn columns(&self) -> usize {
        self.alignments.len()
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SyntaxErrorCode {
    Empty,
    MissingDelimiter,
    InvalidDelimiter,
    ColumnCountMismatch,
    ExtraCells,
    MultipleTables,
    UnexpectedContent,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxError {
    pub code: SyntaxErrorCode,
    /// One-based line and column for display.
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl SyntaxError {
    fn new(code: SyntaxErrorCode, line: usize, message: impl Into<String>) -> Self {
        Self {
            code,
            line,
            column: 1,
            message: message.into(),
        }
    }
}

#[derive(Default)]
struct Marks {
    bold: u16,
    italic: u16,
    strike: u16,
    code: u16,
}

impl Marks {
    fn change(&mut self, mark: Mark, opening: bool) {
        let depth = match mark {
            Mark::Bold => &mut self.bold,
            Mark::Italic => &mut self.italic,
            Mark::Strike => &mut self.strike,
            Mark::Code => &mut self.code,
            _ => return,
        };
        *depth = if opening {
            depth.saturating_add(1)
        } else {
            depth.saturating_sub(1)
        };
    }

    fn set(&self) -> MarkSet {
        let mut set = MarkSet::default();
        if self.bold > 0 {
            set.set(Mark::Bold, true);
        }
        if self.italic > 0 {
            set.set(Mark::Italic, true);
        }
        if self.strike > 0 {
            set.set(Mark::Strike, true);
        }
        if self.code > 0 {
            set.set(Mark::Code, true);
        }
        set
    }
}

struct BuildingCell {
    source_range: Range<usize>,
    text: String,
    spans: Vec<InlineSpan>,
    source_units: Vec<DisplaySourceUnit>,
}

impl BuildingCell {
    fn append(
        &mut self,
        value: &str,
        source: &str,
        source_start: usize,
        marks: &Marks,
        link: Option<&str>,
    ) {
        if value.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(value);
        let end = self.text.len();
        let style = marks.set();
        let link = link.map(str::to_owned);
        match self.spans.last_mut() {
            Some(last) if last.range.end == start && last.marks == style && last.link == link => {
                last.range.end = end;
            }
            _ => self.spans.push(InlineSpan {
                range: start..end,
                marks: style,
                link,
            }),
        }
        self.source_units
            .extend(display_source_units(value, source, source_start, start));
    }
}

/// Align one pulldown-cmark inline event with the source bytes that produced
/// it. Ordinary text is one-to-one; the scan also folds a Markdown escape into
/// the scalar after it and skips formatting/link/code delimiters.
fn display_source_units(
    display: &str,
    source: &str,
    source_start: usize,
    display_start: usize,
) -> Vec<DisplaySourceUnit> {
    let mut units = Vec::with_capacity(display.chars().count());
    let mut source_at = 0usize;
    for (display_offset, wanted) in display.char_indices() {
        let mut found = None;
        while source_at < source.len() {
            let rest = &source[source_at..];
            let Some((relative, candidate)) = rest.char_indices().next() else {
                break;
            };
            debug_assert_eq!(relative, 0);
            let candidate_end = source_at + candidate.len_utf8();
            if candidate == '\\' {
                if let Some(next) = source[candidate_end..].chars().next() {
                    let end = candidate_end + next.len_utf8();
                    if next == wanted {
                        found = Some(source_at..end);
                        source_at = end;
                        break;
                    }
                }
            }
            if candidate == wanted {
                found = Some(source_at..candidate_end);
                source_at = candidate_end;
                break;
            }
            source_at = candidate_end;
        }
        let source_range = found.unwrap_or_else(|| {
            // Pulldown can decode an entity into a scalar that is not literally
            // present. The event range is still the authoritative source; use
            // the remaining envelope rather than manufacture an invalid byte.
            let start = source_at.min(source.len());
            source_at = source.len();
            start..source.len()
        });
        units.push(DisplaySourceUnit {
            display: (display_start + display_offset)
                ..(display_start + display_offset + wanted.len_utf8()),
            source: (source_start + source_range.start)..(source_start + source_range.end),
        });
    }
    units
}

/// Parse exactly one GFM table. Pulldown-cmark owns Markdown recognition; the
/// checks around it only prevent its deliberately lenient row handling from
/// silently discarding source cells.
pub fn parse(source: &str) -> Result<ParsedTable, Vec<SyntaxError>> {
    if source.trim().is_empty() {
        return Err(vec![SyntaxError::new(
            SyntaxErrorCode::Empty,
            1,
            "Type a header row and a delimiter row",
        )]);
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let mut table_count = 0usize;
    let mut table_range = None;
    let mut alignments = Vec::new();
    let mut header = Vec::new();
    let mut rows: Vec<Vec<TableCell>> = Vec::new();
    let mut body_row: Option<Vec<TableCell>> = None;
    let mut in_head = false;
    let mut cell: Option<BuildingCell> = None;
    let mut marks = Marks::default();
    let mut links: Vec<String> = Vec::new();

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Table(found)) => {
                table_count += 1;
                if table_count == 1 {
                    table_range = Some(range);
                    alignments = found.into_iter().map(Alignment::from).collect();
                }
            }
            Event::Start(Tag::TableHead) if table_count == 1 => in_head = true,
            Event::End(TagEnd::TableHead) if table_count == 1 => in_head = false,
            Event::Start(Tag::TableRow) if table_count == 1 && !in_head => {
                body_row = Some(Vec::new());
            }
            Event::End(TagEnd::TableRow) if table_count == 1 && !in_head => {
                rows.push(body_row.take().unwrap_or_default());
            }
            Event::Start(Tag::TableCell) if table_count == 1 => {
                cell = Some(BuildingCell {
                    source_range: range,
                    text: String::new(),
                    spans: Vec::new(),
                    source_units: Vec::new(),
                });
            }
            Event::End(TagEnd::TableCell) if table_count == 1 => {
                if let Some(cell) = cell.take() {
                    let raw = source
                        .get(cell.source_range.clone())
                        .unwrap_or_default()
                        .to_owned();
                    let finished = TableCell {
                        text: cell.text,
                        markdown: raw,
                        source_range: cell.source_range,
                        spans: cell.spans,
                        source_units: cell.source_units,
                    };
                    if in_head {
                        header.push(finished);
                    } else {
                        body_row.get_or_insert_with(Vec::new).push(finished);
                    }
                }
            }
            Event::Start(Tag::Strong) => marks.change(Mark::Bold, true),
            Event::End(TagEnd::Strong) => marks.change(Mark::Bold, false),
            Event::Start(Tag::Emphasis) => marks.change(Mark::Italic, true),
            Event::End(TagEnd::Emphasis) => marks.change(Mark::Italic, false),
            Event::Start(Tag::Strikethrough) => marks.change(Mark::Strike, true),
            Event::End(TagEnd::Strikethrough) => marks.change(Mark::Strike, false),
            Event::Start(Tag::Link { dest_url, .. }) => links.push(dest_url.into_string()),
            Event::End(TagEnd::Link) => {
                links.pop();
            }
            Event::Text(value) if cell.is_some() => {
                let range = include_markdown_escape(source, range);
                let authored = source.get(range.clone()).unwrap_or_default();
                cell.as_mut().unwrap().append(
                    &value,
                    authored,
                    range.start,
                    &marks,
                    links.last().map(String::as_str),
                );
            }
            Event::Code(value) if cell.is_some() => {
                marks.change(Mark::Code, true);
                let authored = source.get(range.clone()).unwrap_or_default();
                cell.as_mut().unwrap().append(
                    &value,
                    authored,
                    range.start,
                    &marks,
                    links.last().map(String::as_str),
                );
                marks.change(Mark::Code, false);
            }
            _ => {}
        }
    }

    if table_count > 1 {
        return Err(vec![SyntaxError::new(
            SyntaxErrorCode::MultipleTables,
            1,
            "A table block can contain only one table",
        )]);
    }
    let Some(table_range) = table_range else {
        return Err(diagnose_unrecognised(source));
    };

    let mut errors = Vec::new();
    let columns = alignments.len();
    let table_source = source.get(table_range.clone()).unwrap_or_default();
    let source_lines: Vec<_> = table_source.lines().collect();
    let header_columns = source_lines
        .first()
        .map_or(0, |line| split_cells(line).len());
    if header.len() != columns || header_columns != columns {
        errors.push(SyntaxError::new(
            SyntaxErrorCode::ColumnCountMismatch,
            2,
            "The header and delimiter row must have the same number of columns",
        ));
    }
    for (index, line) in source_lines.iter().skip(2).enumerate() {
        let cells = split_cells(line).len();
        if cells != columns {
            let code = if cells > columns {
                SyntaxErrorCode::ExtraCells
            } else {
                SyntaxErrorCode::ColumnCountMismatch
            };
            errors.push(SyntaxError::new(
                code,
                index + 3,
                format!(
                    "This row has {} cells; the table has {columns} columns",
                    cells
                ),
            ));
        }
    }
    let trailing = source.get(table_range.end..).unwrap_or_default().to_owned();
    if let Some((index, _)) = trailing
        .lines()
        .enumerate()
        .find(|(_, line)| !line.trim().is_empty() && !is_reference_definition(line))
    {
        errors.push(SyntaxError::new(
            SyntaxErrorCode::UnexpectedContent,
            source_lines.len() + index + 1,
            "A table block can contain only one table and its link definitions",
        ));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ParsedTable {
        alignments,
        header,
        rows,
        source_range: table_range,
        trailing,
    })
}

/// Pulldown's offset for an escaped scalar begins at the scalar, not at the
/// backslash it consumes. Replacement must consume that escape too or `a\|b`
/// would become `a\newb` and leave a stray slash behind.
fn include_markdown_escape(source: &str, range: Range<usize>) -> Range<usize> {
    let slashes = source[..range.start]
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count();
    if slashes % 2 == 1 {
        range.start - 1..range.end
    } else {
        range
    }
}

fn is_reference_definition(line: &str) -> bool {
    let line = line.trim_start();
    let Some(close) = line.find("]:") else {
        return false;
    };
    line.starts_with('[') && close > 1 && !line[close + 2..].trim().is_empty()
}

fn diagnose_unrecognised(source: &str) -> Vec<SyntaxError> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.len() < 2 {
        return vec![SyntaxError::new(
            SyntaxErrorCode::MissingDelimiter,
            2,
            "Add a delimiter row such as | --- | --- |",
        )];
    }
    let delimiter = split_cells(lines[1]);
    if delimiter.is_empty() || delimiter.iter().any(|cell| !is_delimiter(cell)) {
        return vec![SyntaxError::new(
            SyntaxErrorCode::InvalidDelimiter,
            2,
            "Each delimiter cell needs hyphens, with optional alignment colons",
        )];
    }
    let header = split_cells(lines[0]);
    if header.len() != delimiter.len() {
        return vec![SyntaxError::new(
            SyntaxErrorCode::ColumnCountMismatch,
            2,
            format!(
                "The header has {} cells but the delimiter has {}",
                header.len(),
                delimiter.len()
            ),
        )];
    }
    vec![SyntaxError::new(
        SyntaxErrorCode::UnexpectedContent,
        1,
        "This source is not one GFM table",
    )]
}

fn is_delimiter(cell: &str) -> bool {
    let value = cell.trim().trim_start_matches(':').trim_end_matches(':');
    !value.is_empty() && value.bytes().all(|b| b == b'-')
}

fn split_cells(line: &str) -> Vec<&str> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut start = 0usize;
    let mut escaped = false;
    for (i, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '|' {
            cells.push(&line[start..i]);
            start = i + 1;
        }
    }
    cells.push(&line[start..]);
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alignment_marks_links_and_source_ranges() {
        let source = "| **App** | `File` |\n| :--- | ---: |\n| [Koru](https://koru.test) | a\\|b |";
        let table = parse(source).unwrap();
        assert_eq!(table.alignments, vec![Alignment::Start, Alignment::End]);
        assert_eq!(table.header[0].text, "App");
        assert!(table.header[0].spans[0].marks.has(Mark::Bold));
        assert_eq!(
            table.rows[0][0].spans[0].link.as_deref(),
            Some("https://koru.test")
        );
        assert_eq!(table.rows[0][1].text, "a|b");
        assert_eq!(&source[table.rows[0][1].source_range.clone()], " a\\|b ");
        let escaped = source.find("a\\|b").unwrap();
        assert_eq!(
            table.rows[0][1].source_range_for_display(1..2),
            Some(escaped + 1..escaped + 3)
        );
    }

    #[test]
    fn reports_useful_incomplete_source_errors() {
        let error = parse("| A | B |").unwrap_err();
        assert_eq!(error[0].code, SyntaxErrorCode::MissingDelimiter);
        let error = parse("| A | B |\n| --- |").unwrap_err();
        assert_eq!(error[0].code, SyntaxErrorCode::ColumnCountMismatch);
    }

    #[test]
    fn body_rows_must_match_the_header_so_every_cell_has_a_source_range() {
        let error = parse("| A | B |\n| --- | --- |\n| one |").unwrap_err();
        assert_eq!(error[0].code, SyntaxErrorCode::ColumnCountMismatch);
    }

    #[test]
    fn arbitrary_trailing_content_is_not_hidden_behind_the_grid() {
        let error = parse("| A |\n| --- |\n| one |\n\nnot part of the table").unwrap_err();
        assert_eq!(error[0].code, SyntaxErrorCode::UnexpectedContent);
        assert!(parse("| A |\n| --- |\n| [one][1] |\n\n[1]: https://example.com").is_ok());
    }
}
