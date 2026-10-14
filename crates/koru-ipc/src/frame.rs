// SPDX-License-Identifier: MPL-2.0

//! One frame: a magic, a version byte, a length, a JSON body.
//!
//! ```text
//! K O R U   v   len(u32 LE)   JSON body
//! 0 1 2 3   4   5 6 7 8       9..
//! ```
//!
//! ## Why a magic and a version byte before any JSON
//!
//! An endpoint is a well-known name on a shared machine, so the first thing it
//! will meet that is not a Koru client is something else entirely — a probe, a
//! port scanner, a mistyped tool. Four bytes of magic turn "the JSON did not
//! parse" into "that was not a Koru endpoint", which is a different sentence
//! for the person reading it.
//!
//! The version byte is next so that **the format can change without breaking
//! the negotiation that decides it**. A reader meeting a frame version it does
//! not know can still see how long the body is, and so can skip it or close
//! cleanly rather than losing framing altogether.
//!
//! ## The version is a parameter, and this module owns no constant
//!
//! Deliberately. Two protocols are spoken over these frames — the store's
//! rendezvous handshake and the agent's long-lived stream — and they version
//! independently. A constant here would be a third number that had to agree
//! with both, which is the shape that eventually disagrees with one of them.
//! So [`encode`] and [`write_frame`] take the version to stamp, [`read_frame`]
//! hands the one it read back, and what to do about an unfamiliar value is the
//! caller's decision.
//!
//! Note what that decision cannot be at *this* layer: **an unknown frame
//! version cannot be answered with a frame.** A reply would be written in the
//! very format the peer has just shown it does not share. Answering a version
//! disagreement politely is something a *protocol* can do, once both ends agree
//! how to read each other at all.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// The four bytes every frame starts with.
pub const MAGIC: [u8; 4] = *b"KORU";

/// Bytes before the body: magic, version, length.
pub const HEADER_LEN: usize = 9;

/// The largest body either side will read.
///
/// The cap is here so that a hostile — or merely confused — peer cannot make
/// this process allocate a gigabyte by sending a large length and then nothing
/// at all. It is **not** a capacity: it was 64 KB when every frame was a
/// handshake or an agent message, and then trust gossip started to travel over
/// the store's control socket — a `PairingEvents` response carries whole
/// batches of signed records, and a phone's trust store of sixty records is
/// 141 KB as JSON. Over the cap, the store's reply failed to encode, the
/// session broke, the agent reconnected with a fresh snapshot that closed
/// every peer, and the peer re-sent the same batch: a four-second loop that
/// filled the log with "connection lost" and never applied the records.
/// Sixteen MiB holds a batch at `MAX_TRUST_GOSSIP_RECORDS` with room
/// (`a_gossip_batch_at_the_sanity_bound_fits_in_one_frame`, which pins four of
/// them side by side because the reply drains the whole queue).
///
/// **What it costs is real and worth saying**: `read_raw` allocates the
/// declared length before a single body byte arrives, so this is now up to
/// 16 MiB per connection on a peer's say-so rather than 64 KB. That is
/// acceptable *here* and only here — the endpoint is a per-user named pipe or
/// a socket under an `0700` directory, so anything that can declare a length
/// is already running as this user. It is not a number to copy to a socket
/// that anyone can reach.
pub const MAX_BODY: usize = 16 * 1024 * 1024;

/// What a header says about the body that follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub body_len: usize,
}

/// Frame a JSON body at `version`. Fails only if the body is over [`MAX_BODY`].
pub fn encode<T: Serialize>(version: u8, value: &T) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("encode: {e}")))?;
    if body.len() > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "a frame body is {} bytes, over the {MAX_BODY} cap",
                body.len()
            ),
        ));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&MAGIC);
    out.push(version);
    // Little-endian and fixed width, not varint: the header has to be a
    // constant number of bytes or a reader cannot know how much to read first.
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Read a header out of exactly [`HEADER_LEN`] bytes.
///
/// **The version is returned, not enforced.** Refusing an unknown version is a
/// policy decision that belongs to the protocol above — see the module note on
/// why this layer has no polite answer to give.
pub fn parse_header(bytes: &[u8; HEADER_LEN]) -> io::Result<Header> {
    if bytes[..4] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this is not a Koru endpoint (the frame does not begin with KORU)",
        ));
    }
    let body_len = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]) as usize;
    if body_len > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame declared {body_len} bytes, over the {MAX_BODY} cap"),
        ));
    }
    Ok(Header {
        version: bytes[4],
        body_len,
    })
}

/// Read one framed value, blocking. Hands back the version it was stamped with.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> io::Result<(u8, T)> {
    let (version, body) = read_raw(reader)?;
    let value = serde_json::from_slice(&body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("decode: {e}")))?;
    Ok((version, value))
}

/// Read one frame's header and its body as bytes, without deserializing.
///
/// What a long-lived protocol needs and a one-shot handshake does not: the
/// version has to be *accepted* before the body is given a shape, so a stream
/// whose semantics are still being negotiated is never parsed as though they
/// were settled. [`read_frame`] is the shorthand for when the version is
/// already known to be right.
pub fn read_raw<R: Read>(reader: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header)?;
    let header = match parse_header(&header) {
        Ok(header) => header,
        Err(e) => {
            // The one line that answers "something is on this endpoint and it
            // is not us". A port scanner, a mistyped tool, or a peer from a
            // generation whose framing changed.
            crate::ipc_debug!("read: not a frame we know ({e})");
            return Err(e);
        }
    };
    let mut body = vec![0u8; header.body_len];
    reader.read_exact(&mut body)?;
    // **The version and the length, never the body.** An agent's frame carries
    // `match_text`, which is a fragment of what somebody wrote, and a log
    // attached to a report must not be a record of what they typed.
    crate::ipc_debug!("read: v{} {} bytes", header.version, header.body_len);
    Ok((header.version, body))
}

/// Give a body read by [`read_raw`] its shape.
///
/// Here rather than at the call site so that a caller which decides about the
/// version first — the whole reason `read_raw` exists — does not have to link
/// a JSON library to finish the job it started.
pub fn decode<T: for<'de> Deserialize<'de>>(body: &[u8]) -> io::Result<T> {
    serde_json::from_slice(body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("decode: {e}")))
}

/// Write one framed value at `version`, blocking, and flush it.
///
/// The flush is not optional: a named pipe buffers, and a message that sits in
/// a buffer while both sides wait for the other is a deadlock that looks
/// exactly like a hung peer.
pub fn write_frame<W: Write, T: Serialize>(
    writer: &mut W,
    version: u8,
    value: &T,
) -> io::Result<()> {
    let bytes = encode(version, value)?;
    crate::ipc_debug!("write: v{version} {} bytes", bytes.len() - HEADER_LEN);
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests;
