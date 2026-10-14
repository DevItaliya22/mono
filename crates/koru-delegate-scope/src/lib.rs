// SPDX-License-Identifier: MPL-2.0

//! Scope admission: whether a working folder is free, and what to do if it
//! is not.
//!
//! Two delegated agents editing one checkout at once is the failure this
//! crate exists to stop from happening *silently*. It knows whether two
//! working scopes are the same folder or nest, and which run holds one; it
//! has no opinion about what a run does there, starts nothing and stores
//! nothing itself. P1.6's start path calls it before every real start.
//!
//! Four parts:
//!
//! - [`ScopeIdentity`] is what a [`WorkspaceScope`] *is* on this machine's
//!   filesystem: canonicalized, junction- and symlink-resolved, case-folded
//!   where the filesystem is, and compared structurally — never as the
//!   string a person spelled. Two spellings of one folder are one identity;
//!   a folder and its parent [`Overlap`]; siblings do not.
//! - [`ScopeClaims`] is the in-memory registry of who holds which scope, and
//!   [`ScopeClaims::admit`] gives D34's two answers: a **manual** start onto
//!   a busy scope is admitted with a [`BusyWarning`] that names the holder
//!   — the person decides — and a **scheduled** firing onto one is
//!   [`Admission::Skipped`] with the holder recorded, because nobody is
//!   present to weigh the risk (§7.8). `WorkspaceScope::None` is
//!   [`Admission::Refused`] and can never be claimed.
//! - [`remove_working_folder`] is the other end of a scope's life: what
//!   emptying the trash may remove of where a run worked. **Only a folder
//!   Koru made** — a `Temporary` directory — is removed; a project's root or
//!   a folder inside it is the person's own and is left exactly alone, and
//!   the [`Removal`] says which so the trash can tell them.
//! - [`DurableClaims`] is the port `runs.db` implements so a worker restart
//!   rebuilds the registry: a claim whose attempt is still live *or unknown*
//!   is kept (§7.6: unknown is never free), and one whose attempt is known to
//!   have ended is dropped.
//!
//! Privacy is structural here as in the vocabulary crate: a
//! [`ScopeIdentity`]'s `Debug` is a salted hash, a [`BusyWarning`] carries
//! ids and an overlap kind, and a [`ScopeRefusal`] is a code — no path
//! reaches a log, an event or a failure message through this crate.

mod claims;
mod identity;
mod platform;
mod reconcile;
mod removal;

pub use claims::{
    Admission, BusyWarning, Holder, ScheduleRef, ScopeClaim, ScopeClaims, StartOrigin,
};
pub use identity::{
    Overlap, Party, ResolvedScope, RootLookup, ScopeIdentity, ScopeRefusal, ScopeRoots,
};
pub use reconcile::{DurableClaim, DurableClaims, Liveness, ReconcileReport};
pub use removal::{Removal, remove_working_folder};

// The vocabulary this crate admits, re-exported so a caller names one crate.
pub use koru_delegate_core::{ProjectId, RelativeFolder, RunAttemptId, RunId, WorkspaceScope};

#[cfg(test)]
mod tests;
