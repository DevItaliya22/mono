// SPDX-License-Identifier: MPL-2.0

//! Persistent, privacy-safe routing diagnostics for the console-less agent.
//!
//! The event loop only formats and queues a line. A worker owns disk I/O, so a
//! slow antivirus scan or full disk can never sit in front of a search click.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct RoutingLog {
    sender: Option<mpsc::Sender<String>>,
    started: Instant,
    sequence: Arc<AtomicU64>,
}

impl RoutingLog {
    pub fn open(path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel();
        let live = std::thread::Builder::new()
            .name("koru-agent routing log".into())
            .spawn(move || write_lines(&path, receiver))
            .is_ok();
        Self {
            sender: live.then_some(sender),
            started: Instant::now(),
            sequence: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn disabled() -> Self {
        Self {
            sender: None,
            started: Instant::now(),
            sequence: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn event(&self, args: fmt::Arguments<'_>) {
        let Some(sender) = &self.sender else { return };
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let elapsed = self.started.elapsed().as_secs_f64();
        let _ = sender.send(format!(
            "[+ {elapsed:>9.3}s] route #{sequence} [pid={}] {args}",
            std::process::id()
        ));
    }
}

fn write_lines(path: &Path, receiver: mpsc::Receiver<String>) {
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut writer = open(path);
    let mut bytes = fs::metadata(path).map_or(0, |meta| meta.len());
    while let Ok(line) = receiver.recv() {
        let needed = line.len() as u64 + 2;
        if bytes.saturating_add(needed) > MAX_BYTES {
            drop(writer.take());
            let previous = path.with_extension("previous.log");
            let _ = fs::remove_file(&previous);
            let _ = fs::rename(path, previous);
            writer = open(path);
            bytes = 0;
        }
        if let Some(file) = writer.as_mut()
            && writeln!(file, "{line}").and_then(|_| file.flush()).is_ok()
        {
            bytes += needed;
        }
    }
}

fn open(path: &Path) -> Option<BufWriter<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
        .map(BufWriter::new)
}
