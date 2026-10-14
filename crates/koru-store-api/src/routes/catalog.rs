// SPDX-License-Identifier: MPL-2.0

//! The Store-owned workspace-catalog invalidation stream.
//!
//! This is deliberately not a Loro room and not the telemetry event journal.
//! It carries only the process-local generation as an eight-byte little-endian
//! binary frame. The number says only "list workspaces again". A newly
//! connected reader receives the current generation immediately, closing the
//! discover-before-register race without exposing a workspace id or any other
//! catalog content.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use koru_store_core::model::Scope;

use crate::auth::Caller;
use crate::error::ApiResult;
use crate::state::AppState;

pub async fn watch(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    caller.require(Scope::ReadDocuments)?;
    let changes = state.subscribe_catalog_changes();
    Ok(ws.on_upgrade(move |socket| serve(socket, changes)))
}

async fn serve(socket: WebSocket, mut changes: tokio::sync::watch::Receiver<u64>) {
    let (mut sink, mut stream) = socket.split();

    // Mark precisely the generation represented by the initial invalidation.
    // A commit after this borrow remains observable through `changed`, even if
    // it races the upgrade or the client's first catalog read.
    let generation = *changes.borrow_and_update();
    if sink
        .send(Message::Binary(generation.to_le_bytes().to_vec().into()))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    break;
                }
                let generation = *changes.borrow_and_update();
                if sink
                    .send(Message::Binary(generation.to_le_bytes().to_vec().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    // This stream has no client-to-Store data protocol.
                    // Unexpected frames carry no catalog meaning.
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}
