// SPDX-License-Identifier: MPL-2.0

//! GFM table behaviour, isolated from storage and presentation.
//!
//! The node stores Markdown. This module turns that source into a semantic
//! table with UTF-8 source ranges, validates the few places where permissive
//! Markdown would otherwise discard data, and serialises structural edits.

mod parse;
mod serialize;

pub use parse::{
    Alignment, InlineSpan, ParsedTable, SyntaxError, SyntaxErrorCode, TableCell, parse,
};
pub use serialize::{
    delete_column, delete_row, insert_column, insert_row, serialize, set_alignment,
};

/// Source shown for a new `/table` block until the user replaces it.
pub const EXAMPLE: &str = "| Column 1 | Column 2 |\n| --- | --- |\n|  |  |";

/// Render valid GFM source as semantic HTML for the system clipboard.
pub fn to_html(source: &str) -> Option<String> {
    parse(source).ok()?;
    let mut options = pulldown_cmark::Options::empty();
    options.insert(pulldown_cmark::Options::ENABLE_TABLES);
    options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new_ext(source, options));
    Some(html)
}
