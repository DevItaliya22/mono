// SPDX-License-Identifier: MPL-2.0

//! **What is left of `desktop-v1.json`, from this side: one read.**
//!
//! It held four facts about this desktop — what closing does, which key opens
//! search from anywhere, how tall the overlay is, and whether the app starts at
//! sign-in — and every window that opened it rewrote the whole body. Three of
//! the four are registry rows now (ADR-266): the settings authority reads and
//! writes them, and nothing here may serialize a body over the top of that.
//! The overlay height was never a preference at all — it is geometry, and it
//! was in that file by adjacency — so it went to the `global-search` row in
//! `windows.conf`, which the Agent has always owned. See
//! [`crate::search_geometry`].
//!
//! **What remains is a read.** The Agent asks this file which chord raises
//! global search, before any Gateway session exists — a hotkey has to be
//! registered at start-up, and a window that has not connected yet cannot be
//! told what it is. The value is still the authority's to write; this is a
//! projection of it, the way the appearance is.
//!
//! A global shortcut and a Run-key entry are platform traps, which is what this
//! crate is for ([ADR-105](../../../docs/decisions/README.md)).

use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub const GLOBAL_SEARCH_DEFAULT_HEIGHT: u32 = 400;
pub const GLOBAL_SEARCH_MIN_HEIGHT: u32 = 400;
pub const GLOBAL_SEARCH_MAX_HEIGHT: u32 = 1_600;

/// The two startup facts a reminder surface receives from the settings
/// authority without acquiring write access to its machine-local file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReminderStartupFacts {
    pub start_on_login: bool,
    pub explained: bool,
}

/// What ✕ on the last window means.
///
/// **`Background` is the default** — with a live agent, closing the last window
/// keeps the process resident behind the agent's tray icon, so the next launch
/// is a reveal rather than a cold start (store rejoin, tab restore, a full
/// shader compile). The safety rule is unchanged from ADR-41: hiding is refused
/// without an agent connection, so on an agent-less machine — and on Linux,
/// where there is no tray — `Background` closes normally. `Ask` remains for
/// whoever chose it back when it was the default; `Quit` is the Settings row's
/// opt-out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloseBehavior {
    Ask,
    #[default]
    Background,
    Quit,
}

/// **A read-only projection of `desktop-v1.json`. There is no writer here.**
///
/// The settings authority owns every key this file holds. A whole-body write
/// from this side would revert whatever it last committed — precisely the
/// two-writers clobber ADR-266 exists to remove — so the ability to write is
/// gone rather than merely unused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopPreferences {
    /// The chord that raises global search. Read by the Agent at start-up,
    /// before it has a Gateway session to ask.
    pub global_shortcut: String,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            global_shortcut: "Ctrl+Shift+K".into(),
        }
    }
}

impl DesktopPreferences {
    pub fn path() -> Option<PathBuf> {
        ProjectDirs::from("do", "Koru", "Koru")
            .map(|dirs| dirs.config_local_dir().join("desktop-v1.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_keep_safe_defaults() {
        let value: DesktopPreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(value.global_shortcut, "Ctrl+Shift+K");
    }

    /// **A body full of the authority's keys still reads.** Every other key in
    /// that file belongs to the settings authority now, and this projection has
    /// to ignore them rather than refuse — a chord nobody can read is a hotkey
    /// that does not register.
    #[test]
    fn the_authoritys_own_keys_are_read_past() {
        let value: DesktopPreferences = serde_json::from_str(
            r#"{"close_behavior":"quit","global_shortcut":"Ctrl+Alt+J",
                "start_on_login":true,"automatic_checks":false}"#,
        )
        .unwrap();
        assert_eq!(value.global_shortcut, "Ctrl+Alt+J");
    }

    /// **There is no writer here.** Every key in the file is the settings
    /// authority's, and a whole-body write from this side would revert whatever
    /// it last committed — the two-writers clobber ADR-266 removes.
    #[test]
    fn this_projection_cannot_write() {
        let source = include_str!("desktop_preferences.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the module's production half");
        assert!(!source.contains("fs::write"));
        assert!(!source.contains("fn save"));
    }
}
