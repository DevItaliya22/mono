// SPDX-License-Identifier: MPL-2.0

//! Attachment slash-command and pill-menu diagnostics, on demand.
//!
//! ```text
//! set KORU_ATTACH_DEBUG=1
//! ```
//!
//! Lines go to stderr (when a console is attached) and to
//! `%TEMP%/koru-attach-debug-<pid>.log`. No file bytes, full paths only as
//! basenames, no document body text.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};

const MAX_BYTES: u64 = 2 * 1024 * 1024;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_ATTACH_DEBUG))
}

pub fn write(args: fmt::Arguments<'_>) {
    if !enabled() {
        return;
    }
    let line = args.to_string();
    eprintln!("[koru:attach] {line}");
    if let Some(sender) = sender() {
        let _ = sender.send(line);
    }
}

/// Announce the log path once so a GUI run without a console still tells the
/// user where to look (first attach event only, via the log file itself).
pub fn path() -> Option<PathBuf> {
    enabled().then(log_path)
}

fn sender() -> Option<&'static mpsc::Sender<String>> {
    static SENDER: OnceLock<Option<mpsc::Sender<String>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let path = log_path();
            let (sender, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .name("koru attach diagnostics".into())
                .spawn(move || {
                    let banner = format!(
                        "KORU_ATTACH_DEBUG log started pid={} path={}",
                        std::process::id(),
                        path.display()
                    );
                    eprintln!("[koru:attach] {banner}");
                    write_lines(&path, receiver, banner);
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

fn log_path() -> PathBuf {
    std::env::temp_dir().join(format!("koru-attach-debug-{}.log", std::process::id()))
}

fn write_lines(path: &Path, receiver: mpsc::Receiver<String>, banner: String) {
    let mut writer = open(path);
    let mut bytes = 0u64;
    if let Some(file) = writer.as_mut()
        && writeln!(file, "{banner}")
            .and_then(|_| file.flush())
            .is_ok()
    {
        bytes = banner.len() as u64 + 2;
    }
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
    #[test]
    fn the_debug_variable_name_is_attach_specific() {
        assert_eq!(koru_env::name::KORU_ATTACH_DEBUG, "KORU_ATTACH_DEBUG");
    }
}
