// SPDX-License-Identifier: MPL-2.0

use super::{Alignment, ParsedTable, TableCell};

pub fn serialize(table: &ParsedTable) -> String {
    let mut out = String::new();
    write_row(&mut out, &table.header);
    out.push('\n');
    out.push('|');
    for alignment in &table.alignments {
        out.push(' ');
        out.push_str(match alignment {
            Alignment::Start => "---",
            Alignment::Center => ":---:",
            Alignment::End => "---:",
        });
        out.push_str(" |");
    }
    for row in &table.rows {
        out.push('\n');
        write_row(&mut out, row);
    }
    out.push_str(&table.trailing);
    out
}

fn write_row(out: &mut String, cells: &[TableCell]) {
    out.push('|');
    for cell in cells {
        out.push(' ');
        out.push_str(cell.markdown.trim());
        out.push_str(" |");
    }
}

fn empty_cell() -> TableCell {
    TableCell {
        text: String::new(),
        markdown: String::new(),
        source_range: 0..0,
        spans: Vec::new(),
        source_units: Vec::new(),
    }
}

pub fn insert_row(table: &mut ParsedTable, at: usize) {
    let row = vec![empty_cell(); table.columns()];
    table.rows.insert(at.min(table.rows.len()), row);
}

pub fn delete_row(table: &mut ParsedTable, at: usize) -> bool {
    if at >= table.rows.len() {
        return false;
    }
    table.rows.remove(at);
    true
}

pub fn insert_column(table: &mut ParsedTable, at: usize) {
    let at = at.min(table.columns());
    table.alignments.insert(at, Alignment::Start);
    table.header.insert(at, empty_cell());
    for row in &mut table.rows {
        row.insert(at, empty_cell());
    }
}

pub fn delete_column(table: &mut ParsedTable, at: usize) -> bool {
    if table.columns() <= 1 || at >= table.columns() {
        return false;
    }
    table.alignments.remove(at);
    table.header.remove(at);
    for row in &mut table.rows {
        row.remove(at);
    }
    true
}

pub fn set_alignment(table: &mut ParsedTable, column: usize, alignment: Alignment) -> bool {
    let Some(slot) = table.alignments.get_mut(column) else {
        return false;
    };
    *slot = alignment;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::parse;

    #[test]
    fn structural_edits_round_trip_through_markdown() {
        let mut table = parse("| A | B |\n| --- | ---: |\n| one | two |").unwrap();
        insert_row(&mut table, 1);
        insert_column(&mut table, 1);
        assert!(set_alignment(&mut table, 1, Alignment::Center));
        let source = serialize(&table);
        let reparsed = parse(&source).unwrap();
        assert_eq!(reparsed.columns(), 3);
        assert_eq!(reparsed.rows.len(), 2);
        assert_eq!(reparsed.alignments[1], Alignment::Center);
    }
}
