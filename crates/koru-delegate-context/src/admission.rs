// SPDX-License-Identifier: MPL-2.0

//! D48 — after a deletion, the source resolves for nothing new.
//!
//! Koru cannot retract bytes a provider already holds, so an attempt under
//! way is never interrupted by a deletion: nothing in capture or
//! materialization looks at the trash. What Koru controls is what starts
//! *next*. The runtime asks this before a Resume and before a new run
//! that reuses a pin; a source that is now in the trash or unavailable is
//! refused with a stable code, and the first such item is named by index.

use std::fmt;

use crate::{ContextPin, ContextState, ContextStore, Fault, StoreContext};

/// Why a set of pins may not start a new attempt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceRefusal {
    /// Item `item` is now in the trash. Restoring it lifts the refusal.
    InTrash { item: usize },
    /// Item `item` can no longer be opened here.
    Unavailable { item: usize },
    /// The store could not answer for item `item`.
    Unanswered { item: usize, fault: Fault },
}

impl SourceRefusal {
    pub fn item(self) -> usize {
        match self {
            Self::InTrash { item } | Self::Unavailable { item } | Self::Unanswered { item, .. } => {
                item
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InTrash { .. } => "source_in_trash",
            Self::Unavailable { .. } => "source_unavailable",
            Self::Unanswered { .. } => "source_unanswered",
        }
    }
}

impl fmt::Display for SourceRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unanswered { item, fault } => {
                write!(f, "{} (item {item}): {fault}", self.as_str())
            }
            other => write!(f, "{} (item {})", other.as_str(), other.item()),
        }
    }
}

impl std::error::Error for SourceRefusal {}

impl<S: ContextStore> StoreContext<'_, S> {
    /// May these pins start a new attempt? Every pin must be live now.
    /// Reads only; refuses on the first pin that is not.
    pub fn admit(&mut self, pins: &[ContextPin]) -> Result<(), SourceRefusal> {
        for (item, pin) in pins.iter().enumerate() {
            match self.state(pin) {
                Ok(ContextState::Live) => {}
                Ok(ContextState::InTrash) => return Err(SourceRefusal::InTrash { item }),
                Ok(ContextState::Unavailable) => return Err(SourceRefusal::Unavailable { item }),
                Err(fault) => return Err(SourceRefusal::Unanswered { item, fault }),
            }
        }
        Ok(())
    }
}
