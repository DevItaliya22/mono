// SPDX-License-Identifier: MPL-2.0

//! Blocking Socials persistence over the existing Store client and protocol.
//!
//! Callers own their worker and client. This crate creates no worker, opens no
//! root and performs no work until a persistence function is called. Updates
//! retain the authoring surface's snapshot comparison and durable batch ack;
//! these checks are not an atomic compare-and-swap against concurrent writers.

mod watch;
pub use watch::{Change, Watch};

use std::time::{Duration, Instant};

use koru_api_client::{Client, Connection, Event};
use koru_socials::{Channel, Document, POST_KIND, Post};
use koru_store_core::{DocumentId, WorkspaceId};
use loro::{ExportMode, LoroDoc, LoroValue};
use uuid::Uuid;

pub const CHANNEL_KIND: &str = "koru-social-channel";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Changed,
    Invalid,
}

/// Read a post from its persisted snapshot, refusing unsupported data.
pub fn load_post(client: &Client, workspace: WorkspaceId, id: Uuid) -> Result<Post, Error> {
    let bytes = client
        .store()
        .snapshot(workspace, DocumentId::from_uuid(id))
        .map_err(|_| Error::Unavailable)?;
    Document::from_snapshot(&bytes)
        .and_then(|document| document.read())
        .map_err(|_| Error::Invalid)
}

/// Save unchanged retries idempotently; refuse an existing post whose current
/// value differs from the caller's base. Successful updates have a durable ack.
pub fn save_post(
    client: &Client,
    workspace: WorkspaceId,
    post: &Post,
    base: Option<&Post>,
) -> Result<(), Error> {
    let id = DocumentId::from_uuid(post.id);
    if let Some(base) = base {
        let bytes = client
            .store()
            .snapshot(workspace, id)
            .map_err(|_| Error::Unavailable)?;
        let document = Document::from_snapshot(&bytes).map_err(|_| Error::Invalid)?;
        let current = document.read().map_err(|_| Error::Invalid)?;
        if &current == post {
            return Ok(());
        }
        if &current != base {
            return Err(Error::Changed);
        }
        let version = document.loro.oplog_vv();
        document.write(post).map_err(|_| Error::Invalid)?;
        let update = document
            .loro
            .export(ExportMode::updates(&version))
            .map_err(|_| Error::Invalid)?;
        commit(client, id, &version, update)?;
    } else {
        // Stable client-minted ID makes a lost create response recoverable.
        match client.store().snapshot(workspace, id) {
            Ok(bytes) => {
                if Document::from_snapshot(&bytes)
                    .and_then(|d| d.read())
                    .ok()
                    .as_ref()
                    == Some(post)
                {
                    return Ok(());
                }
                return Err(Error::Changed);
            }
            Err(error) if error.code == koru_store_core::errors::Code::DocumentNotFound => {}
            Err(_) => return Err(Error::Unavailable),
        }
        let document = Document::new(post).map_err(|_| Error::Invalid)?;
        client
            .store()
            .import_document_at(
                workspace,
                id,
                &post.title,
                POST_KIND,
                None,
                document.snapshot().map_err(|_| Error::Invalid)?,
            )
            .map_err(|_| Error::Unavailable)?;
    }
    Ok(())
}

/// Persist non-secret channel metadata using the existing format version 1.
pub fn save_channel(
    client: &Client,
    workspace: WorkspaceId,
    channel: &Channel,
) -> Result<(), Error> {
    let id = DocumentId::from_uuid(channel.id);
    let doc = LoroDoc::new();
    let existing = match client.store().snapshot(workspace, id) {
        Ok(bytes) => {
            read_channel(&bytes).ok_or(Error::Invalid)?;
            doc.import(&bytes).map_err(|_| Error::Invalid)?;
            true
        }
        Err(error) if error.code == koru_store_core::errors::Code::DocumentNotFound => false,
        Err(_) => return Err(Error::Unavailable),
    };
    let version = doc.oplog_vv();
    let root = doc.get_map("social_channel");
    root.insert("schema", 1_i64).map_err(|_| Error::Invalid)?;
    root.insert(
        "channel",
        serde_json::to_string(channel).map_err(|_| Error::Invalid)?,
    )
    .map_err(|_| Error::Invalid)?;
    if existing {
        let update = doc
            .export(ExportMode::updates(&version))
            .map_err(|_| Error::Invalid)?;
        commit(client, id, &version, update)
    } else {
        client
            .store()
            .import_document_at(
                workspace,
                id,
                &channel.name,
                CHANNEL_KIND,
                None,
                doc.export(ExportMode::Snapshot)
                    .map_err(|_| Error::Invalid)?,
            )
            .map_err(|_| Error::Unavailable)?;
        Ok(())
    }
}

/// Decode a supported channel snapshot; malformed or newer data is unreadable.
pub fn read_channel(bytes: &[u8]) -> Option<Channel> {
    let doc = LoroDoc::new();
    doc.import(bytes).ok()?;
    let LoroValue::Map(values) = doc.get_map("social_channel").get_deep_value() else {
        return None;
    };
    if values.get("schema") != Some(&LoroValue::I64(1)) {
        return None;
    }
    let LoroValue::String(encoded) = values.get("channel")? else {
        return None;
    };
    serde_json::from_str(encoded.as_str()).ok()
}

/// Wait for the exact durable batch acknowledgement on the Store protocol.
/// Timeout is a failed save, never evidence of a successful commit.
pub fn commit(
    client: &Client,
    id: DocumentId,
    version: &loro::VersionVector,
    update: Vec<u8>,
) -> Result<(), Error> {
    let mut socket =
        Connection::open(&client.store().discovery()).map_err(|_| Error::Unavailable)?;
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| Error::Unavailable)?;
    let room = format!("document:{}", id.to_storage());
    socket
        .join(&room, version)
        .map_err(|_| Error::Unavailable)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut batch = None;
    while Instant::now() < deadline {
        match socket.next_event().map_err(|_| Error::Unavailable)? {
            Some(Event::Joined {
                room: joined,
                version: remote,
                ..
            }) if joined == room && batch.is_none() => {
                let remote = if remote.is_empty() {
                    loro::VersionVector::default()
                } else {
                    loro::VersionVector::decode(&remote).map_err(|_| Error::Invalid)?
                };
                if &remote != version {
                    return Err(Error::Changed);
                }
                batch = Some(
                    socket
                        .send_update(&room, update.clone())
                        .map_err(|_| Error::Unavailable)?,
                );
            }
            Some(Event::Updates { updates, .. })
                if updates.iter().any(|u| !u.is_empty()) && batch.is_none() =>
            {
                // An edit arrived after the snapshot. Preserve the caller's
                // working copy rather than overwrite unseen work.
                return Err(Error::Changed);
            }
            Some(Event::Acked { ack, .. }) if Some(ack.batch) == batch => {
                return if ack.durable {
                    Ok(())
                } else {
                    Err(Error::Unavailable)
                };
            }
            Some(Event::JoinRefused { .. } | Event::RoomProblem { .. }) => {
                return Err(Error::Unavailable);
            }
            _ => {}
        }
    }
    Err(Error::Unavailable)
}
