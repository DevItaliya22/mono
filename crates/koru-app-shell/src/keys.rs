// SPDX-License-Identifier: MPL-2.0

//! Opaque keys. Identity crosses the boundary as a uuid or a shell-minted
//! ordinal; the shell never learns what one means.
//!
//! `ProfileKey` and `WorkspaceKey` follow the `koru-link` precedent ADR-262
//! names: a newtype over `uuid::Uuid`, built from and turned back into one by
//! the host, so there is no shared id crate and no type parameter on any shell
//! type.
//!
//! ## `WindowKey` is the runner's, not a second one
//!
//! SP-0's boundary minted its own `WindowKey`. SP-1 then shipped one in
//! `koru-window`, where [`MultiWindowApp::windows`] is *the single authority*
//! over which windows exist — so a shell key of its own would need a map on
//! every event and two sources of truth about the same window. The shipped
//! boundary re-exports [`koru_window::WindowKey`] and mints values of it; the
//! rule SP-0's key existed to enforce ("every command names a target window") is
//! kept by [`Commands`](crate::Commands) being stamped with one, not by owning
//! the type.
//!
//! [`MultiWindowApp::windows`]: koru_window::MultiWindowApp::windows

use uuid::Uuid;

/// One chromed window, named by the application and handed to the runner.
///
/// Re-exported rather than redefined: `run_windows` opens and drops windows to
/// match [`MultiWindowApp::windows`](koru_window::MultiWindowApp::windows), so
/// this is the only name for a window that can be authoritative.
pub use koru_window::WindowKey;

macro_rules! uuid_key {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(Uuid);

        impl $name {
            pub fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }
    };
}

uuid_key!(
    ProfileKey,
    "A profile, as the shell sees it: sixteen bytes and nothing else."
);
uuid_key!(
    WorkspaceKey,
    "A workspace, as the shell sees it: sixteen bytes and nothing else."
);

/// One tab inside a window, for hosts that use the shell's tab manager.
///
/// An ordinal would be wrong: reordering a strip renumbers every tab, and a
/// command recorded during a press would then land on a different one by the
/// time it is drained. This is stable for the tab's life.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TabKey(u64);

impl TabKey {
    /// A key with a chosen number, for a test that has to name one without a
    /// window. **Never in the product**: only [`Minter`] hands out live keys,
    /// and one minted here would collide with them.
    #[doc(hidden)]
    pub const fn for_test(value: u64) -> Self {
        Self(value)
    }

    /// The key a tab carries between the strip minting it and the shell seating
    /// it — a moment long, inside one method, and never observable.
    ///
    /// Zero, which [`Minter`] never hands out because it increments first. See
    /// [`ShellTab`](crate::ShellTab) for why the two halves of a floor tab are
    /// minted apart.
    pub(crate) const UNSEATED: Self = Self(0);
}

/// Mints windows and tabs from one counter, so no key of either kind is ever
/// reused inside a process.
///
/// Not `pub`: only [`ShellState`](crate::ShellState) hands these out.
#[derive(Default, Debug)]
pub(crate) struct Minter(u64);

impl Minter {
    pub(crate) fn window(&mut self) -> WindowKey {
        self.0 += 1;
        WindowKey(self.0)
    }

    pub(crate) fn tab(&mut self) -> TabKey {
        self.0 += 1;
        TabKey(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One counter for both, so a window key and a tab key minted in the same
    /// process never share a number — which is what makes a stale command
    /// addressed to a closed window fail to find one rather than find a tab's.
    #[test]
    fn windows_and_tabs_are_minted_from_one_counter() {
        let mut minter = Minter::default();
        let first = minter.window();
        let tab = minter.tab();
        let second = minter.window();
        assert_ne!(first, second);
        assert_eq!(first.0 + 1, tab.0);
        assert_eq!(tab.0 + 1, second.0);
    }

    /// The host puts a uuid in and gets the same one back; the shell learns
    /// nothing in between.
    #[test]
    fn an_identity_crosses_the_boundary_unchanged() {
        let id = Uuid::new_v4();
        assert_eq!(ProfileKey::from_uuid(id).as_uuid(), id);
        assert_eq!(WorkspaceKey::from_uuid(id).as_uuid(), id);
    }
}
