// SPDX-License-Identifier: MPL-2.0

//! Which rows of the Welcome page have been ticked on **this installation**.
//!
//! **Machine-wide, not per Profile.** A person who learnt their way around
//! Koru under one Profile has learnt it for every Profile on the machine, so
//! the ticks are shared by every Profile here and keyed by nothing but the
//! row.
//!
//! **Why `store_state`.** It is the catalog's key/value table and is local to
//! one installation by construction — the peer id and the install id live
//! there for the same reason. Nothing in it is replicated, synced, exported or
//! seen by another device, which is exactly what a first-run checklist wants:
//! a new machine starts with a fresh Welcome page. Using it needs no table and
//! no migration.
//!
//! One key per row — `welcome.done.<row>` = `"1"` — so two windows ticking
//! two different rows can never overwrite each other's answer. The row ids
//! are the Welcome page's stable strings; the store does not know what they
//! mean and only checks that they are short, lowercase and dashed.

use super::*;

/// The `store_state` key prefix every tick is written under.
const KEY_PREFIX: &str = "welcome.done.";

/// The most rows one query may ask about.
pub const WELCOME_QUERY_MAX: usize = 32;

/// The longest row id accepted.
const ROW_MAX: usize = 40;

/// Whether `row` is a well-formed Welcome row id: 1..=40 characters of
/// `[a-z0-9-]`. Anything else is refused rather than written, so a key in
/// `store_state` can never be named by arbitrary caller text.
pub fn valid_welcome_row(row: &str) -> bool {
    (1..=ROW_MAX).contains(&row.len())
        && row
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn checked(row: &str) -> Result<String> {
    if valid_welcome_row(row) {
        Ok(format!("{KEY_PREFIX}{row}"))
    } else {
        Err(Error::new(
            Code::InvalidRequest,
            "a Welcome row id is 1 to 40 characters of a-z, 0-9 and -",
        ))
    }
}

impl<C: Catalog, B: Backend> Store<C, B> {
    /// The rows of `rows` that are ticked, in the order asked. A row never
    /// ticked is absent rather than an error.
    pub fn welcome_progress(&self, rows: &[String]) -> Result<Vec<String>> {
        if rows.len() > WELCOME_QUERY_MAX {
            return Err(Error::new(
                Code::InvalidRequest,
                format!("at most {WELCOME_QUERY_MAX} Welcome rows may be asked about at once"),
            ));
        }
        let keys = rows
            .iter()
            .map(|row| checked(row))
            .collect::<Result<Vec<_>>>()?;
        let mut done = Vec::new();
        for (row, key) in rows.iter().zip(keys) {
            if self.catalog.state(&key)?.is_some_and(|value| value == "1") {
                done.push(row.clone());
            }
        }
        Ok(done)
    }

    /// Tick `row` for every Profile on this installation. Ticking a row that
    /// is already ticked is a no-op that still answers `Ok`.
    pub fn mark_welcome_done(&mut self, row: &str) -> Result<()> {
        let key = checked(row)?;
        self.catalog.set_state(&key, "1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_id_is_short_lowercase_and_dashed() {
        for good in ["learn", "first-document", "a", "row-2", &"x".repeat(40)] {
            assert!(valid_welcome_row(good), "{good}");
        }
        for bad in [
            "",
            "Learn",
            "first_document",
            "a.b",
            "../peer_id",
            "with space",
            &"x".repeat(41),
            "é",
        ] {
            assert!(!valid_welcome_row(bad), "{bad}");
        }
    }

    #[test]
    fn a_tick_is_keyed_by_the_row_alone() {
        assert_eq!(checked("learn").unwrap(), "welcome.done.learn");
        assert_eq!(checked("Nope").unwrap_err().code, Code::InvalidRequest);
    }
}
