// SPDX-License-Identifier: MPL-2.0

//! Splitting an oversized document update into the protocol's own
//! `DocUpdateFragmentHeader` / `DocUpdateFragment` messages, and putting the
//! pieces back together.
//!
//! The protocol refuses to encode any message over 256KB
//! (`loro_protocol::protocol::MAX_MESSAGE_SIZE`), and the reply builder used
//! to drop the oversized `DocUpdate` on the floor — so a document whose
//! catch-up outgrew the limit could never reach a new device again, silently.
//! Fragments are the protocol's answer, not a Koru extension; this module is
//! just the two halves of using them. Blobs are unaffected either way: they
//! travel over their own ranged `BlobRequest`/`BlobChunk` frames and never
//! pass through the capped encoder.

use loro_protocol::protocol::{BatchId, CrdtType, ProtocolMessage};

use super::AuthenticatedPeer;
use koru_store_core::WorkspaceId;

/// One fragment's payload. The whole encoded message must stay under the
/// protocol's 256KB `MAX_MESSAGE_SIZE`; this leaves room for the envelope.
const FRAGMENT_PAYLOAD_BYTES: usize = 200 * 1024;

/// A document update as protocol messages: one `DocUpdate` when it fits, a
/// fragment header plus fragments when it does not.
pub(super) fn doc_update_messages(
    crdt: CrdtType,
    room_id: &str,
    bytes: Vec<u8>,
) -> Vec<ProtocolMessage> {
    if bytes.len() <= FRAGMENT_PAYLOAD_BYTES {
        return vec![ProtocolMessage::DocUpdate {
            crdt,
            room_id: room_id.to_owned(),
            updates: vec![bytes],
            batch_id: BatchId([0; 8]),
        }];
    }
    let total_size_bytes = bytes.len() as u64;
    let chunks: Vec<Vec<u8>> = bytes
        .chunks(FRAGMENT_PAYLOAD_BYTES)
        .map(<[u8]>::to_vec)
        .collect();
    let mut messages = Vec::with_capacity(chunks.len() + 1);
    messages.push(ProtocolMessage::DocUpdateFragmentHeader {
        crdt,
        room_id: room_id.to_owned(),
        batch_id: BatchId([0; 8]),
        fragment_count: chunks.len() as u64,
        total_size_bytes,
    });
    for (index, fragment) in chunks.into_iter().enumerate() {
        messages.push(ProtocolMessage::DocUpdateFragment {
            crdt,
            room_id: room_id.to_owned(),
            batch_id: BatchId([0; 8]),
            index: index as u64,
            fragment,
        });
    }
    messages
}

/// Reassemble fragmented updates within one batch back into `DocUpdate`s.
/// Fragments only ever travel inside the batch that carries their header —
/// the transport is request/response, so there is no cross-frame state to
/// keep. A group that does not add up is logged and dropped rather than
/// applied partially; the version comparison on the next pass retries it.
pub(super) fn reassemble(
    peer: &AuthenticatedPeer,
    workspace: WorkspaceId,
    messages: Vec<ProtocolMessage>,
) -> Vec<ProtocolMessage> {
    use std::collections::BTreeMap;
    struct Group {
        crdt: CrdtType,
        fragment_count: u64,
        total_size_bytes: u64,
        fragments: BTreeMap<u64, Vec<u8>>,
    }
    let mut groups: BTreeMap<(String, [u8; 8]), Group> = BTreeMap::new();
    let mut passthrough = Vec::with_capacity(messages.len());
    for message in messages {
        match message {
            ProtocolMessage::DocUpdateFragmentHeader {
                crdt,
                room_id,
                batch_id,
                fragment_count,
                total_size_bytes,
            } => {
                groups.insert(
                    (room_id, batch_id.0),
                    Group {
                        crdt,
                        fragment_count,
                        total_size_bytes,
                        fragments: BTreeMap::new(),
                    },
                );
            }
            ProtocolMessage::DocUpdateFragment {
                room_id,
                batch_id,
                index,
                fragment,
                ..
            } => match groups.get_mut(&(room_id.clone(), batch_id.0)) {
                Some(group) => {
                    group.fragments.insert(index, fragment);
                }
                None => tracing::warn!(
                    peer = %peer.device_id.to_storage(),
                    peer_name = %peer.label(),
                    workspace = %workspace.to_storage(),
                    room_id,
                    index,
                    "sync fragment arrived without a header; dropped"
                ),
            },
            other => passthrough.push(other),
        }
    }
    for ((room_id, batch_id), group) in groups {
        let received = group.fragments.len() as u64;
        let bytes: Vec<u8> = group.fragments.into_values().flatten().collect();
        if received != group.fragment_count || bytes.len() as u64 != group.total_size_bytes {
            tracing::warn!(
                peer = %peer.device_id.to_storage(),
                peer_name = %peer.label(),
                workspace = %workspace.to_storage(),
                room_id,
                received,
                expected = group.fragment_count,
                bytes = bytes.len(),
                expected_bytes = group.total_size_bytes,
                "sync fragmented update incomplete; dropped"
            );
            continue;
        }
        tracing::info!(
            peer = %peer.device_id.to_storage(),
            peer_name = %peer.label(),
            workspace = %workspace.to_storage(),
            room_id,
            fragments = received,
            bytes = bytes.len(),
            "sync fragmented update reassembled"
        );
        passthrough.push(ProtocolMessage::DocUpdate {
            crdt: group.crdt,
            room_id,
            updates: vec![bytes],
            batch_id: BatchId(batch_id),
        });
    }
    passthrough
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small update keeps the single-message shape the wire has always
    /// carried.
    #[test]
    fn a_small_update_stays_one_doc_update() {
        let messages = doc_update_messages(CrdtType::Loro, "document:x", vec![1, 2, 3]);
        assert_eq!(messages.len(), 1);
        assert!(
            matches!(&messages[0], ProtocolMessage::DocUpdate { updates, .. } if updates == &vec![vec![1, 2, 3]])
        );
    }

    /// Every message of a fragmented update must fit the wire's own cap, or
    /// splitting was pointless.
    #[test]
    fn every_fragment_encodes_under_the_wire_cap() {
        let bytes: Vec<u8> = (0..600 * 1024).map(|i| (i % 251) as u8).collect();
        let messages = doc_update_messages(CrdtType::Loro, "document:x", bytes);
        assert!(messages.len() > 2, "600KB must fragment");
        for message in &messages {
            loro_protocol::encoding::encode(message).expect("every fragment fits the wire cap");
        }
    }
}
