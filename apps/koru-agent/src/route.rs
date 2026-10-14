// SPDX-License-Identifier: MPL-2.0

//! Where a chosen search result goes, and what happens when nothing is holding
//! the workspace it lives in.
//!
//! ```text
//! Open(target) ──> new_window?
//!    yes -> a shell already on (profile, workspace)?
//!             yes -> deliver (shell opens a second pane on the same doc)
//!             no  -> launch a new process for that pair
//!    no  -> a shell already on (profile, workspace)?
//!             yes -> deliver there
//!             no  -> a shell of that profile on *any* workspace?
//!                     yes -> deliver there (it switches workspace in place)
//!                     no  -> launch
//!    ...on the next Hello matching (profile, workspace): deliver and clear
//!    ...on the deadline: drop it, and say the launch never arrived
//! ```
//!
//! ## The target never goes on the command line
//!
//! It carries `match_text`, which is a fragment of what somebody wrote — and a
//! command line is readable by every process on Windows and by anything that
//! can read `/proc` on Linux. The gateway bootstrap already refuses to put a
//! *token* on one for the same reason; this is that rule applied to content.
//!
//! So a shell is spawned with only profile/workspace ids, and asks for the
//! private target that is waiting when it says `Hello`.
//!
//! ## Replace, never queue
//!
//! Two results chosen before the window opens means the person changed their
//! mind, not that they want two windows. A queue would open both, seconds
//! apart, in an order nobody asked for.
//!
//! ## Why there is a deadline
//!
//! Without one a pending target waits forever, and the next time that workspace
//! is opened — twenty minutes later, for an unrelated reason — a result nobody
//! remembers choosing appears. The deadline is generous, because a cold launch
//! of Koru is not fast, and it is not a retry: when it expires the target is
//! dropped and said to be dropped.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use koru_agent_wire::Target;
use koru_store_core::ids::{ProfileId, WorkspaceId};

use crate::registry::{ConnectionId, Registry};

/// How long a spawned shell has to introduce itself before its target is given
/// up on. A cold launch opens a store connection, a manifest and every
/// remembered tab, so this is minutes-generous rather than seconds-tight.
pub const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);

/// What the caller should do about a target. **Returned rather than performed**,
/// so the decision is testable without a process, a socket or a window — which
/// is the whole reason this module has no side effects.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Routed {
    /// Push it down this connection, and bring that window forward if the
    /// platform told us its pid.
    Deliver {
        connection: ConnectionId,
        pid: Option<u32>,
        target: Box<Target>,
    },
    /// Nothing is holding that workspace. Start a shell named only by the
    /// profile/workspace ids and expect a `Hello`.
    Launch {
        profile: ProfileId,
        workspace: WorkspaceId,
    },
    /// A launch for that workspace is already on its way, and now carries this
    /// target instead of the one it had.
    AlreadyLaunching,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Pending {
    target: Target,
    deadline: Instant,
}

#[derive(Default)]
pub struct Router {
    pending: HashMap<(ProfileId, WorkspaceId), Pending>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide what to do with a target.
    ///
    /// `now` is passed in rather than read, so a test can drive the deadline
    /// without sleeping — the same rule `SessionCredential::has_expired`
    /// follows.
    ///
    /// **Default reuses a window.** Ordinary open prefers a shell already on
    /// the target workspace, then the profile's most recently active window
    /// (which switches workspace in place). `new_window` never steals another
    /// workspace's window: only an exact match is reused, and only so that
    /// shell can open a second pane on the same live document.
    pub fn open(
        &mut self,
        registry: &Registry,
        profile: ProfileId,
        workspace: WorkspaceId,
        target: Target,
        now: Instant,
    ) -> Routed {
        let shell = if target.new_window {
            registry.route(profile, workspace)
        } else {
            registry
                .route(profile, workspace)
                .or_else(|| registry.route_profile(profile))
        };
        if let Some(shell) = shell {
            return Routed::Deliver {
                connection: shell.connection,
                pid: shell.pid,
                target: Box::new(target),
            };
        }
        // An expired entry is not a launch in flight; it is a launch that never
        // arrived, and the person is asking again.
        let live = self
            .pending
            .get(&(profile, workspace))
            .is_some_and(|pending| now < pending.deadline);
        self.pending.insert(
            (profile, workspace),
            Pending {
                target,
                deadline: now + LAUNCH_DEADLINE,
            },
        );
        if live {
            Routed::AlreadyLaunching
        } else {
            Routed::Launch { profile, workspace }
        }
    }

    /// A shell has introduced itself. Hand over whatever was waiting for it.
    ///
    /// Clears on delivery, so a second window opened on the same workspace an
    /// hour later does not inherit somebody's old search result.
    pub fn claim(
        &mut self,
        profile: ProfileId,
        workspace: WorkspaceId,
        now: Instant,
    ) -> Option<Target> {
        let pending = self.pending.remove(&(profile, workspace))?;
        (now < pending.deadline).then_some(pending.target)
    }

    /// Drop everything nobody came for. The caller says so, because a dropped
    /// target is news: somebody chose a result and it did not open.
    pub fn expire(&mut self, now: Instant) -> Vec<(ProfileId, WorkspaceId)> {
        let dead: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| now >= pending.deadline)
            .map(|(key, _)| *key)
            .collect();
        for key in &dead {
            self.pending.remove(key);
        }
        dead
    }

    /// A spawn that failed. The target goes rather than waiting for a `Hello`
    /// that cannot come.
    pub fn launch_failed(&mut self, profile: ProfileId, workspace: WorkspaceId) {
        self.pending.remove(&(profile, workspace));
    }

    /// When the earliest pending target gives up, so the loop can sleep until
    /// exactly then rather than on a clock of its own.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|pending| pending.deadline).min()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn has_pending(&self, profile: ProfileId, workspace: WorkspaceId) -> bool {
        self.pending.contains_key(&(profile, workspace))
    }
}

#[cfg(test)]
mod tests;
