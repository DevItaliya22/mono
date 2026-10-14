// SPDX-License-Identifier: MPL-2.0

//! What a pin points at *now* — live, in trash, or no longer available
//! (D46, D47).
//!
//! A [`crate::RunContextItem`] is the run's record of what it received, and
//! it never changes. This is the *other* half a surface shows beside it: the
//! target's current state, read from whatever holds the source at the moment
//! of the question. Delegation purges nothing and restores nothing; it
//! reports.
//!
//! It lives in the vocabulary crate rather than beside the code that reads
//! it, because three parties speak it and only one of them can open a store:
//! `koru-delegate-context` computes it, `/v1/runs` carries it, and a surface
//! renders it.

use serde::{Deserialize, Serialize};

/// The current state of a pinned source. The pin itself never changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ContextState {
    /// Listed, not in the trash, and the pinned bytes are retained.
    Live,
    /// In the workspace's trash — its own stamp or an ancestor's. The
    /// pinned bytes are still retained; restore makes it live again.
    InTrash,
    /// The pinned bytes cannot be opened here: the document was purged
    /// (its versions cascade with it), the blob is gone, or the workspace
    /// is.
    Unavailable,
}

impl ContextState {
    /// The closed token this state is reported under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::InTrash => "in_trash",
            Self::Unavailable => "unavailable",
        }
    }
}
