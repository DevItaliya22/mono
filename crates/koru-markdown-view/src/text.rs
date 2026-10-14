// SPDX-License-Identifier: MPL-2.0

//! Every word this surface says, and the one place each is assembled.
//!
//! The same three rules `koru-artifacts-ui`'s `text.rs` keeps: nothing is a
//! literal, every size goes through `koru_i18n::number::bytes`, and a
//! sentence is built here rather than at the place that draws it.

use koru_i18n::{Key, Lang, fill, number, t};

/// *Reading…* — asked for, nothing back yet.
pub fn reading(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownReading)
}

/// The file has no text in it. A state said out loud, never an empty column.
pub fn empty_document(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownEmptyDocument)
}

/// The row that lays out the next page of a long document.
pub fn show_rest(lang: Lang, remaining: usize) -> String {
    fill(
        t(lang, Key::MarkdownShowRest),
        &[&number::decimal(lang, remaining as u64)],
    )
}

/// **What the reader is looking at**, said before anything else: the file as
/// Koru reads Markdown, which is also what a save would write back.
pub fn as_koru_reads_it(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownAsKoruReadsIt)
}

/// The line over the composer while an edit is open.
pub fn editing(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownEditing)
}

/// This is not Markdown, so it is named rather than drawn.
pub fn not_markdown(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownNotMarkdown)
}

/// The bytes are here and could not be read as a document.
pub fn unreadable(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownUnreadable)
}

/// D27, in this surface's words: the content is somewhere else, so there is
/// nothing here to draw and the device that has it is named when it can be.
pub fn held_elsewhere(lang: Lang, device: Option<&str>) -> String {
    match device {
        Some(name) => fill(t(lang, Key::MarkdownHeldElsewhere), &[name]),
        None => t(lang, Key::MarkdownHeldElsewhereUnknown).to_owned(),
    }
}

pub fn type_label(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownTypeLabel)
}

pub fn size_label(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownSizeLabel)
}

pub fn file_label(lang: Lang) -> &'static str {
    t(lang, Key::MarkdownFileLabel)
}

/// A size, in the person's language.
pub fn size(lang: Lang, bytes: u64) -> String {
    number::bytes(lang, bytes)
}
