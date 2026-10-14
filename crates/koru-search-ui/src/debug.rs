// SPDX-License-Identifier: MPL-2.0

//! Why a search result did or did not open, on demand.
//!
//! ```sh
//! KORU_SEARCH_DEBUG=1 ./koru.sh run
//! ```
//!
//! Off, this is a load of a `bool` and one branch. On, it prints one line per
//! routing decision: which hit was selected, which workspace and document it
//! resolved to, which were refused and why, and what the host was asked to do
//! about it.
//!
//! **A name, not an activity.** Each line says what was *decided* — accepted,
//! rejected, and the reason — because the two bugs this switch was built for
//! were both "the result did nothing", and a log that said only "a selection
//! happened" would have shown the same output in the working case. See
//! `guide/contributing/PRINCIPLES.md`, *report news, not activity*.
//!
//! Output goes to stderr, so a debug build shows it in the console it keeps. A
//! release build has no console (`windows_subsystem = "windows"`), so run a
//! debug build — which `./koru.sh run` already does.
//!
//! ## Why it is here rather than in `koru-workspace-host`
//!
//! It was `koru_workspace_host::searchdebug`, and reaching it meant linking a
//! workspace. What it describes is this crate's own decisions, so it moved with
//! them ([ADR-105](../../../docs/decisions/ADR-101-125.md#adr-105-ipc--mechanism-is-pushed-down-and-domain-behaviour-stays-with-its-domain)).
//! The workspace host keeps its own adapter-side logger for the *other* half of
//! the same journey — what it did with the target once it had one. There is no
//! facade between the two owners.

use std::fmt;
use std::sync::OnceLock;

/// One rule with every other switch now — `koru_env::switch` — which is the
/// alignment TECHDEBT 29 asked for: `KORU_SEARCH_DEBUG=0` finally means off,
/// in this copy and in `koru-workspace-host`'s, because both ask the same function.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_SEARCH_DEBUG))
}

pub fn write(args: fmt::Arguments<'_>) {
    if enabled() {
        eprintln!("{args}");
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_debug_variable_name_is_search_specific() {
        // Pinned because `koru-workspace-host` reads the same name from its own copy
        // until stage 5b, and a rename on one side would silently halve the
        // log. Both copies now read through the same `koru_env` constant, so
        // what is pinned is the constant's value.
        assert_eq!(koru_env::name::KORU_SEARCH_DEBUG, "KORU_SEARCH_DEBUG");
    }
}
