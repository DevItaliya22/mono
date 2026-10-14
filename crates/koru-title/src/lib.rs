// SPDX-License-Identifier: MPL-2.0

//! Presentation-neutral title rules shared by desktop and mobile.

use std::borrow::Cow;

/// What a thing with no title of its own is called. A literal, deliberately:
/// this crate links only `emojis`, and the milestone is English-only by owner
/// decision — `koru_i18n::Key::DocUntitled` stays where it is and is not wired
/// up here ([ADR-287]).
///
/// [ADR-287]: ../../../docs/decisions/ADR-276-300.md#adr-287-taskread--the-tasks-query-is-a-second-question-asked-of-the-one-read-model
pub const UNTITLED: &str = "Untitled";

/// A thing's title: its own text, else its authored name, else [`UNTITLED`].
///
/// **The one chain.** A node's text is what a person typed into it; `name` is
/// the authored label a node carries when its text is not a title — an image's
/// caption, a file's filename, a list block's label. Whitespace does not count
/// as a title, so a row holding only spaces falls through rather than
/// rendering as a blank line in a list.
///
/// Three call sites in this tree carry two different fallbacks and a fourth
/// spells it "Untitled heading"; this is what they converge on, and what the
/// read projection answers a task row's title with — one chain, computed the
/// same way on both sides ([ADR-287]).
///
/// [ADR-287]: ../../../docs/decisions/ADR-276-300.md#adr-287-taskread--the-tasks-query-is-a-second-question-asked-of-the-one-read-model
pub fn resolve_title<'a>(text: &'a str, name: Option<&'a str>) -> Cow<'a, str> {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        return Cow::Borrowed(trimmed);
    }
    if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) {
        return Cow::Borrowed(name);
    }
    Cow::Borrowed(UNTITLED)
}

/// Longest leading emoji sequence recognised by `emojis`, and the remainder
/// with leading whitespace stripped.
///
/// Only a prefix counts: an emoji in the middle of a title remains text. When
/// the title contains only an emoji, the remainder is empty.
pub fn split_leading_emoji(title: &str) -> (Option<&str>, &str) {
    let mut best = 0;
    for i in title
        .char_indices()
        .map(|(i, _)| i)
        .skip(1)
        .chain(std::iter::once(title.len()))
    {
        if emojis::get(&title[..i]).is_some() {
            best = i;
        }
    }
    if best == 0 {
        return (None, title);
    }
    (Some(&title[..best]), title[best..].trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chain, in order, and the fallback at the end of it.
    #[test]
    fn a_title_is_its_text_then_its_name_then_untitled() {
        assert_eq!(resolve_title("Ship it", None), "Ship it");
        assert_eq!(resolve_title("Ship it", Some("notes.md")), "Ship it");
        assert_eq!(resolve_title("", Some("notes.md")), "notes.md");
        assert_eq!(resolve_title("", None), "Untitled");
        assert_eq!(resolve_title("", Some("")), "Untitled");
    }

    /// Whitespace is not a title. A row holding two spaces would otherwise
    /// render as a blank line in a list of things you can click.
    #[test]
    fn whitespace_falls_through_the_chain() {
        assert_eq!(resolve_title("   ", Some("photo.png")), "photo.png");
        assert_eq!(resolve_title("\t\n", None), "Untitled");
        assert_eq!(resolve_title("  Ship it  ", None), "Ship it");
        assert_eq!(resolve_title("", Some("  photo.png ")), "photo.png");
    }

    #[test]
    fn splits_only_one_complete_leading_emoji() {
        assert_eq!(split_leading_emoji("🚀 Ship it"), (Some("🚀"), "Ship it"));
        assert_eq!(split_leading_emoji("Price 🚀"), (None, "Price 🚀"));
        assert_eq!(split_leading_emoji("👨‍👩‍👧 Family"), (Some("👨‍👩‍👧"), "Family"));
    }
}
