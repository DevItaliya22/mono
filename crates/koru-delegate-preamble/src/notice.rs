// SPDX-License-Identifier: MPL-2.0

//! Why an edited copy was not used, and the one sentence a surface shows.

use koru_i18n::{Key, Lang, fill, number::decimal, t};

/// The objective faults D35 allows, minus *missing*, which is not news.
///
/// Closed, and carrying counts only: a surface renders it through
/// [`Notice::message`] and may show the file's path beside the sentence,
/// but the sentence never contains one and neither does this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The file exists and holds nothing but whitespace.
    Empty,
    /// The file could not be opened or read — permissions, or something at
    /// the path that is not a file.
    Unreadable,
    /// The bytes are not UTF-8. Usually a file saved as UTF-16 by an editor
    /// that offered it.
    BadEncoding,
    /// The file is larger than [`crate::SIZE_LIMIT_BYTES`].
    Oversized {
        /// The file's size, as far as it was measured.
        bytes: u64,
        /// The limit it exceeded.
        limit: u64,
    },
}

impl Notice {
    /// The sentence for `lang`, from `koru-i18n`. Says that the edited
    /// instructions were not used and the built-in ones were, and why.
    pub fn message(self, lang: Lang) -> String {
        match self {
            Notice::Empty => t(lang, Key::DelegatePreambleEmpty).to_owned(),
            Notice::Unreadable => t(lang, Key::DelegatePreambleUnreadable).to_owned(),
            Notice::BadEncoding => t(lang, Key::DelegatePreambleBadEncoding).to_owned(),
            Notice::Oversized { bytes, limit } => fill(
                t(lang, Key::DelegatePreambleOversized),
                &[&decimal(lang, bytes), &decimal(lang, limit)],
            ),
        }
    }
}
