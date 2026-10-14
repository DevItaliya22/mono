// SPDX-License-Identifier: MPL-2.0

//! Who did something to a run — by id, never by name.
//!
//! A person is their profile, the orchestrator is the run it serves, an agent
//! is the session it runs in. Names are translatable and renameable and
//! identities are not (PRINCIPLES, *a stable identifier never derives from a
//! name*); a surface resolves a name at display time and the run never stores
//! one.

use serde::{Deserialize, Serialize};

use crate::{ProfileId, RunId, SessionId};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActorRef {
    /// A person, as the profile they were acting under.
    Person { profile: ProfileId },
    /// The delegate worker acting for a run.
    Orchestrator { run: RunId },
    /// The provider agent, as the managed session it ran in.
    Agent { session: SessionId },
}
