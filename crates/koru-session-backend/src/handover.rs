// SPDX-License-Identifier: MPL-2.0

//! A run can create its session after this window read the catalog. Subscribe
//! before asking for control, so a release at an approval cannot go unseen.

use super::*;
use koru_session_protocol::{ControlRequestOutcome, HandoverState, TakeoverUrgency};

pub(super) fn request<S: BackendSink>(
    client: &Arc<SessionClient>,
    sink: &S,
    internal: &SyncSender<BackendCommand>,
    generation: u64,
    subscribed: &mut BTreeSet<SessionId>,
    session: SessionId,
    urgency: TakeoverUrgency,
) -> Result<(), ClientError> {
    subscribe_session(client, internal, generation, subscribed, session)?;
    // Give the window the new session's summary before it builds the response
    // tab. A release after this read is covered by the subscription above.
    refresh(client, sink, internal, generation, subscribed, false);
    let outcome = client.request_control(session, urgency)?;
    send(
        sink,
        BackendEvent::Handover {
            session_id: session,
            handover: match outcome {
                ControlRequestOutcome::Available => HandoverState::Released,
                ControlRequestOutcome::Waiting => HandoverState::Requested,
            },
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests;
