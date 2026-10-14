// SPDX-License-Identifier: MPL-2.0

//! Bulk purge is a sequence of independent edits; its report must say so.

use crate::ErrorReport;
use koru_i18n::{Key, Lang, t};

pub(super) fn purge_report<I, E>(
    ids: impl IntoIterator<Item = I>,
    lang: Lang,
    mut purge: impl FnMut(I) -> Result<bool, E>,
) -> Option<ErrorReport>
where
    E: std::fmt::Display,
{
    let mut removed = false;
    let mut failed = None;
    for id in ids {
        match purge(id) {
            Ok(changed) => removed |= changed,
            Err(error) => {
                failed.get_or_insert(error);
            }
        }
    }
    failed.map(|error| {
        let body = if removed {
            Key::TrashDeletePartial
        } else {
            Key::TrashDeleteFailedBody
        };
        ErrorReport::new(t(lang, Key::TrashDeleteFailedTitle), t(lang, body)).with_details(error)
    })
}

#[cfg(test)]
mod tests;
