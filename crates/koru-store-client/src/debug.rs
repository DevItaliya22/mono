// SPDX-License-Identifier: MPL-2.0

//! What the sync conversation did, on demand.
//!
//! ```sh
//! KORU_SYNC_DEBUG=1 ./koru.sh run          # the client's half
//! KORU_SYNC_DEBUG=1 ./koru.sh run-all      # both halves: the Agent hosts the gateway
//! ```
//!
//! Off, this is a load of a `bool` and one branch. On, it prints a line per
//! update sent, batch acknowledged, peer state change and announcement
//! debounce — on **both** sides, because the two halves of "my edit did not
//! arrive" live in different processes.
//!
//! ## Why a flag rather than always
//!
//! `guide/contributing/PRINCIPLES.md`, *report news, not activity*. Every line here is
//! per-event and routine: a keystroke is an update, an update is a batch, and a
//! batch is two lines. Typing a sentence with the console open produced pages
//! of transcript that said only that sync was working, which is the definition
//! of a line people learn to scroll past — and it buried the lines that were
//! news.
//!
//! **`--console` is not the switch.** It used to be, for the agent's half, and
//! that conflated two questions: *where does output go* and *how much of it is
//! there*. A developer running the stack in a terminal to watch one thing was
//! given everything.
//!
//! The consequential outcomes are not logged here. A refused write, a store
//! that will not answer and a peer that will not authorise are errors with
//! types, reported by whoever can act on them.
//!
//! ## What a line may contain, and what it may never
//!
//! Room ids, batch ids, device ids, workspace ids, stages, byte counts and
//! error codes. **Never a document's content** — not a title, not a text run,
//! not a `match_text`. A transcript somebody attaches to a bug report must not
//! be a record of what they wrote, which is the same rule `KORU_IPC_DEBUG` and
//! `KORU_INPUT_DEBUG` follow.

use std::fmt;
use std::sync::OnceLock;

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_SYNC_DEBUG))
}

/// Print one line, prefixed so it can be grepped out of a mixed log.
pub fn write(args: fmt::Arguments<'_>) {
    if enabled() {
        eprintln!("[Koru][sync] {args}");
    }
}

/// Whether to report a Store credential being renewed. These are rare lifecycle
/// events rather than sync traffic, so they have their own switch and cannot
/// bury an edit transcript.
pub fn session_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_STORE_SESSION_DEBUG))
}

/// Print a credential lifecycle event without ever including a bearer token.
pub fn session_write(args: fmt::Arguments<'_>) {
    if session_enabled() {
        eprintln!("[Koru][store-session] {args}");
    }
}

/// Write a line only when the switch is on, evaluating nothing otherwise.
///
/// A macro rather than a function so that formatting a room id, a batch id and
/// a wall-clock stamp costs nothing at all when the flag is off — `write` alone
/// would still build the `Arguments`, and one of these sits on the path every
/// keystroke takes.
#[macro_export]
macro_rules! sync_debug {
    ($($arg:tt)*) => {
        if $crate::debug::enabled() {
            $crate::debug::write(format_args!($($arg)*))
        }
    };
}

/// Write a Store credential lifecycle event only when its switch is on.
#[macro_export]
macro_rules! store_session_debug {
    ($($arg:tt)*) => {
        if $crate::debug::session_enabled() {
            $crate::debug::session_write(format_args!($($arg)*))
        }
    };
}

#[cfg(test)]
mod tests {
    /// Its own switch rather than a share of `KORU_IPC_DEBUG`: the endpoint
    /// layer answers "did they connect", and this answers "did the edit get
    /// there". Somebody debugging one is rarely debugging the other, and a
    /// single switch would drown whichever they wanted.
    #[test]
    fn the_variable_name_is_sync_specific() {
        assert_eq!(koru_env::name::KORU_SYNC_DEBUG, "KORU_SYNC_DEBUG");
    }

    #[test]
    fn the_session_variable_names_credential_lifecycle() {
        assert_eq!(
            koru_env::name::KORU_STORE_SESSION_DEBUG,
            "KORU_STORE_SESSION_DEBUG"
        );
    }
}
