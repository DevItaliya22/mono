// SPDX-License-Identifier: MPL-2.0

//! How much room a Koru write may count on, on the volume holding a path.
//!
//! **One question, asked the operating system's way.** Two callers ask it —
//! whether a Cloud download and its published copy fit (`koru-blob-sync`), and
//! whether an update can be staged (`koru-updater`) — and both used `fs2`,
//! which is `statvfs`'s `f_bavail` off Windows.
//!
//! **On Apple that number is too small.** APFS keeps purgeable space — Time
//! Machine local snapshots, optimised-storage files, caches — which the system
//! frees on demand for a write that matters, and `f_bavail` leaves all of it
//! out. On a nearly full Mac, and much more on an iPhone, Koru refused a
//! download the system would have made room for, and told the person the
//! device could not fit it. Foundation's
//! `NSURLVolumeAvailableCapacityForImportantUsageKey` is the number Apple
//! documents for exactly this ("a write the user asked for"), so on Apple that
//! is the answer, and `fs2` is the fallback if Foundation cannot give one.
//! Everywhere else `fs2` is already right: Windows' `GetDiskFreeSpaceExW`
//! answers for the caller, and Linux has no purgeable space.

use std::{io, path::Path};

/// Bytes a write the person asked for may use on the volume holding `path`.
/// `path` must exist.
pub fn available(path: &Path) -> io::Result<u64> {
    #[cfg(target_vendor = "apple")]
    if let Some(bytes) = apple::important_usage(path) {
        return Ok(bytes);
    }
    fs2::available_space(path)
}

#[cfg(target_vendor = "apple")]
mod apple {
    use std::path::Path;

    use objc2_foundation::{
        NSArray, NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
    };

    /// `None` when Foundation has no answer — a path that is not UTF-8, a
    /// volume that does not report the key — so the caller falls back to
    /// `statvfs` rather than refusing on a missing number.
    pub(super) fn important_usage(path: &Path) -> Option<u64> {
        let path = NSString::from_str(path.to_str()?);
        let url = NSURL::fileURLWithPath(&path);
        // SAFETY: a static NSString Foundation exports, read once.
        let key = unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey };
        let values = url
            .resourceValuesForKeys_error(&NSArray::from_slice(&[key]))
            .ok()?;
        let number = values.objectForKey(key)?.downcast::<NSNumber>().ok()?;
        u64::try_from(number.longLongValue()).ok()
    }
}

#[cfg(test)]
mod tests;
