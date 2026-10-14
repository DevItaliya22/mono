// SPDX-License-Identifier: MPL-2.0

//! The list of profiles this machine knows about.
//!
//! One profile is one folder, so this file answers exactly one question: *which
//! folders*. It is an **index and nothing else**. Every folder it names also
//! describes itself — `profile.conf` beside the profile document — so losing
//! this file loses nothing that re-picking a folder does not restore.
//!
//! That is not a consolation, it is a requirement. This file lives outside every
//! data directory, which means `--backup` does not cover it, copying a profile
//! folder to another machine does not carry it, and a cloud drive does not sync
//! it. Anything that existed *only* here would be something a person could lose
//! by reinstalling.
//!
//! **The `last-selected` marker is the remembered source.** A plain launch
//! reopens the profile whose id this file names — see [`selected_id`]; every
//! open stamps it. This reverses an earlier rule that the registry was "never a
//! resolution source", justified because the marker is set only by an explicit
//! profile open, never derived or guessed. `--profile <name|id|path>` still
//! looks a folder up here and enters as the user's explicit choice.
//!
//! Parsing is deliberately in the tolerant, hand-editable style the other conf
//! files here use, rather than serde: one record per line, `#` comments, a
//! stripped byte-order mark, and **a bad line drops on its own** instead of
//! taking the file with it.
//!
//! ```text
//! # Koru profiles:  <id> | <name> | <email> | <last opened> | <folder>
//! last-selected 6f0e5b2c-…
//! 6f0e5b2c-… | Work     | sam@acme.com | 20260801-120000 | C:\Users\sam\Koru\Work
//! 9a1c33f0-… | Personal |              | 20260731-201500 | D:\Dropbox\Koru\Personal
//! ```

use std::path::{Path, PathBuf};

use koru_core::ProfileId;

/// Lives beside the executable (or its per-user fallback), so the shell — the
/// only thing that knows where that is — owns the path.
pub const REGISTRY_FILE: &str = "profiles.conf";

/// The directive prefix naming the profile a plain launch reopens.
///
/// A `last-selected <ProfileId>` line records the last profile the user opened;
/// [`selected_id`] reads it. [`parse`] ignores the line — an older build's
/// [`parse_line`] drops it via `ProfileId::parse` failure — so it is
/// forward-compatible.
const SELECTED: &str = "last-selected";

/// The separator.
///
/// `|` because it is **illegal in a Windows path**, so the folder — which is
/// everything after the fourth one — can contain anything a path can and still
/// come back in one piece.
///
/// A display name *can* contain one, though, and then every field after it would
/// shift by one and the row would name the wrong folder. So [`body`] replaces it
/// on the way out: the separator is reserved, and reserving it in the writer is
/// the only place that costs nothing.
const SEP: char = '|';

/// How many separators come before the folder. Four fields precede it.
const FIELDS_BEFORE_DIR: usize = 4;

/// One profile, as the index records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRecord {
    pub id: ProfileId,
    pub name: String,
    /// Blank rather than absent when there is none — a profile is a local record
    /// until there is something to sign in to.
    pub email: Option<String>,
    /// [`crate::backup::stamp`] format. A string rather than a `SystemTime`
    /// because it is only sorted and shown, and a UTC stamp in that shape sorts
    /// correctly as plain text.
    pub last_opened: Option<String>,
    pub dir: PathBuf,
}

impl ProfileRecord {
    pub fn new(id: ProfileId, name: &str, email: Option<&str>, dir: &Path) -> Self {
        Self {
            id,
            name: name.to_string(),
            email: email.map(str::to_string),
            last_opened: None,
            dir: dir.to_path_buf(),
        }
    }
}

/// Read the file's body. A line that cannot be understood is skipped; one bad
/// line must never cost somebody the other five profiles.
pub fn parse(text: &str) -> Vec<ProfileRecord> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(parse_line)
        .collect()
}

/// The id of the profile a plain launch should reopen, if the file names one.
///
/// Scans for a `last-selected <id>` line. A UUID is unambiguous and no record
/// field starts with the word, so the directive cannot be mistaken for a row.
/// `None` when the file is silent — a first run, or a file written by a build
/// predating the marker.
pub fn selected_id(text: &str) -> Option<ProfileId> {
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if let Some(rest) = line.strip_prefix(SELECTED) {
            if let Some(id) = ProfileId::parse(rest.trim()) {
                return Some(id);
            }
        }
    }
    None
}

fn parse_line(line: &str) -> Option<ProfileRecord> {
    // `splitn` so everything after the fourth separator is the folder, in one
    // piece, whatever is in it.
    let mut parts = line.splitn(FIELDS_BEFORE_DIR + 1, SEP);
    let id = ProfileId::parse(parts.next()?.trim())?;
    let name = parts.next()?.trim().to_string();
    let email = non_empty(parts.next()?);
    let last_opened = non_empty(parts.next()?);
    let dir = parts.next()?.trim();
    if dir.is_empty() {
        // A record with no folder names nothing and cannot be opened. Dropping
        // it is the honest answer; keeping it would put a dead row in the
        // switcher that fails only when clicked.
        return None;
    }
    Some(ProfileRecord {
        id,
        name,
        email,
        last_opened,
        dir: PathBuf::from(dir),
    })
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The body to write back, carrying its own explanation — this is the file
/// somebody opens when a profile has gone missing from the switcher.
///
/// `selected` is the id of the profile a plain launch should reopen; `None`
/// leaves the directive out (a fresh list with no open profile yet).
pub fn body(records: &[ProfileRecord], selected: Option<ProfileId>) -> String {
    let mut out = String::from(
        "# The Koru profiles on this machine.\n\
         #   <id> | <name> | <email> | <last opened, UTC> | <folder>\n\
         #\n\
         # This is only an index. Every folder below also describes itself in its\n\
         # own profile.conf, so losing this file loses nothing - point Koru at the\n\
         # folder again and it comes back with the same id, name and email.\n",
    );
    if let Some(id) = selected {
        out.push_str(&format!(
            "# The `{SELECTED}` line names the profile a plain launch reopens.\n\
             {SELECTED} {}\n",
            id.to_storage()
        ));
    }
    for r in records {
        out.push_str(&format!(
            "{} {SEP} {} {SEP} {} {SEP} {} {SEP} {}\n",
            r.id.to_storage(),
            reserved(&r.name),
            reserved(r.email.as_deref().unwrap_or("")),
            reserved(r.last_opened.as_deref().unwrap_or("")),
            // Not sanitised: the folder is the last field and takes the rest of
            // the line, so it is the one place a separator is harmless — and it
            // is the one field we must never alter, because an altered path is
            // a profile that cannot be opened.
            r.dir.display(),
        ));
    }
    out
}

/// Keep a field out of the separator's way.
///
/// A `|` in any field but the last would shift every field after it, so the row
/// would name a different folder than the person who typed the name intended.
/// Names are cosmetic, so a space costs nothing; the folder is not, so it is
/// left exactly as it is.
fn reserved(field: &str) -> String {
    field.replace(SEP, " ")
}

/// Add `rec`, or update the entry that is already there.
///
/// **Keyed on the id, not the folder.** A profile folder copied to a second path
/// is a second *replica* of the same profile, not a second profile — so the two
/// paths collapse to one row, and the one that was opened most recently wins.
/// Keying on the path instead would put the same account in the switcher twice
/// with no way to tell which was which.
pub fn upsert(records: &mut Vec<ProfileRecord>, rec: ProfileRecord) {
    match records.iter_mut().find(|r| r.id == rec.id) {
        Some(existing) => *existing = rec,
        None => records.push(rec),
    }
}

pub fn find_by_id(records: &[ProfileRecord], id: ProfileId) -> Option<&ProfileRecord> {
    records.iter().find(|r| r.id == id)
}

/// Look a profile up by what somebody typed after `--profile`.
///
/// An exact case-insensitive name first, then a unique prefix. **Ambiguity is
/// `None`** — two profiles starting with "wo" means the caller refuses and says
/// so, because choosing one is the same failure as guessing a directory.
pub fn find_by_name<'a>(records: &'a [ProfileRecord], needle: &str) -> Option<&'a ProfileRecord> {
    let needle = needle.trim();
    if needle.is_empty() {
        return None;
    }
    let exact: Vec<&ProfileRecord> = records
        .iter()
        .filter(|r| r.name.eq_ignore_ascii_case(needle))
        .collect();
    if let [only] = exact[..] {
        return Some(only);
    }
    if !exact.is_empty() {
        return None; // two profiles with one name: say nothing rather than pick
    }
    let lower = needle.to_lowercase();
    let prefixed: Vec<&ProfileRecord> = records
        .iter()
        .filter(|r| r.name.to_lowercase().starts_with(&lower))
        .collect();
    match prefixed[..] {
        [only] => Some(only),
        _ => None,
    }
}

/// The profile that was open last, which is what a plain launch reopens.
pub fn most_recent(records: &[ProfileRecord]) -> Option<&ProfileRecord> {
    records.iter().max_by(|a, b| {
        a.last_opened
            .cmp(&b.last_opened)
            .then_with(|| a.name.cmp(&b.name))
    })
}

/// Most recently opened first — the order the picker and the switcher show.
pub fn by_recency(records: &[ProfileRecord]) -> Vec<&ProfileRecord> {
    let mut out: Vec<&ProfileRecord> = records.iter().collect();
    out.sort_by(|a, b| {
        b.last_opened
            .cmp(&a.last_opened)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[cfg(test)]
mod tests;
