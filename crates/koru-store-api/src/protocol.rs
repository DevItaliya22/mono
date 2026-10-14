// SPDX-License-Identifier: MPL-2.0

//! The Loro Protocol, translated into store operations.
//!
//! **No Koru-specific extensions.** Every message on the wire is one the
//! `loro-protocol` codec defines, so a client built against the published
//! protocol — knowing nothing about Koru — can join an authorised room and
//! sync. That is a hard constraint, not an aspiration: the moment a Koru
//! message appears here, the store stops being an API platform and becomes an
//! app with a private channel.
//!
//! Authentication therefore happens on the **WebSocket upgrade**, as an
//! ordinary HTTP header, and not inside the protocol.
//!
//! ## The order, which is the whole contract
//!
//! ```text
//! DocUpdate arrives
//!   │
//!   ├─ 1  import into the live document      validates; a bad update is
//!   │                                        refused here and never stored
//!   ├─ 2  commit                             one transaction, one fsync
//!   ├─ 3  Ack{ ref_id, status: Ok }          ← never before 2
//!   └─ 4  broadcast to the room's others     ← never before 2 either
//! ```
//!
//! Step 3 before step 2 would make an acknowledgement a lie. Step 4 before
//! step 2 would let another client see — and build on — work a power cut could
//! still erase. Both are one line out of place, which is why the order is
//! written here as well as implemented below.
//!
//! ## What an `Ack` means
//!
//! `UpdateStatusCode::Ok` means **durably persisted on this machine**. Not
//! received, not queued, not replicated anywhere else. A client may discard
//! nothing on the strength of it except its own retry.
//!
//! ## Koru's error codes travel in `app_code` — but only with `AppError`
//!
//! The protocol's own code sets are small and general — `JoinErrorCode` has
//! four members, `RoomErrorCode` three. Koru has twenty-four. The protocol
//! provides `app_code` for exactly this, and a refusal carries Koru's stable
//! token there.
//!
//! **The codec only writes `app_code` when the code is `AppError`.** With any
//! other code the field is dropped on encode and comes back `None`, silently —
//! `encoding.rs` guards it with `if matches!(code, JoinErrorCode::AppError)`.
//! So the rule here is:
//!
//! - a refusal the protocol can express on its own — a bad credential is
//!   `AuthFailed` — uses that code and **no** `app_code`, because the protocol
//!   code already is the information and a stock client acts on it correctly;
//! - a refusal only Koru can express uses `AppError` **with** the token.
//!
//! Setting `app_code` beside `AuthFailed` looks right, compiles, and reaches
//! the client as nothing at all.

use koru_store_core::errors::{Code, Error};
use koru_store_core::ids::{DocumentId, WorkspaceId};
use koru_store_core::model::{Scope, Timestamp};
use loro_protocol::protocol::{
    BatchId, CrdtType, JoinErrorCode, Permission, ProtocolMessage, RoomErrorCode, UpdateStatusCode,
};

use crate::auth::Caller;
use crate::rooms::Room;
use crate::state::KoruStore;

/// Where a room's contents actually live.
#[derive(Copy, Clone, Debug)]
pub struct Target {
    pub workspace: WorkspaceId,
    pub document: DocumentId,
}

/// Resolve a room to a workspace and a document.
///
/// A manifest room names its workspace directly. A document room names only the
/// document, so the workspace comes from the location index — which is why that
/// index exists.
pub fn resolve(store: &mut KoruStore, room: &Room) -> Result<Option<Target>, Error> {
    match room {
        Room::Manifest(workspace) => {
            let document = store.manifest_id(*workspace)?;
            Ok(Some(Target {
                workspace: *workspace,
                document,
            }))
        }
        Room::Document(document) => Ok(store.locate(*document)?.map(|workspace| Target {
            workspace,
            document: *document,
        })),
    }
}

/// What handling a message produced.
pub struct Outcome {
    /// Sent back to whoever sent the message.
    pub reply: Option<ProtocolMessage>,
    /// Sent to everyone else in the room — **only ever after a commit**.
    pub broadcast: Option<ProtocolMessage>,
    /// Rooms this connection has now joined or left.
    pub joined: Option<Room>,
    pub left: Option<Room>,
    /// A client update reached the durable Store commit boundary.
    ///
    /// This is separate from `broadcast`: a catch-up is broadcast-shaped but
    /// changes nothing locally, while a committed update must wake replication
    /// even if its acknowledgement or room relay cannot be sent.
    pub durable_change: bool,
}

impl Outcome {
    fn nothing() -> Self {
        Self {
            reply: None,
            broadcast: None,
            joined: None,
            left: None,
            durable_change: false,
        }
    }

    fn reply(message: ProtocolMessage) -> Self {
        Self {
            reply: Some(message),
            ..Self::nothing()
        }
    }
}

/// Handle one decoded message.
///
/// Takes the store lock for the duration, which is what serialises commits.
pub fn handle(
    store: &mut KoruStore,
    caller: &Caller,
    message: ProtocolMessage,
    now: Timestamp,
) -> Outcome {
    match message {
        ProtocolMessage::JoinRequest {
            crdt,
            room_id,
            version,
            ..
        } => join(store, caller, crdt, room_id, version),

        ProtocolMessage::DocUpdate {
            crdt,
            room_id,
            updates,
            batch_id,
        } => update(store, caller, crdt, room_id, updates, batch_id, now),

        ProtocolMessage::Leave { crdt, room_id } => match Room::parse(&room_id) {
            Some(room) => Outcome {
                left: Some(room),
                ..Outcome::nothing()
            },
            None => Outcome::reply(room_error(
                crdt,
                room_id,
                RoomErrorCode::Unknown,
                "no such room",
            )),
        },

        // The WebSocket adapter reassembles fragments before this
        // transport-neutral handler. Seeing one here means another adapter did
        // not implement that required boundary, so refuse it explicitly rather
        // than silently dropping an edit.
        ProtocolMessage::DocUpdateFragmentHeader { crdt, room_id, .. }
        | ProtocolMessage::DocUpdateFragment { crdt, room_id, .. } => Outcome::reply(room_error(
            crdt,
            room_id,
            RoomErrorCode::Unknown,
            "fragmented updates must be reassembled by the transport",
        )),

        // Responses. A client sending one is confused; saying so is more use
        // than ignoring it.
        ProtocolMessage::JoinResponseOk { crdt, room_id, .. }
        | ProtocolMessage::JoinError { crdt, room_id, .. }
        | ProtocolMessage::Ack { crdt, room_id, .. }
        | ProtocolMessage::RoomError { crdt, room_id, .. } => Outcome::reply(room_error(
            crdt,
            room_id,
            RoomErrorCode::Unknown,
            "that is a message the store sends, not one it accepts",
        )),
    }
}

fn join(
    store: &mut KoruStore,
    caller: &Caller,
    crdt: CrdtType,
    room_id: String,
    version: Vec<u8>,
) -> Outcome {
    if !caller.allows(Scope::ReadDocuments) {
        return Outcome::reply(ProtocolMessage::JoinError {
            crdt,
            room_id,
            code: JoinErrorCode::AuthFailed,
            message: "this credential cannot read documents".into(),
            receiver_version: None,
            // No `app_code`: the codec would drop it beside `AuthFailed`, and
            // that code already says everything a client needs.
            app_code: None,
        });
    }

    let Some(room) = Room::parse(&room_id) else {
        return Outcome::reply(ProtocolMessage::JoinError {
            crdt,
            room_id,
            code: JoinErrorCode::AppError,
            message: "no such room".into(),
            receiver_version: None,
            app_code: Some(Code::DocumentNotFound.as_str().into()),
        });
    };
    let target = match resolve(store, &room) {
        Ok(Some(target)) => target,
        Ok(None) => {
            tracing::warn!(room = %room_id, "room join refused: no such room");
            return Outcome::reply(ProtocolMessage::JoinError {
                crdt,
                room_id,
                code: JoinErrorCode::AppError,
                message: "no such room".into(),
                receiver_version: None,
                app_code: Some(Code::DocumentNotFound.as_str().into()),
            });
        }
        Err(error) => {
            tracing::error!(
                room = %room_id,
                code = error.code().as_str(),
                detail = error.message(),
                "room join failed"
            );
            return Outcome::reply(ProtocolMessage::JoinError {
                crdt,
                room_id,
                code: JoinErrorCode::AppError,
                message: error.message().into(),
                receiver_version: None,
                app_code: Some(error.code().as_str().into()),
            });
        }
    };

    // The store's own position, so the client knows what it is missing. The
    // catch-up itself goes as an ordinary `DocUpdate` below.
    let ours = match store.version(target.workspace, target.document) {
        Ok(v) => v,
        Err(e) => {
            return Outcome::reply(ProtocolMessage::JoinError {
                crdt,
                room_id,
                code: JoinErrorCode::AppError,
                message: e.message().to_string(),
                receiver_version: None,
                app_code: Some(e.code().as_str().into()),
            });
        }
    };

    // Whatever the client is missing, in one delta. A client with no history at
    // all sends an empty version vector and gets everything — which is the only
    // form that applies, because a delta from a non-empty version names
    // ancestors an empty replica cannot obtain and Loro would park it as
    // pending with no error to notice.
    //
    // **Whether to send is decided by comparing versions, never by testing the
    // encoded bytes.** An update covering no operations still carries a header,
    // so it is never empty — `!bytes.is_empty()` looks like the obvious guard
    // and sends a pointless frame on every single join. `koru-docstore`'s
    // `publish_new` records the same trap; this is the second place to meet it.
    let theirs = decode_version(&version);
    let behind = !theirs.includes_vv(&ours);
    let catch_up = behind
        .then(|| store.updates_since(target.workspace, target.document, &theirs))
        .and_then(|r| r.ok());

    let permission = if caller.allows(Scope::EditDocuments) {
        Permission::Write
    } else {
        Permission::Read
    };

    Outcome {
        reply: Some(ProtocolMessage::JoinResponseOk {
            crdt,
            room_id: room_id.clone(),
            permission,
            version: ours.encode(),
            extra: None,
        }),
        // Not a broadcast: this goes to the joiner, and the connection sends it
        // straight after the response.
        broadcast: catch_up.map(|updates| ProtocolMessage::DocUpdate {
            crdt,
            room_id,
            updates: vec![updates],
            batch_id: BatchId([0; 8]),
        }),
        joined: Some(room),
        left: None,
        durable_change: false,
    }
}

fn update(
    store: &mut KoruStore,
    caller: &Caller,
    crdt: CrdtType,
    room_id: String,
    updates: Vec<Vec<u8>>,
    batch_id: BatchId,
    now: Timestamp,
) -> Outcome {
    if !caller.allows(Scope::EditDocuments) {
        return Outcome::reply(ack(
            crdt,
            room_id,
            batch_id,
            UpdateStatusCode::PermissionDenied,
        ));
    }
    let Some(room) = Room::parse(&room_id) else {
        tracing::warn!(room = %room_id, "update refused: no such room");
        return Outcome::reply(room_error(
            crdt,
            room_id,
            RoomErrorCode::Unknown,
            "no such room",
        ));
    };
    let target = match resolve(store, &room) {
        Ok(Some(target)) => target,
        Ok(None) => {
            tracing::warn!(room = %room_id, "update refused: no such room");
            return Outcome::reply(room_error(
                crdt,
                room_id,
                RoomErrorCode::Unknown,
                "no such room",
            ));
        }
        Err(error) => {
            tracing::error!(
                room = %room_id,
                code = error.code().as_str(),
                detail = error.message(),
                "update room resolution failed"
            );
            return Outcome::reply(room_error(
                crdt,
                room_id,
                RoomErrorCode::Unknown,
                error.message(),
            ));
        }
    };

    // 1. Validate. A malformed update is refused before anything is written,
    //    so the journal can never hold bytes that will not replay.
    for bytes in &updates {
        if let Err(e) = store.apply_update(
            target.workspace,
            target.document,
            bytes.clone(),
            now,
            koru_store_core::ports::UpdateOrigin::Store,
        ) {
            tracing::error!(
                room = %room_id,
                workspace = %target.workspace,
                document = %target.document,
                code = e.code().as_str(),
                detail = e.message(),
                "store update validation failed"
            );
            return Outcome::reply(ack(crdt, room_id, batch_id, status_for(&e)));
        }
    }

    // 2. Commit. One transaction, one fsync.
    if let Err(e) = store.flush(target.workspace, target.document, now) {
        tracing::error!(
            room = %room_id,
            workspace = %target.workspace,
            document = %target.document,
            code = e.code().as_str(),
            detail = e.message(),
            "store update commit failed"
        );
        return Outcome::reply(ack(crdt, room_id, batch_id, status_for(&e)));
    }

    // 3 and 4. Only now.
    Outcome {
        reply: Some(ack(crdt, room_id.clone(), batch_id, UpdateStatusCode::Ok)),
        broadcast: Some(ProtocolMessage::DocUpdate {
            crdt,
            room_id,
            updates,
            batch_id,
        }),
        joined: None,
        left: None,
        durable_change: true,
    }
}

fn ack(
    crdt: CrdtType,
    room_id: String,
    ref_id: BatchId,
    status: UpdateStatusCode,
) -> ProtocolMessage {
    ProtocolMessage::Ack {
        crdt,
        room_id,
        ref_id,
        status,
    }
}

fn room_error(
    crdt: CrdtType,
    room_id: String,
    code: RoomErrorCode,
    message: &str,
) -> ProtocolMessage {
    ProtocolMessage::RoomError {
        crdt,
        room_id,
        code,
        message: message.into(),
    }
}

/// A store error, in the protocol's vocabulary.
///
/// The mapping is deliberately coarse: the protocol's statuses are about the
/// *update*, and Koru's codes are about the store. Anything the client could
/// have done differently is `InvalidUpdate`; anything it could not is
/// `Unknown`, which is honest rather than blaming the sender.
fn status_for(error: &Error) -> UpdateStatusCode {
    match error.code() {
        Code::CrdtRejected => UpdateStatusCode::InvalidUpdate,
        Code::PayloadTooLarge => UpdateStatusCode::PayloadTooLarge,
        Code::Forbidden | Code::ReadOnly | Code::SchemaTooNew => UpdateStatusCode::PermissionDenied,
        _ => UpdateStatusCode::Unknown,
    }
}

/// A version vector as the wire carries it.
///
/// An unreadable one is treated as **empty**, which asks for everything. That
/// is the safe direction: the alternative is sending a delta against a position
/// the client may not hold, which Loro parks as pending and silently never
/// applies.
fn decode_version(bytes: &[u8]) -> loro::VersionVector {
    if bytes.is_empty() {
        return loro::VersionVector::new();
    }
    loro::VersionVector::decode(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests;
