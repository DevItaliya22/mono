// SPDX-License-Identifier: MPL-2.0

//! The control-handover rows (§7.4): the human lease granted, and the
//! orchestrator's reacquisition.

use koru_delegate_core::RunAttemptId;
use koru_events::{Actor, Envelope, event_name};

use crate::chain::{RunChain, Stamp, flag, id};

/// The human lease was granted. `discovered` marks a takeover recovery found
/// already in place after a crash between release and attach.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ControlTaken {
    pub attempt: RunAttemptId,
    pub discovered: bool,
}

/// The orchestrator reacquired the session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ControlReturned {
    pub attempt: RunAttemptId,
}

impl RunChain {
    /// `delegate.control.taken` — recorded only when the lease was granted,
    /// never on a request. The person is the actor, discovered or not: a
    /// takeover found after a crash is still theirs.
    pub fn control_taken(&self, stamp: Stamp, fact: ControlTaken) -> Envelope {
        let draft = self
            .row(event_name!("delegate.control.taken"), Actor::user())
            .attr("attempt", id(fact.attempt.as_uuid()))
            .attr("discovered", flag(fact.discovered));
        self.finish(draft, stamp)
    }

    /// `delegate.control.returned` — recorded only when reacquisition
    /// succeeded.
    pub fn control_returned(&self, stamp: Stamp, fact: ControlReturned) -> Envelope {
        let draft = self
            .row(
                event_name!("delegate.control.returned"),
                self.orchestrator(),
            )
            .attr("attempt", id(fact.attempt.as_uuid()));
        self.finish(draft, stamp)
    }
}
