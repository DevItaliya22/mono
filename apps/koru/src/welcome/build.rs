// SPDX-License-Identifier: MPL-2.0

//! What this build is: its stamp and its channel's copy.
//!
//! The Welcome page used to draw these beside the brand; the page no longer
//! does, and the update surface (`mainview/update_ui.rs`) is now their only
//! reader. They stay in this module so no `crate::welcome::…` path changed.

use chrono::{DateTime, Local};
use std::sync::OnceLock;
use std::time::SystemTime;

/// Where *Download another channel* goes.
///
/// **This page does not exist yet, and shipping it anyway is a decision rather
/// than an oversight.** The website, which is not in this repository, has no
/// download route yet; the route is owed and lands before the release this
/// button is meant for. Until then the link
/// 404s, which is a worse first impression than no link but a better one than a
/// channel control that cannot change the channel — the thing it replaced.
///
/// The consequence to watch: this is the *only* way a person can move between
/// channels now, so the page is not a nicety. See `docs/deferred/`.
pub(crate) const DOWNLOAD_URL: &str = "https://koru.do/download";

/// How a stamp writes a moment: `2026-08-08 14:32`, in the reader's own time
/// zone.
const STAMP: &str = "%Y-%m-%d %H:%M";

/// When this build was released, or failing that when it was built — with the
/// word that says which, since they are not the same claim.
///
/// The release stamp is compiled in by `koru-release`. Nothing stamps a local
/// build, and a compile-time constant would be the wrong fix: a build script
/// reruns only when its crate's inputs change, so the value would freeze. The
/// executable's own timestamp is exactly what a dev build means by "built".
/// Resolved once per process.
pub(crate) fn build_stamp() -> Option<&'static str> {
    static STAMPED: OnceLock<Option<String>> = OnceLock::new();
    STAMPED
        .get_or_init(|| stamp(koru_channel::build_info().released(), exe_modified()))
        .as_deref()
}

/// The two sources, separated from where they come from so both arms are
/// reachable in a test.
pub(super) fn stamp(released: Option<&str>, built: Option<SystemTime>) -> Option<String> {
    if let Some(released) = released {
        // A stamp that will not parse is still the truth about this build, so
        // it is shown as written rather than dropped.
        let shown = DateTime::parse_from_rfc3339(released).map_or_else(
            |_| released.to_string(),
            |at| at.with_timezone(&Local).format(STAMP).to_string(),
        );
        return Some(format!("released {shown}"));
    }
    Some(format!(
        "built {}",
        DateTime::<Local>::from(built?).format(STAMP)
    ))
}

/// `nightly` → `Nightly`.
///
/// The report carries the **storage spelling**; capitalising here rather than
/// mapping to an enum keeps a channel this build has never heard of readable
/// instead of silently becoming Stable.
pub(crate) fn channel_label(token: &str) -> String {
    let mut chars = token.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The one line of caution a channel is owed, if any.
pub(crate) fn channel_note(token: &str) -> Option<&'static str> {
    match token {
        "nightly" => Some("Nightly builds may be unstable and update frequently."),
        "internal" => Some("Internal builds are for developers only and are not published."),
        _ => None,
    }
}

fn exe_modified() -> Option<SystemTime> {
    std::env::current_exe()
        .ok()?
        .metadata()
        .ok()?
        .modified()
        .ok()
}
