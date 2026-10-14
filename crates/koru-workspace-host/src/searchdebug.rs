// SPDX-License-Identifier: MPL-2.0

//! Privacy-safe search routing diagnostics, enabled only on demand.
//!
//! ```text
//! set KORU_SEARCH_DEBUG=1
//! ```
//!
//! Lines still reach stderr for a development console and are also written by
//! a worker to `%TEMP%/koru-search-debug-<pid>.log`. The event loop never does
//! file I/O: antivirus or a full disk must not turn diagnostics into the stall
//! being diagnosed.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};

const MAX_BYTES: u64 = 2 * 1024 * 1024;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_SEARCH_DEBUG))
}

pub fn write(args: fmt::Arguments<'_>) {
    if enabled() {
        let line = args.to_string();
        eprintln!("{line}");
        if let Some(sender) = sender() {
            let _ = sender.send(line);
        }
    }
}

fn sender() -> Option<&'static mpsc::Sender<String>> {
    static SENDER: OnceLock<Option<mpsc::Sender<String>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let path = log_path();
            let (sender, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .name("koru search diagnostics".into())
                .spawn(move || write_lines(&path, receiver))
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

fn log_path() -> PathBuf {
    std::env::temp_dir().join(format!("koru-search-debug-{}.log", std::process::id()))
}

fn write_lines(path: &Path, receiver: mpsc::Receiver<String>) {
    let mut writer = open(path);
    let mut bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
    while let Ok(line) = receiver.recv() {
        let needed = line.len() as u64 + 2;
        if bytes.saturating_add(needed) > MAX_BYTES {
            drop(writer.take());
            let previous = path.with_extension("previous.log");
            let _ = std::fs::remove_file(&previous);
            let _ = std::fs::rename(path, previous);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debug_variable_name_is_search_specific() {
        // Both copies of this switch read the same `koru_env` constant now,
        // so what is pinned is the constant's value.
        assert_eq!(koru_env::name::KORU_SEARCH_DEBUG, "KORU_SEARCH_DEBUG");
    }

    #[test]
    fn the_log_is_process_specific() {
        assert!(log_path().ends_with(format!("koru-search-debug-{}.log", std::process::id())));
    }
}
