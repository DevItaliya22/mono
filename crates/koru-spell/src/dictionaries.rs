// SPDX-License-Identifier: MPL-2.0

//! **Where the dictionaries live, and how they become answers.**
//!
//! Two types with one rule each:
//!
//! - [`DictionaryPaths`] is *where* — the `ModelPaths` shape from
//!   `koru-voice-models`: resolved once, `KORU_DICTIONARIES` first, then a
//!   `--root`, then the local-data default, and a [`decide`] that takes the
//!   world as arguments so the three cases are testable without `set_var`.
//! - [`DictionaryManager`] is *how* — load `.aff` + `.dic` once, cache the
//!   parsed dictionary behind an `Arc`, and answer [`DictionaryState`] without
//!   doing work.
//!
//! **Every failure fails open.** A language whose files are missing is
//! [`DictionaryState::NotInstalled`]; one whose files would not parse is
//! [`DictionaryState::Failed`] and stays failed for the session rather than
//! re-parsing on every keystroke's recheck. Neither is ever an error the
//! editor must handle to keep editing (spec §31).
//!
//! **Nothing here belongs on the thread that answers a person.** [`load`]
//! reads files and parses megabytes; the spell worker's thread is where it
//! runs. The manager's other methods only look at memory.
//!
//! [`decide`]: DictionaryPaths::decide
//! [`load`]: DictionaryManager::load

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::engine::SpellbookDictionary;
use crate::{SpellLang, SpellProvider};

/// The two file names inside a language's directory. Fixed names under a
/// per-tag directory (`en-US/dictionary.aff`), so the layout is the manifest
/// and no metadata file is needed to find the bytes.
const AFF: &str = "dictionary.aff";
const DIC: &str = "dictionary.dic";

/// Where the dictionary tree is, and every path inside it.
///
/// A resolved value rather than a function, so a manager that was pointed
/// somewhere cannot later ask the world again and get a different answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DictionaryPaths {
    root: PathBuf,
    /// The machine-wide tree a `--root` run falls back to, per language.
    ///
    /// **Dictionaries are application resources, not a Profile's data.** A
    /// `--root` isolates what a person writes; it has no reason to make a
    /// development run spell-check nothing because the dictionaries this
    /// machine already has sit in the ordinary place. The rooted tree still
    /// wins when it has the language, so a test can pin its own files.
    fallback: Option<PathBuf>,
}

impl DictionaryPaths {
    /// **The whole rule**, and it reads the world: `KORU_DICTIONARIES` first,
    /// then a `--root` if one was given, then the local-data default.
    ///
    /// `None` when there is no default to fall back to — a machine with
    /// neither `%LOCALAPPDATA%` nor `$HOME` — because guessing a directory is
    /// the failure `guide/contributing/PRINCIPLES.md` names by name, and because for spell
    /// checking specifically, "nowhere" degrades to "no underlines" rather
    /// than to an error.
    pub fn resolve(root: Option<&Path>) -> Option<DictionaryPaths> {
        DictionaryPaths::decide(
            koru_env::value(koru_env::name::KORU_DICTIONARIES).map(PathBuf::from),
            root,
            local_data_base(),
        )
    }

    /// The rule with the world passed in, so the three cases are testable
    /// without `set_var` — a process-global write that races every other test
    /// in the binary.
    pub fn decide(
        overridden: Option<PathBuf>,
        root: Option<&Path>,
        local_data: Option<PathBuf>,
    ) -> Option<DictionaryPaths> {
        let machine = local_data.map(|base| base.join("Koru").join("dictionaries"));
        if let Some(path) = overridden {
            return Some(DictionaryPaths {
                root: path,
                fallback: None,
            });
        }
        if let Some(root) = root {
            return Some(DictionaryPaths {
                root: root.join("dictionaries"),
                fallback: machine,
            });
        }
        machine.map(|root| DictionaryPaths {
            root,
            fallback: None,
        })
    }

    /// A dictionary tree somebody named exactly — a test's directory, or the
    /// value `KORU_DICTIONARIES` carried. Nothing is appended.
    pub fn at(root: impl Into<PathBuf>) -> DictionaryPaths {
        DictionaryPaths {
            root: root.into(),
            fallback: None,
        }
    }

    /// The tree's root; one directory per language tag lives under it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a download installs: the machine-wide tree when there is one,
    /// so every run on this machine — rooted or not — gets the language.
    pub fn install_root(&self) -> &Path {
        self.fallback.as_deref().unwrap_or(&self.root)
    }

    /// `<root>/<tag>/dictionary.aff` — the affix half.
    pub fn aff(&self, lang: SpellLang) -> PathBuf {
        self.pair(lang).0
    }

    /// `<root>/<tag>/dictionary.dic` — the word-list half.
    pub fn dic(&self, lang: SpellLang) -> PathBuf {
        self.pair(lang).1
    }

    /// Whether both halves are on disk in either tree.
    pub fn installed(&self, lang: SpellLang) -> bool {
        let (aff, dic) = self.pair(lang);
        aff.is_file() && dic.is_file()
    }

    /// Immutable downloaded pair. A directory rename publishes both files at
    /// once; staging directories are never reader candidates. Existing manual
    /// dictionaries retain priority, including an explicit root's overrides.
    pub fn download_dir(root: &Path, lang: SpellLang) -> PathBuf {
        root.join(lang.tag()).join(crate::sources::SOURCE_REF)
    }

    fn pair(&self, lang: SpellLang) -> (PathBuf, PathBuf) {
        self.pair_where(lang, |file| file.is_file())
    }

    fn pair_where(&self, lang: SpellLang, exists: impl Fn(&Path) -> bool) -> (PathBuf, PathBuf) {
        for root in std::iter::once(&self.root).chain(self.fallback.iter()) {
            for dir in [root.join(lang.tag()), Self::download_dir(root, lang)] {
                let pair = (dir.join(AFF), dir.join(DIC));
                if exists(&pair.0) && exists(&pair.1) {
                    return pair;
                }
            }
        }
        let dir = self.root.join(lang.tag());
        (dir.join(AFF), dir.join(DIC))
    }
}

/// What the manager knows about one language, without doing work to answer.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DictionaryState {
    /// No files on disk (or nobody has looked yet and the files are absent).
    NotInstalled,
    /// Files exist but no load has completed; a `load` is due or in flight.
    Loading,
    /// Loaded and answering.
    Ready,
    /// A load was tried and did not parse; it will not be retried this
    /// session — spell checking for this language is quietly off.
    Failed,
}

/// Why a load produced no dictionary. The message carries file names and
/// parser text, never document content — it is written to be loggable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpellError {
    pub message: String,
}

impl SpellError {
    fn new(message: impl Into<String>) -> SpellError {
        SpellError {
            message: message.into(),
        }
    }
}

enum Slot {
    Ready(Arc<dyn SpellProvider>),
    Failed,
}

/// Loads dictionaries once and hands out shared providers.
///
/// Cheap to share behind its own `Arc`; the lock guards only the map, never a
/// parse — the parse happens outside it so two languages can load at once and
/// a slow parse cannot block a `state()` question.
pub struct DictionaryManager {
    paths: DictionaryPaths,
    slots: Mutex<HashMap<SpellLang, Slot>>,
}

impl DictionaryManager {
    pub fn new(paths: DictionaryPaths) -> DictionaryManager {
        DictionaryManager {
            paths,
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub fn paths(&self) -> &DictionaryPaths {
        &self.paths
    }

    /// The already-loaded provider, or `None` — never any I/O, safe anywhere.
    pub fn get(&self, lang: SpellLang) -> Option<Arc<dyn SpellProvider>> {
        match self.slots.lock().unwrap().get(&lang) {
            Some(Slot::Ready(provider)) => Some(Arc::clone(provider)),
            _ => None,
        }
    }

    /// Where this language stands. Touches the disk only in the
    /// `NotInstalled`/`Loading` distinction (one metadata stat), so callers on
    /// an answering thread should prefer [`get`](DictionaryManager::get).
    pub fn state(&self, lang: SpellLang) -> DictionaryState {
        match self.slots.lock().unwrap().get(&lang) {
            Some(Slot::Ready(_)) => return DictionaryState::Ready,
            Some(Slot::Failed) => return DictionaryState::Failed,
            None => {}
        }
        if self.paths.installed(lang) {
            DictionaryState::Loading
        } else {
            DictionaryState::NotInstalled
        }
    }

    /// Load (or return the cached) dictionary. **Blocking**: reads and parses
    /// megabytes — worker-thread work, never the editor's.
    ///
    /// A parse failure is remembered as [`DictionaryState::Failed`] and not
    /// retried: the files will not become valid by being re-read every
    /// debounce, and the retry that matters — after a re-download — goes
    /// through [`reload`](DictionaryManager::reload).
    pub fn load(&self, lang: SpellLang) -> Result<Arc<dyn SpellProvider>, SpellError> {
        match self.slots.lock().unwrap().get(&lang) {
            Some(Slot::Ready(provider)) => return Ok(Arc::clone(provider)),
            Some(Slot::Failed) => {
                return Err(SpellError::new(format!(
                    "{} previously failed to load; not retrying",
                    lang.tag()
                )));
            }
            None => {}
        }
        let loaded = self.read_and_parse(lang);
        let mut slots = self.slots.lock().unwrap();
        match loaded {
            Ok(provider) => {
                let provider: Arc<dyn SpellProvider> = provider;
                slots.insert(lang, Slot::Ready(Arc::clone(&provider)));
                Ok(provider)
            }
            Err(error) => {
                slots.insert(lang, Slot::Failed);
                Err(error)
            }
        }
    }

    /// Forget a language and load it fresh — the path a dictionary update
    /// takes, and the only way out of `Failed`.
    pub fn reload(&self, lang: SpellLang) -> Result<Arc<dyn SpellProvider>, SpellError> {
        self.slots.lock().unwrap().remove(&lang);
        self.load(lang)
    }

    fn read_and_parse(&self, lang: SpellLang) -> Result<Arc<dyn SpellProvider>, SpellError> {
        // Resolve once: a pair installed between two independent lookups must
        // not combine halves from different roots or releases.
        let (aff_path, dic_path) = self.paths.pair(lang);
        let aff_bytes = std::fs::read(&aff_path)
            .map_err(|e| SpellError::new(format!("read {}: {e}", aff_path.display())))?;
        let dic_bytes = std::fs::read(&dic_path)
            .map_err(|e| SpellError::new(format!("read {}: {e}", dic_path.display())))?;
        let (aff, dic) = decode_pair(&aff_bytes, &dic_bytes)
            .map_err(|e| SpellError::new(format!("decode {}: {e}", lang.tag())))?;
        SpellbookDictionary::parse(&aff, &dic)
            .map(|dictionary| Arc::new(dictionary) as Arc<dyn SpellProvider>)
            .map_err(|e| SpellError::new(format!("parse {}: {e}", lang.tag())))
    }
}

/// Decode an `.aff`/`.dic` pair to UTF-8, honouring the `.aff`'s own `SET`
/// declaration — Hunspell predates UTF-8's victory, and a shipped dictionary
/// (German's, today) may still be Latin-1. The declaration line is rewritten
/// to `SET UTF-8` after transcoding, or the engine would re-read the new
/// bytes under the old name.
fn decode_pair(aff: &[u8], dic: &[u8]) -> Result<(String, String), String> {
    // The SET line is ASCII in every encoding this handles, so it can be
    // sniffed from the raw bytes.
    let declared = aff
        .split(|b| *b == b'\n')
        .find_map(|line| std::str::from_utf8(line).ok()?.trim().strip_prefix("SET "))
        .map(|token| token.trim().to_ascii_uppercase());
    match declared.as_deref() {
        None | Some("UTF-8" | "UTF8") => Ok((
            String::from_utf8(aff.to_vec()).map_err(|e| e.to_string())?,
            String::from_utf8(dic.to_vec()).map_err(|e| e.to_string())?,
        )),
        Some(latin @ ("ISO8859-1" | "ISO-8859-1" | "ISO8859-15" | "ISO-8859-15")) => {
            let euro = latin.ends_with("15");
            let decode =
                |bytes: &[u8]| -> String { bytes.iter().map(|b| latin1_char(*b, euro)).collect() };
            let aff = decode(aff).replace(&format!("SET {latin}"), "SET UTF-8");
            Ok((aff, decode(dic)))
        }
        Some(other) => Err(format!("unsupported dictionary encoding {other}")),
    }
}

/// One ISO 8859-1 byte as a char — the identity mapping — with the eight
/// ISO 8859-15 revisions applied when `euro`.
fn latin1_char(byte: u8, euro: bool) -> char {
    if euro {
        match byte {
            0xA4 => return '€',
            0xA6 => return 'Š',
            0xA8 => return 'š',
            0xB4 => return 'Ž',
            0xB8 => return 'ž',
            0xBC => return 'Œ',
            0xBD => return 'œ',
            0xBE => return 'Ÿ',
            _ => {}
        }
    }
    byte as char
}

/// `%LOCALAPPDATA%` on Windows, `~/Library/Application Support` on macOS,
/// `$XDG_DATA_HOME` (or `~/.local/share`) elsewhere — the same rule
/// `koru-voice-models` restates, for the same reason: this crate must not
/// know which channel it is, so it cannot borrow `koru_channel`'s copy.
fn local_data_base() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
    }
}

#[cfg(test)]
mod tests;
