// SPDX-License-Identifier: MPL-2.0

use crate::{ManagedIntent, PendingInputIntent, TerminalIntent};

/// Every command emitted by a reusable session surface.
///
/// The host adds session identity, controller authority, IPC idempotency, and
/// transport at this one boundary. Views never import those concerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSurfaceIntent {
    Terminal(TerminalIntent),
    PendingInput(PendingInputIntent),
    Managed(ManagedIntent),
}

pub trait SessionSurfaceInteraction {
    fn dispatch(&mut self, intent: SessionSurfaceIntent);
}

impl SessionSurfaceInteraction for () {
    fn dispatch(&mut self, _intent: SessionSurfaceIntent) {}
}
