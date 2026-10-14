// SPDX-License-Identifier: MPL-2.0

//! The Welcome page's ticks, kept machine-wide in the Store.
//!
//! **Machine-wide, not per Profile.** A tick says "this person on this
//! computer has done this", and every Profile on the installation shares it —
//! which is why it lives in the Store catalog's `store_state` table (one key
//! per row, `welcome.done.<row>`), reached through
//! `GET`/`PUT /v1/internal/welcome-progress`, rather than in a Profile
//! document or `Settings`.
//!
//! The adapter has `TaskCountsPanel`'s shape: the client arrives during
//! drawing and no request is made there; the maintenance turn does the work
//! on one-shot threads, never on the event thread. A window with no client
//! (every unit test) makes no request at all and keeps its ticks in memory.

use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// How soon to look again for an answer that is owed.
const POLL: Duration = Duration::from_millis(50);
/// How long a failed read waits before it is tried again.
const RETRY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct WelcomeProgress {
    client: Option<koru_api_client::Client>,
    done: BTreeSet<String>,
    /// The read has answered (or been given up on until `retry_at`).
    loaded: bool,
    retry_at: Option<Instant>,
    receiver: Option<Receiver<Result<Vec<String>, String>>>,
    /// Ticks made before a client arrived, written when one does.
    unwritten: Vec<&'static str>,
}

impl WelcomeProgress {
    /// Cache the store client supplied during drawing. **No request is made
    /// here.**
    pub fn set_client(&mut self, client: Option<koru_api_client::Client>) {
        self.client = client;
    }

    pub fn done(&self) -> impl Iterator<Item = &str> {
        self.done.iter().map(String::as_str)
    }

    /// Only while an answer is owed, or a first read is due.
    pub fn deadline(&self) -> Option<Instant> {
        self.client.as_ref()?;
        if self.receiver.is_some() || (!self.loaded && self.retry_at.is_none()) {
            return Some(Instant::now() + POLL);
        }
        if self.loaded {
            return None;
        }
        self.retry_at
    }

    /// Start the read when one is due and take its answer when it lands.
    /// Answers whether the set of ticks grew.
    pub fn maintain(&mut self, now: Instant) -> bool {
        let Some(client) = self.client.clone() else {
            return false;
        };
        for row in std::mem::take(&mut self.unwritten) {
            write(&client, row);
        }
        let mut changed = false;
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(Ok(rows)) => {
                    self.receiver = None;
                    self.loaded = true;
                    for row in rows {
                        changed |= self.done.insert(row);
                    }
                }
                Ok(Err(error)) => {
                    self.receiver = None;
                    self.retry_at = Some(now + RETRY);
                    eprintln!("[Koru][welcome] reading the Welcome ticks failed: {error}");
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.retry_at = Some(now + RETRY);
                }
            }
        }
        let due =
            !self.loaded && self.receiver.is_none() && self.retry_at.is_none_or(|at| at <= now);
        if due {
            let (tx, rx) = mpsc::channel();
            let rows = crate::welcome::RowId::persisted();
            let started = std::thread::Builder::new()
                .name("koru-welcome-progress".into())
                .spawn(move || {
                    let answer = client
                        .welcome_progress(&rows)
                        .map_err(|error| error.to_string());
                    let _ = tx.send(answer);
                });
            match started {
                Ok(_) => {
                    self.receiver = Some(rx);
                    self.retry_at = None;
                }
                Err(_) => self.retry_at = Some(now + RETRY),
            }
        }
        changed
    }

    /// Rows the page ticked. Kept here at once — so every Welcome tab in the
    /// window agrees before the Store answers — and written in the background.
    pub fn mark(&mut self, rows: Vec<&'static str>) -> bool {
        let mut changed = false;
        for row in rows {
            if !self.done.insert(row.to_owned()) {
                continue;
            }
            changed = true;
            match &self.client {
                Some(client) => write(client, row),
                None => self.unwritten.push(row),
            }
        }
        changed
    }
}

/// One fire-and-forget write. A failure costs a tick that has to be made again
/// next session, which is not worth a retry queue.
fn write(client: &koru_api_client::Client, row: &'static str) {
    let client = client.clone();
    let _ = std::thread::Builder::new()
        .name("koru-welcome-tick".into())
        .spawn(move || {
            if let Err(error) = client.mark_welcome_done(row) {
                eprintln!("[Koru][welcome] saving the {row} tick failed: {error}");
            }
        });
}

#[cfg(test)]
mod tests;
