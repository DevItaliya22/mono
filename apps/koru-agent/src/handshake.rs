// SPDX-License-Identifier: MPL-2.0

//! What the agent says to a shell that has just introduced itself.
//!
//! ## Two versions, at two layers, and only one of them can be answered
//!
//! ```text
//! layer          owns                         an unknown value means
//! ─────────────────────────────────────────────────────────────────────────
//! koru-ipc       the frame: KORU, version      "this is not something I can
//!                byte, length, body            read at all"  -> close, log
//! koru-agent-    PROTOCOL_VERSION: what the    "I can read you, and I do not
//! wire           messages mean                 speak this"   -> Refused
//! ```
//!
//! **An unknown frame version is closed, not refused.** A refusal is itself a
//! frame; writing one in a format the peer has just shown it does not share is
//! a promise that cannot be kept. So the transport half of this — [`accept`]'s
//! caller — reads a frame's body as *bytes* and consults [`framing`] before
//! anything is given a shape.
//!
//! Only once the framing is agreed does the minimal `ShellHello` get
//! deserialized, and only once *that* is accepted does anything else. A stream
//! whose semantics are still being negotiated must never have its bodies parsed
//! as though they were settled.

use koru_agent_wire::{
    AgentGreeting, AgentWelcome, FRAME_VERSION, PROTOCOL_VERSION, ShellHello, SyncStatus,
};

/// What to do about the version byte on an incoming frame.
///
/// There is no `Refuse` arm, and that absence is the point — see the module
/// note.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Framing {
    /// Read the body; it is a shape we know.
    Understood,
    /// Log which version arrived and drop the connection. Nothing can be
    /// written back that the peer would be able to read.
    Close,
}

pub fn framing(version: u8) -> Framing {
    if version == FRAME_VERSION {
        Framing::Understood
    } else {
        Framing::Close
    }
}

/// What to answer a `ShellHello`.
///
/// A pure function of the hello and this build's identity, so every branch is
/// assertable without a socket.
pub fn accept(hello: &ShellHello, device_id: &str, instance_id: &str) -> AgentGreeting {
    accept_with_sync(hello, device_id, instance_id, &SyncStatus::default())
}

pub fn accept_with_sync(
    hello: &ShellHello,
    device_id: &str,
    instance_id: &str,
    sync: &SyncStatus,
) -> AgentGreeting {
    if hello.protocol_version != PROTOCOL_VERSION {
        // **Both numbers, named.** "Incompatible" on its own sends somebody to
        // read two changelogs to find out which end is behind.
        return AgentGreeting::Refused {
            what: "protocol".to_string(),
            ours: PROTOCOL_VERSION,
            theirs: hello.protocol_version,
            message: format!(
                "this agent speaks agent protocol {PROTOCOL_VERSION} and {} speaks {}; \
                 one of them has to be updated",
                hello.product, hello.protocol_version,
            ),
        };
    }
    AgentGreeting::Welcome(Box::new(AgentWelcome {
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: PROTOCOL_VERSION,
        device_id: device_id.to_string(),
        instance_id: instance_id.to_string(),
        sync: sync.clone(),
    }))
}

#[cfg(test)]
mod tests;
