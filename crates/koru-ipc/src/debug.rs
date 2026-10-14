// SPDX-License-Identifier: MPL-2.0

//! What happened on the endpoint, on demand.
//!
//! ```sh
//! KORU_IPC_DEBUG=1 ./koru.sh run          # the client's half
//! KORU_IPC_DEBUG=1 ./koru.sh run-all      # both halves: the Agent hosts the gateway
//! ```
//!
//! Off, this is a load of a `bool` and one branch. On, it prints a line per
//! endpoint derived, connection attempted, name bound, peer accepted and frame
//! crossed — on **both** sides, because the two halves of "it did not connect"
//! live in different processes and the whole difficulty of this layer is that
//! you can only ever see one of them at a time.
//!
//! ## Why a flag rather than always
//!
//! `guide/contributing/PRINCIPLES.md`, *report news, not activity*: a line that appears
//! whether or not anything happened is one people learn to scroll past. Every
//! line here is per-event and routine — a connection is not news — so it goes
//! behind the flag, where generosity costs nothing.
//!
//! The consequential outcomes are **not** logged here at all. They are already
//! [`crate::IpcError`], typed and carrying the endpoint's name, and the caller
//! reports them through whatever it has: the store has `tracing` and a log
//! file, the app has a dialog, and a library that wrote to stderr on its own
//! account would be talking over both. A stale-socket takeover is deliberately
//! quiet for the same reason the workspace store's recovery is a *value* rather
//! than a log line.
//!
//! ## What a line may contain, and what it may never
//!
//! Endpoint names, service tokens, channels, error kinds, pids, frame versions
//! and **byte counts**. Never a frame's body — an agent's frame carries
//! `match_text`, which is a fragment of what somebody wrote, and a log somebody
//! attaches to a report must not be a record of what they typed. That is the
//! same rule `KORU_INPUT_DEBUG` follows for keystrokes, and it is a rule about
//! *construction*: the body is not available at any call site below.

use std::fmt;
use std::sync::OnceLock;

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_IPC_DEBUG))
}

/// Print one line, prefixed so it can be grepped out of a mixed log.
///
/// The pid is on every line because this layer is *between* processes: a
/// transcript with two of them in it is the normal case, and without a pid the
/// two halves of a failed connection are indistinguishable.
pub fn write(args: fmt::Arguments<'_>) {
    if enabled() {
        eprintln!("[Koru][ipc][pid={}] {args}", std::process::id());
    }
}

/// Write a line only when the switch is on, evaluating nothing otherwise.
///
/// A macro rather than a function so that formatting a pid, an endpoint name
/// and an error kind costs nothing at all when the flag is off — `write` alone
/// would still build the `Arguments`.
#[macro_export]
macro_rules! ipc_debug {
    ($($arg:tt)*) => {
        if $crate::debug::enabled() {
            $crate::debug::write(format_args!($($arg)*))
        }
    };
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_variable_name_is_ipc_specific() {
        // Its own switch rather than a share of `KORU_SEARCH_DEBUG`: this layer
        // is underneath every product in the tree, and somebody debugging a
        // store that will not start is not debugging search.
        assert_eq!(koru_env::name::KORU_IPC_DEBUG, "KORU_IPC_DEBUG");
    }

    #[test]
    fn a_line_carries_the_pid_that_wrote_it() {
        // Asserted on the *format* rather than on captured output, because
        // stderr is not capturable here and the property that matters is that
        // the pid is in the template at all — a transcript of two processes is
        // this layer's normal case.
        assert!(
            format!("[Koru][ipc][pid={}] x", std::process::id()).contains("pid="),
            "the prefix must name the process"
        );
    }
}
