// SPDX-License-Identifier: MPL-2.0

//! Canonical table properties.
//!
//! A table's cells remain GFM Markdown in the node's text.  This module owns
//! only state that is part of the shared document model; parsing, layout and
//! editing live in `koru-blocks` and `koru-editor` respectively.

/// How a table uses the horizontal room offered by a document.
///
/// Compact is represented by an absent metadata key, so every table written
/// before this property existed keeps the same default without migration.
#[derive(Copy, Clone, PartialEq, Eq, Default, Debug)]
pub enum TableLayout {
    #[default]
    Compact,
    Extended,
}

impl TableLayout {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Extended => "extended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "compact" => Some(Self::Compact),
            "extended" => Some(Self::Extended),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_tokens_round_trip() {
        for layout in [TableLayout::Compact, TableLayout::Extended] {
            assert_eq!(TableLayout::parse(layout.as_str()), Some(layout));
        }
        assert_eq!(TableLayout::parse("wide"), None);
    }
}
