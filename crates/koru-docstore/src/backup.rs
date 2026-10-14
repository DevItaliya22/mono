// SPDX-License-Identifier: MPL-2.0

//! Copies of the snapshot, kept so a bad build is survivable.
//!
//! There is exactly one `snapshot.loro` and every save replaces it. That is
//! fine while the only thing writing it is the build that wrote it last, and it
//! stops being fine the moment a released build and a build from this tree take
//! turns — a schema change, a bumped `loro`, a mistake, and the one copy is the
//! rewritten one.
//!
//! So a copy is taken **before the first save of each session**: the state as
//! it was when this build opened your content, which is exactly what you want
//! back when that build turns out to be wrong. After that one an hour is plenty
//! — saves fire every 800 ms of idle, and twenty of those a minute would be a
//! disk-filling machine rather than a safety net.
//!
//! Names are UTC rather than local time, so the set sorts chronologically as
//! plain text and never gains a duplicate or a gap at a daylight-saving change.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long after a session's first backup before another is taken.
pub const BACKUP_EVERY: Duration = Duration::from_secs(60 * 60);

/// How many to keep. Twenty at today's ~1.4 MB snapshot is under 30 MB, which
/// is a price worth paying without asking.
pub const BACKUPS_KEPT: usize = 20;

const PREFIX: &str = "snapshot-";
const SUFFIX: &str = ".loro";

/// `YYYYMMDD-HHMMSS`, UTC, from a wall clock. Used for backup names and for the
/// quarantine directory, so the two line up when reading a directory listing.
pub fn stamp(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        // A clock before 1970 is not worth a code path; it just sorts first.
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

pub fn stamp_now() -> String {
    stamp(SystemTime::now())
}

/// Days since the epoch to a civil date. Hinnant's algorithm — the standard
/// branch-free one, valid for any date this program will ever see, and cheaper
/// than a date dependency for the one thing we need it for.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Every backup in `dir`, **newest first** — the order recovery wants.
///
/// Sorted by name rather than by mtime: the name is what the writer controlled,
/// and a file copied off a USB stick brings someone else's mtime with it.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX))
        })
        .collect();
    found.sort_unstable();
    found.reverse();
    found
}

/// Copy `live` into `dir` under a timestamped name, then prune to
/// [`BACKUPS_KEPT`].
///
/// Via a temp file and a rename, for the same reason the snapshot itself is: a
/// half-written backup that looks like a whole one is worse than no backup.
///
/// `tag` names the instance doing the copying. The stamp only resolves to the
/// second, so two windows opening a profile together would otherwise pick the
/// same temp name and interleave into one file — and a corrupt *backup* is the
/// worst possible file to corrupt, because nothing looks at it until the day
/// everything else has already failed.
pub fn take(dir: &Path, live: &Path, tag: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let dest = dir.join(format!("{PREFIX}{}{SUFFIX}", stamp_now()));
    let tmp = dest.with_extension(format!("{tag}.part"));
    fs::copy(live, &tmp)?;
    fs::rename(&tmp, &dest)?;
    prune(dir, BACKUPS_KEPT);
    Ok(dest)
}

/// Keep the newest `keep` and delete the rest. Best-effort: a file that will
/// not delete is left alone rather than failing the save that triggered this.
pub fn prune(dir: &Path, keep: usize) {
    for old in list(dir).into_iter().skip(keep) {
        let _ = fs::remove_file(old);
    }
}

/// Copy a whole directory tree. What `--backup` runs, and how the quarantine
/// keeps an unreadable file: a **copy**, so nothing on the failing path can
/// itself destroy the thing it was called to preserve.
///
/// **`trash/` is skipped.** `--backup` copies the whole profile directory, and
/// the trash holds entire deleted workspaces — so without this, deleting a
/// large workspace would silently double the size of every backup taken
/// afterwards, and go on doing so for a month. A deleted workspace is not
/// something a backup is *for*; it is already a copy of something you asked to
/// be rid of.
///
/// Returns how many files were copied.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    fs::create_dir_all(to)?;
    let mut n = 0;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_name() == crate::trash::TRASH_DIR {
            continue;
        }
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            n += copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn the_stamp_is_a_real_date() {
        assert_eq!(stamp(at(0)), "19700101-000000");
        // 2026-07-31 12:51:18 UTC
        assert_eq!(stamp(at(1_785_502_278)), "20260731-125118");
        // A leap day, which is where a hand-rolled calendar goes wrong.
        assert_eq!(stamp(at(1_709_164_800)), "20240229-000000");
        // ...and the day after, in case the leap ran one day long.
        assert_eq!(stamp(at(1_709_251_200)), "20240301-000000");
    }

    /// The name is what `list` sorts on, so a later moment must produce a
    /// lexicographically greater name — for every field, not just the seconds.
    #[test]
    fn later_stamps_sort_after_earlier_ones() {
        let mut last = stamp(at(0));
        for secs in [
            59,
            60,
            3_599,
            3_600,
            86_399,
            86_400,
            1_000_000_000,
            1_785_502_278,
        ] {
            let now = stamp(at(secs));
            assert!(now > last, "{now} should sort after {last}");
            last = now;
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("koru-backup-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backups_are_listed_newest_first_and_pruned_to_a_ceiling() {
        let dir = temp("prune");
        for n in 0..25 {
            fs::write(
                dir.join(format!("{PREFIX}20260101-0000{n:02}{SUFFIX}")),
                "x",
            )
            .unwrap();
        }
        // Not a backup, and must survive the pruning untouched.
        fs::write(dir.join("notes.txt"), "x").unwrap();

        let all = list(&dir);
        assert_eq!(all.len(), 25, "only the snapshot-*.loro files");
        assert!(all[0] > all[1], "newest first");

        prune(&dir, 20);
        let kept = list(&dir);
        assert_eq!(kept.len(), 20);
        assert_eq!(kept[0], all[0], "the newest is the one kept");
        assert!(dir.join("notes.txt").exists(), "pruning stays in its lane");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn taking_a_backup_copies_the_live_file() {
        let dir = temp("take");
        let live = dir.join("snapshot.loro");
        fs::write(&live, b"the document").unwrap();

        let made = take(&dir.join("backups"), &live, "A").unwrap();
        assert_eq!(fs::read(&made).unwrap(), b"the document");
        assert!(live.exists(), "the original is copied, not moved");
        assert!(
            !made.with_extension("A.part").exists(),
            "no temp file left behind"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The stamp only resolves to the second, so two instances backing up at
    /// once would otherwise interleave into one temp file — and a corrupt
    /// *backup* is the worst file to corrupt, since nothing reads it until the
    /// day everything else has already failed.
    #[test]
    fn a_backup_temp_file_is_named_per_instance() {
        let dir = temp("taketag");
        let live = dir.join("snapshot.loro");
        fs::write(&live, b"the document").unwrap();
        let backups = dir.join("backups");

        let made = take(&backups, &live, "i3f9c").unwrap();
        assert_ne!(
            made.with_extension("i3f9c.part"),
            made.with_extension("i9c11.part"),
            "two instances cannot pick the same temp name"
        );
        // And whatever the tag was, the file that lands is the plain one, so
        // `list` still finds it.
        assert!(list(&backups).contains(&made));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tree_copy_reaches_the_nested_files() {
        let dir = temp("tree");
        let from = dir.join("data");
        fs::create_dir_all(from.join("updates")).unwrap();
        fs::write(from.join("snapshot.loro"), "s").unwrap();
        fs::write(from.join("updates/peer-default.loro"), "u").unwrap();

        let to = dir.join("copy");
        assert_eq!(copy_tree(&from, &to).unwrap(), 2);
        assert_eq!(fs::read_to_string(to.join("snapshot.loro")).unwrap(), "s");
        assert_eq!(
            fs::read_to_string(to.join("updates/peer-default.loro")).unwrap(),
            "u"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
