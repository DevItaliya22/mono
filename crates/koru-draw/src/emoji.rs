// SPDX-License-Identifier: MPL-2.0

//! Leading emoji in a title — one rule for Explore, tabs, and document pills.

pub use koru_title::split_leading_emoji;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(split_leading_emoji("Pricing"), (None, "Pricing"));
    }

    #[test]
    fn leading_emoji_is_split() {
        assert_eq!(
            split_leading_emoji("💻Craft.do Pricing"),
            (Some("💻"), "Craft.do Pricing")
        );
    }

    #[test]
    fn space_after_emoji_is_trimmed_from_the_label() {
        assert_eq!(split_leading_emoji("🚀 Ship it"), (Some("🚀"), "Ship it"));
    }

    #[test]
    fn emoji_only_leaves_an_empty_label() {
        assert_eq!(split_leading_emoji("🔥"), (Some("🔥"), ""));
    }

    #[test]
    fn mid_title_emoji_stays_in_the_label() {
        assert_eq!(split_leading_emoji("Price 💻"), (None, "Price 💻"));
    }

    #[test]
    fn leading_spaces_before_emoji_do_not_count() {
        assert_eq!(split_leading_emoji(" 🚀 Go"), (None, " 🚀 Go"));
    }

    #[test]
    fn zwj_sequence_is_one_emoji() {
        let family = "👨‍👩‍👧";
        let title = format!("{family} Family");
        assert_eq!(split_leading_emoji(&title), (Some(family), "Family"));
    }

    #[test]
    fn flag_is_one_emoji() {
        assert_eq!(split_leading_emoji("🇬🇧 UK"), (Some("🇬🇧"), "UK"));
    }

    #[test]
    fn empty_title() {
        assert_eq!(split_leading_emoji(""), (None, ""));
    }
}
