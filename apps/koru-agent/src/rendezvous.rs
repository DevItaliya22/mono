// SPDX-License-Identifier: MPL-2.0

//! Binding the agent's endpoint, and answering it.
//!
//! ## Binding is the claim; accepting is the readiness signal
//!
//! Two moments, and the gap is load-bearing. Binding is what makes an agent a
//! singleton per user and channel — a second one fails there, before it has
//! touched anything — and a shell that connects in the window between the two
//! waits at `accept` rather than being told an address that does not work yet.
//!
//! ## The handshake happens on the accept thread, before anything is spawned
//!
//! A peer that will not be talked to should never have had two threads started
//! for it. So the version negotiation is here, synchronously, and only a shell
//! that is welcomed becomes a [`crate::connection::Connection`].
//!
//! **And it is two decisions, not one.** An unknown *frame* version closes the
//! socket without a word — a refusal is itself a frame, and writing one in a
//! format the peer has just shown it does not share is a promise that cannot be
//! kept. An unknown *protocol* version gets `AgentRefused`, naming both numbers.
//! [`crate::handshake`] is where that lives, and it is pure so both branches are
//! assertable without a socket.
//!
//! ## Why the accept loop is a thread rather than the winit loop's business
//!
//! `accept` blocks, and the loop may never block. The thread raises
//! [`AgentEvent::ShellArrived`] and hands the connection over; everything after
//! that is the loop's.

use koru_agent_wire::{AgentGreeting, FRAME_VERSION, ShellHello, SyncStatus};
use koru_ipc::{Endpoint, IpcError, Listener};

use crate::connection::{self, Connection};
use crate::event::{AgentEvent, Waker};
use crate::handshake::{self, Framing};
use crate::registry::ConnectionId;

/// How long a peer has to introduce itself before it is dropped.
///
/// A handshake is one frame each way and answers in microseconds. The cap is
/// here so that something which connects and then says nothing cannot hold an
/// accept thread — a port scanner does exactly that.
const HELLO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Claim the agent's endpoint. Holding this is what makes this process *the*
/// agent for its user and channel.
pub fn bind(endpoint: Endpoint) -> Result<Listener, IpcError> {
    Listener::bind(endpoint)
}

/// Answer the endpoint until the process ends.
///
/// Runs on a thread of its own. `hand_over` is called on that thread with each
/// welcomed connection, before the loop is told about it, so the loop never
/// receives an id it has no connection for.
pub fn serve(
    listener: Listener,
    device_id: String,
    instance_id: String,
    sync: SyncStatus,
    waker: Waker,
    hand_over: impl Fn(Connection) + Send + 'static,
) {
    let mut next = 0u64;
    loop {
        let Ok(stream) = listener.accept() else {
            // One bad accept is not a reason to stop answering. An agent that
            // stopped would still hold the name — so every shell would find it
            // and none could talk to it, which is the worst of the three
            // possible states.
            continue;
        };
        next += 1;
        let id = ConnectionId(next);
        let pid = koru_ipc::peer_pid(&stream);

        if !koru_ipc::same_user(&stream) {
            // No reply at all. Telling an unauthorised peer *why* confirms an
            // agent is here and names its channel, which is what the endpoint's
            // permissions exist to withhold.
            continue;
        }
        koru_ipc::set_timeouts(&stream, Some(HELLO_TIMEOUT));

        match greet(&stream, &device_id, &instance_id, &sync) {
            Greeted::Welcomed => {}
            Greeted::Refused | Greeted::Unreadable => continue,
        }
        // The timeouts were for the handshake. A welcomed shell may then sit
        // idle for hours on purpose, and a deadline would turn that into a
        // disconnection.
        koru_ipc::set_timeouts(&stream, None);

        match connection::adopt(id, stream, waker.clone()) {
            Ok(connection) => {
                hand_over(connection);
                waker.wake(AgentEvent::ShellArrived {
                    connection: id,
                    pid,
                });
            }
            Err(e) => koru_ipc::ipc_debug!("shell {}: could not start its threads: {e}", id.0),
        }
    }
}

enum Greeted {
    Welcomed,
    Refused,
    /// The framing was not ours. Closed without a word — see the module note.
    Unreadable,
}

fn greet(
    stream: &koru_ipc::Stream,
    device_id: &str,
    instance_id: &str,
    sync: &SyncStatus,
) -> Greeted {
    let mut reader = stream;
    let Ok((version, body)) = koru_ipc::frame::read_raw(&mut reader) else {
        return Greeted::Unreadable;
    };
    if handshake::framing(version) != Framing::Understood {
        koru_ipc::ipc_debug!("hello: frame v{version} is not ours; closing without a reply");
        return Greeted::Unreadable;
    }
    // **Only now is a shape given to it.** A stream whose framing has just been
    // agreed may still be speaking a protocol we do not, and parsing its bodies
    // as though it were not is how a field that changed meaning arrives as a
    // silent default.
    let Ok(hello) = koru_ipc::frame::decode::<ShellHello>(&body) else {
        return Greeted::Unreadable;
    };

    let greeting = handshake::accept_with_sync(&hello, device_id, instance_id, sync);
    let welcomed = matches!(greeting, AgentGreeting::Welcome(_));
    let mut writer = stream;
    if koru_ipc::frame::write_frame(&mut writer, FRAME_VERSION, &greeting).is_err() {
        return Greeted::Unreadable;
    }
    if welcomed {
        koru_ipc::ipc_debug!("hello: {} {} welcomed", hello.product, hello.version);
        Greeted::Welcomed
    } else {
        koru_ipc::ipc_debug!(
            "hello: {} speaks protocol {} and this agent speaks {}; refused",
            hello.product,
            hello.protocol_version,
            koru_agent_wire::PROTOCOL_VERSION
        );
        Greeted::Refused
    }
}
