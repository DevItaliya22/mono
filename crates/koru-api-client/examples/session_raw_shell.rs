// SPDX-License-Identifier: MPL-2.0

//! Rooted public-session smoke test: create, stream, write, and terminate.

mod support;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use koru_api_client::sessions::{
    IdempotencyKey, RowIndex, RowRange, SessionStreamItem, SnapshotRequest, ViewportGeneration,
    WriteTextRequest,
};

const MARKER: &str = "PUBLIC_SESSION_SMOKE_OK";

fn main() -> support::ExampleResult<()> {
    let support::Args { root, image } = support::args(false)?;
    debug_assert!(image.is_none());
    let client = support::connect(&root)?;
    let (session_id, lease) = support::create_and_attach(&client)?;
    let operation = run(&client, session_id, lease);
    let cleanup = support::terminate(&client, session_id, lease);
    support::finish(operation, cleanup)
}

fn run(
    client: &koru_api_client::Client,
    session_id: koru_api_client::sessions::SessionId,
    lease: koru_api_client::sessions::ControllerLeaseToken,
) -> support::ExampleResult<()> {
    let newline = if cfg!(windows) { "\r\n" } else { "\n" };
    for attempt in 1..=4 {
        let viewport_generation = ViewportGeneration::new(attempt)
            .map_err(|code| std::io::Error::other(code.to_string()))?;
        let observed = client.sessions().snapshot(
            session_id,
            &SnapshotRequest {
                viewport_generation,
                range: RowRange {
                    first: RowIndex(0),
                    count: 24,
                },
                columns: 80,
            },
        )?;
        let (sender, receiver) = mpsc::sync_channel(32);
        let subscription =
            client
                .sessions()
                .subscribe(session_id, observed.resume, move |item| {
                    let _ = sender.try_send(item);
                })?;
        client.sessions().write_text(
            session_id,
            &WriteTextRequest {
                idempotency_key: IdempotencyKey::mint(),
                controller_lease: lease,
                text: format!("echo {MARKER}{newline}"),
            },
        )?;

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut retry_after_gap = false;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining.min(Duration::from_millis(250))) {
                Ok(Ok(SessionStreamItem::Output { chunk }))
                    if String::from_utf8_lossy(&chunk.bytes).contains(MARKER) =>
                {
                    drop(subscription);
                    println!("public rooted session streamed {MARKER}");
                    return Ok(());
                }
                Ok(Ok(SessionStreamItem::Gap { .. })) => {
                    retry_after_gap = true;
                    break;
                }
                Ok(Err(error)) => return Err(error.into()),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        subscription.close();
        if !retry_after_gap {
            return Err("session stream ended before output or an explicit gap".into());
        }
    }
    Err("session remained too active to establish a gap-free smoke stream".into())
}
