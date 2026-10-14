// SPDX-License-Identifier: MPL-2.0

//! **Download a missing dictionary, verified, off the thread that answers a
//! person.**
//!
//! Settings offers this when the language spell check would use has no files
//! on this device. The bytes come from `koru_spell::sources` — one pinned
//! release, one SHA-256 per file — and are installed only when both halves of
//! the pair, and every upstream README and license file the language carries
//! (`sources::notices`), arrived whole and matched: a private staging directory
//! is renamed once to publish them together, so no dictionary is ever on disk
//! without its license. Readers ignore staging, and concurrent
//! processes cannot overwrite each other's files. Then the spell worker is
//! asked to load the language again (it remembered it as missing), and every
//! open document re-checks.
//!
//! One download at a time: a second press while one runs is ignored. State is
//! a process global, read by Settings through [`state`] and [`version`], for
//! the reason the spelling preferences are (ADR-252): any Settings tab may be
//! the one showing it.

use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use koru_spell::sources::{self, SourceFile};
use koru_spell::{DictionaryPaths, SpellLang};
use sha2::{Digest, Sha256};

/// Where a download stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Running {
        lang: SpellLang,
        received: u64,
        total: u64,
    },
    /// The last attempt failed; the reason is loggable (no document text).
    Failed {
        lang: SpellLang,
        reason: String,
    },
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
/// Bumped on every change, so a view re-reads only when something moved.
static VERSION: AtomicU64 = AtomicU64::new(0);

pub fn state() -> State {
    STATE.lock().unwrap().clone()
}

pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

fn set(next: State) {
    *STATE.lock().unwrap() = next;
    VERSION.fetch_add(1, Ordering::Relaxed);
    super::wake();
}

/// Start downloading `lang`. Returns `false` when one is already running.
pub fn start(lang: SpellLang) -> bool {
    {
        let mut state = STATE.lock().unwrap();
        if matches!(*state, State::Running { .. }) {
            return false;
        }
        *state = State::Running {
            lang,
            received: 0,
            total: sources::download_bytes(lang),
        };
    }
    VERSION.fetch_add(1, Ordering::Relaxed);
    let root = crate::cli::root_from_args();
    let spawned = std::thread::Builder::new()
        .name("koru-dictionary-download".into())
        .spawn(move || {
            let result = DictionaryPaths::resolve(root.as_deref())
                .ok_or_else(|| "this machine has no local data folder".to_owned())
                .and_then(|paths| install(lang, paths.install_root()));
            match result {
                Ok(()) => {
                    tracing::info!(target: "koru::spell", lang = lang.tag(), "dictionary installed");
                    super::reload(lang);
                    set(State::Idle);
                }
                Err(reason) => {
                    tracing::warn!(target: "koru::spell", lang = lang.tag(), %reason, "dictionary download failed");
                    set(State::Failed { lang, reason });
                }
            }
        });
    if spawned.is_err() {
        set(State::Failed {
            lang,
            reason: "could not start the download".to_owned(),
        });
    }
    true
}

/// Fetch into an isolated sibling directory, then publish the verified pair
/// with one rename. A crash leaves only an unreferenced staging directory;
/// retrying never requires deleting or repairing the active dictionary.
fn install(lang: SpellLang, root: &Path) -> Result<(), String> {
    let dir = root.join(lang.tag());
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let stage = dir.join(format!(".download-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&stage).map_err(|e| format!("create {}: {e}", stage.display()))?;
    let result = install_staged(lang, root, &stage);
    // Only this operation's directory, never another process's staging or
    // installed data. On success the rename has already removed this path.
    let _ = std::fs::remove_dir_all(&stage);
    result
}

fn install_staged(lang: SpellLang, root: &Path, stage: &Path) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(10 * 60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || attempt.url().scheme() != "https" {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| e.to_string())?;
    let files = sources::download_files(lang);
    let mut received = 0;
    for file in &files {
        fetch(
            &client,
            file,
            &stage.join(file.installed_as),
            lang,
            &mut received,
        )?;
    }
    let target = DictionaryPaths::download_dir(root, lang);
    if let Err(error) = std::fs::rename(stage, &target) {
        // Another process may have won. Accept only the exact verified set,
        // never just an existing directory, one half of a dictionary, or a
        // pair without its notices.
        for file in &files {
            let bytes = std::fs::read(target.join(file.installed_as))
                .map_err(|_| format!("install {}: {error}", target.display()))?;
            if bytes.len() as u64 != file.bytes
                || format!("{:x}", Sha256::digest(&bytes)) != file.sha256
            {
                return Err(format!("installed dictionary does not match {}", file.path));
            }
        }
    }
    Ok(())
}

fn fetch(
    client: &reqwest::blocking::Client,
    file: &SourceFile,
    part: &Path,
    lang: SpellLang,
    received: &mut u64,
) -> Result<(), String> {
    let mut response = client
        .get(file.url())
        .send()
        .map_err(|e| format!("{}: {e}", file.path))?;
    if !response.status().is_success() {
        return Err(format!("{}: HTTP {}", file.path, response.status()));
    }
    let mut out =
        std::fs::File::create(part).map_err(|e| format!("write {}: {e}", part.display()))?;
    let mut hash = Sha256::new();
    let mut written: u64 = 0;
    let mut buffer = vec![0u8; 64 * 1024];
    let total = sources::download_bytes(lang);
    loop {
        let n = response
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", file.path))?;
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > file.bytes {
            return Err(format!("{}: larger than expected", file.path));
        }
        hash.update(&buffer[..n]);
        std::io::Write::write_all(&mut out, &buffer[..n])
            .map_err(|e| format!("write {}: {e}", part.display()))?;
        *received += n as u64;
        *STATE.lock().unwrap() = State::Running {
            lang,
            received: *received,
            total,
        };
        // A repaint per quarter-megabyte, not per chunk: progress is a
        // number on a row, and a wake costs every view a maintain pass.
        const STEP: u64 = 256 * 1024;
        if *received / STEP != (*received - n as u64) / STEP {
            VERSION.fetch_add(1, Ordering::Relaxed);
            super::wake();
        }
    }
    if written != file.bytes {
        return Err(format!(
            "{}: {written} of {} bytes arrived",
            file.path, file.bytes
        ));
    }
    let digest = format!("{:x}", hash.finalize());
    if digest != file.sha256 {
        return Err(format!("{}: checksum does not match", file.path));
    }
    std::io::Write::flush(&mut out).map_err(|e| e.to_string())?;
    out.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}
