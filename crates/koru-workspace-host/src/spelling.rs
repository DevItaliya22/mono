// SPDX-License-Identifier: MPL-2.0

//! Spell checking off the event loop.
//!
//! The `ImageLoader` shape with one twist: results do not land in a shared
//! cache, they land on the view that asked. Each view holds a [`SpellClient`]
//! — its own reply channel plus a sender into the one worker — so two windows
//! showing the same document each get their own answer, and routing is the
//! channel itself rather than a token table (spell-check plan §4).
//!
//! The worker owns the `DictionaryManager`; dictionary parsing (tens to
//! hundreds of milliseconds per language) and every check happen on the
//! `koru-spell-worker` thread. A completed batch wakes the loop once with
//! [`Wake::Spelling`]; the wake carries no work — `about_to_wait` already
//! runs every view's `maintain`, and draining the reply channel there is one
//! empty `try_recv` on an idle pass, which is the bar `about_to_wait` sets.
//!
//! **The handle is a process global**, the `voice_prefs` route (ADR-252): a
//! view cannot reach the `App`, views are rebuilt wholesale on `reseat`, and
//! threading a handle through four layers of constructors would be state the
//! global already is. `install` is called once from `App::new`; a process
//! with no worker (a test) gets `None` from [`client`] and spell checking is
//! quietly off — the same fail-open answer as a missing dictionary.
//!
//! Requests coalesce per `(client, node, cell)`: a burst of rechecks for one
//! paragraph collapses to the newest revision, the `koru-search-ui` reader's
//! newest-request rule.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{OnceLock, RwLock};

use koru_spell::{DictionaryManager, DictionaryPaths, tokenizer};
// Views name their language through this module, not through the engine crate:
// the boundary the plan seals (D1) has one door.
use koru_core::NodeId;
pub use koru_spell::SpellLang;
pub use koru_spell::sources;
use winit::event_loop::EventLoopProxy;

use crate::app::Wake;

/// One block (or one table cell) to check, at one revision.
pub struct SpellRequest {
    pub node: NodeId,
    /// `Some((row, column))` when `text` is one rendered table cell's text.
    pub cell: Option<(usize, usize)>,
    /// `VersionVector::encode()` at extraction time — compared by the
    /// controller on arrival, never ordered.
    pub revision: Vec<u8>,
    pub lang: SpellLang,
    pub text: String,
    /// Byte ranges over `text` the tokenizer must not look inside.
    pub exclusions: Vec<Range<usize>>,
}

/// The misspelled ranges of one checked request, echoing its identity.
pub struct SpellResult {
    pub node: NodeId,
    pub cell: Option<(usize, usize)>,
    pub revision: Vec<u8>,
    pub issues: Vec<Range<usize>>,
}

/// What comes back over a client's reply channel.
pub enum SpellReply {
    Checked(SpellResult),
    /// Replacement spellings for one word, best first — asked for lazily when
    /// a menu opens (plan D13), never alongside a check.
    Suggestions {
        word: String,
        suggestions: Vec<String>,
    },
}

enum Work {
    Check(SpellRequest),
    Suggest {
        lang: SpellLang,
        word: String,
    },
    /// A dictionary was just installed: forget what the manager remembered
    /// about the language (missing, or failed) and load it.
    Reload(SpellLang),
}

struct Job {
    client: u64,
    work: Work,
    reply: mpsc::Sender<SpellReply>,
}

static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
/// The event loop's wake, kept by [`spawn`] so work finishing off-thread —
/// a dictionary download — can ask for a pass.
static PROXY: OnceLock<EventLoopProxy<Wake>> = OnceLock::new();

pub mod download;

fn wake() {
    if let Some(proxy) = PROXY.get() {
        let _ = proxy.send_event(Wake::Spelling);
    }
}

/// Ask the worker to load `lang` afresh, then have every view re-check.
fn reload(lang: SpellLang) {
    if let Some(tx) = WORKER.get() {
        let (reply, _rx) = mpsc::channel();
        let _ = tx.send(Job {
            client: u64::MAX,
            work: Work::Reload(lang),
            reply,
        });
    }
}
static CLIENT_IDS: AtomicU64 = AtomicU64::new(0);

/// A view's private line to the spell worker. Cheap to make; dropped with the
/// view, at which point in-flight replies go nowhere and are freed.
pub struct SpellClient {
    id: u64,
    tx: mpsc::Sender<Job>,
    reply: mpsc::Sender<SpellReply>,
    rx: mpsc::Receiver<SpellReply>,
}

impl SpellClient {
    /// Queue one check. Never blocks; a dead worker means the request is
    /// dropped and no underline appears — fail open.
    pub fn enqueue(&self, request: SpellRequest) {
        let _ = self.tx.send(Job {
            client: self.id,
            work: Work::Check(request),
            reply: self.reply.clone(),
        });
    }

    /// Queue one suggestion lookup — the lazy half of the engine (D13).
    pub fn suggest(&self, lang: SpellLang, word: String) {
        let _ = self.tx.send(Job {
            client: self.id,
            work: Work::Suggest { lang, word },
            reply: self.reply.clone(),
        });
    }

    /// Every reply that has arrived. One empty `try_recv` when idle.
    pub fn drain(&self) -> Vec<SpellReply> {
        self.rx.try_iter().collect()
    }
}

/// A client for the installed worker, or `None` when no worker runs in this
/// process (a test, or a host that never called [`spawn`]).
pub fn client() -> Option<SpellClient> {
    WORKER.get().map(|tx| {
        let (reply, rx) = mpsc::channel();
        SpellClient {
            id: CLIENT_IDS.fetch_add(1, Ordering::Relaxed),
            tx: tx.clone(),
            reply,
            rx,
        }
    })
}

// ------------------------------------------------------------- preferences
//
// The two spelling preferences that follow the person, mirrored here as
// atomics for ADR-252's reason: a view cannot reach `Settings`, and two
// Settings tabs are two views of one preference. `Settings` remains the
// record; `App::new` seeds these, the Settings surface writes here, and
// `capture` reads them back into what saves. The *profile's* locale — the
// resolved half of "automatic" — arrives from the dictionary thread, which is
// the one place that already reads the profile record.

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// `0` is automatic; a fixed language is its index in [`SpellLang::ALL`] + 1.
static PREFERRED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// What "automatic" resolves to: the profile locale's spell language, same
/// encoding. `0` until the profile record has been read — quietly off (D6).
static PROFILE_LANG: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// Bumped on any preference change so controllers re-resolve in `maintain`.
static PREFS_VERSION: AtomicU64 = AtomicU64::new(1);

/// The stored token for "resolve from the profile locale".
pub const AUTO: &str = "auto";

fn lang_index(lang: Option<SpellLang>) -> u8 {
    lang.and_then(|lang| SpellLang::ALL.iter().position(|l| *l == lang))
        .map_or(0, |i| i as u8 + 1)
}

fn lang_at(index: u8) -> Option<SpellLang> {
    SpellLang::ALL
        .get(usize::from(index.checked_sub(1)?))
        .copied()
}

pub fn prefs_version() -> u64 {
    PREFS_VERSION.load(Ordering::Acquire)
}

fn bump_prefs() {
    PREFS_VERSION.fetch_add(1, Ordering::AcqRel);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::Relaxed) != on {
        bump_prefs();
    }
}

/// The explicit preference, `None` meaning automatic.
pub fn preferred() -> Option<SpellLang> {
    lang_at(PREFERRED.load(Ordering::Relaxed))
}

pub fn set_preferred(lang: Option<SpellLang>) {
    let index = lang_index(lang);
    if PREFERRED.swap(index, Ordering::Relaxed) != index {
        bump_prefs();
    }
}

/// What automatic resolves to — the profile locale's spell language, if any.
pub fn profile_language() -> Option<SpellLang> {
    lang_at(PROFILE_LANG.load(Ordering::Relaxed))
}

pub fn set_profile_language(lang: Option<SpellLang>) {
    let index = lang_index(lang);
    if PROFILE_LANG.swap(index, Ordering::Relaxed) != index {
        bump_prefs();
    }
}

/// The language checks run in right now: off → `None`; otherwise the explicit
/// preference, else the profile locale's language, else **en-US** — D6's
/// quietly-off was amended at acceptance: a profile whose locale names no
/// dictionary still gets English checking by default, and the Settings toggle
/// remains the way out. A per-document override outranks all of this at the
/// view, which is the one place that can see the document.
pub fn effective_language() -> Option<SpellLang> {
    if !enabled() {
        return None;
    }
    preferred()
        .or_else(profile_language)
        .or(Some(SpellLang::EnUs))
}

/// Whether both Hunspell files for the language selected on this installation
/// exist. This is a lightweight Settings diagnostic; parsing remains on the
/// spelling worker.
pub fn dictionary_files_available(lang: SpellLang) -> bool {
    DictionaryPaths::resolve(crate::cli::root_from_args().as_deref())
        .is_some_and(|paths| paths.installed(lang))
}

/// Parse a stored preference token: `auto`/empty → `None`, else the tag if it
/// names a supported dictionary, else `None` (an unrecognized tag behaves as
/// automatic, the `dictation_language` rule).
pub fn parse_language(token: &str) -> Option<SpellLang> {
    let token = token.trim();
    if token.is_empty() || token.eq_ignore_ascii_case(AUTO) {
        return None;
    }
    SpellLang::ALL
        .into_iter()
        .find(|lang| lang.tag().eq_ignore_ascii_case(token))
}

/// The token a preference persists as.
pub fn language_token(lang: Option<SpellLang>) -> &'static str {
    lang.map_or(AUTO, SpellLang::tag)
}

// ---------------------------------------------------------------- overlays
//
// The custom-word overlays (spec §22): the personal dictionary, shared by
// every view and consulted by the worker *before* the engine, and the
// per-document ignores, consulted by each view's controller. Both are process
// globals beside the worker handle, fed by the dictionary thread below, and
// every change bumps one version so controllers notice in their next
// `maintain` without being individually told.

static PERSONAL: RwLock<Option<HashMap<SpellLang, HashSet<String>>>> = RwLock::new(None);
static DOC_IGNORES: RwLock<Option<HashMap<String, HashSet<String>>>> = RwLock::new(None);
/// The current workspace's shared dictionary (D3) — jargon every collaborator
/// accepts. Replaced wholesale when the workspace's system document changes
/// or the workspace itself switches.
static WORKSPACE_WORDS: RwLock<Option<HashMap<SpellLang, HashSet<String>>>> = RwLock::new(None);
static OVERLAY_VERSION: AtomicU64 = AtomicU64::new(1);

/// Bumped on every overlay change; controllers compare it in `maintain`.
pub fn overlay_version() -> u64 {
    OVERLAY_VERSION.load(Ordering::Acquire)
}

fn bump_overlays() {
    OVERLAY_VERSION.fetch_add(1, Ordering::AcqRel);
}

/// Whether the personal dictionary holds `word` for `lang`. Case-insensitive:
/// somebody who taught it `Loro` is not asking to be corrected on `loro`.
pub fn personal_contains(lang: SpellLang, word_lower: &str) -> bool {
    PERSONAL
        .read()
        .ok()
        .and_then(|map| {
            map.as_ref()
                .map(|map| map.get(&lang).is_some_and(|set| set.contains(word_lower)))
        })
        .unwrap_or(false)
}

/// How many words the personal dictionary holds, across its languages — a
/// word taught in two languages counts once. `None` until the dictionary has
/// been read.
pub fn personal_word_count() -> Option<usize> {
    let map = PERSONAL.read().ok()?;
    let map = map.as_ref()?;
    let mut words: HashSet<&str> = HashSet::new();
    for set in map.values() {
        words.extend(set.iter().map(String::as_str));
    }
    Some(words.len())
}

/// Whether the current workspace's shared dictionary holds `word` for `lang`.
pub fn workspace_contains(lang: SpellLang, word_lower: &str) -> bool {
    WORKSPACE_WORDS
        .read()
        .ok()
        .and_then(|map| {
            map.as_ref()
                .map(|map| map.get(&lang).is_some_and(|set| set.contains(word_lower)))
        })
        .unwrap_or(false)
}

/// Replace the workspace dictionary overlay — called by the shell whenever
/// the workspace's system document changed (a collaborator's add arrives the
/// same way this window's own does: through sync).
pub fn set_workspace_words(words: std::collections::BTreeMap<String, Vec<String>>) {
    let mut map: HashMap<SpellLang, HashSet<String>> = HashMap::new();
    for (tag, list) in words {
        let Some(lang) = SpellLang::ALL
            .into_iter()
            .find(|l| l.tag().eq_ignore_ascii_case(&tag))
        else {
            continue;
        };
        map.entry(lang)
            .or_default()
            .extend(list.into_iter().map(|w| w.to_lowercase()));
    }
    if let Ok(mut overlay) = WORKSPACE_WORDS.write() {
        *overlay = Some(map);
    }
    bump_overlays();
}

/// Whether `word` is ignored for this document (`entity`) in `lang`.
pub fn document_ignored(entity: &str, lang: SpellLang, word_lower: &str) -> bool {
    DOC_IGNORES
        .read()
        .ok()
        .and_then(|map| {
            map.as_ref().map(|map| {
                map.get(entity)
                    .is_some_and(|set| set.contains(&ignore_token(lang, word_lower)))
            })
        })
        .unwrap_or(false)
}

fn ignore_token(lang: SpellLang, word_lower: &str) -> String {
    format!("{}|{}", lang.tag(), word_lower)
}

/// The stored attribute key for one ignored word. Attribute keys are ASCII
/// (`koru.plan`-shaped), so the word travels hex-encoded — deterministic and
/// reversible, which is what removal and enumeration need.
fn ignore_attribute_key(lang: SpellLang, word_lower: &str) -> String {
    let mut hex = String::with_capacity(word_lower.len() * 2);
    for byte in word_lower.bytes() {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    format!("koru.spell.ignore.{}.{}", lang.tag(), hex)
}

fn parse_ignore_attribute_key(key: &str) -> Option<String> {
    let rest = key.strip_prefix("koru.spell.ignore.")?;
    let (lang_tag, hex) = rest.rsplit_once('.')?;
    let lang = SpellLang::ALL
        .into_iter()
        .find(|l| l.tag().eq_ignore_ascii_case(lang_tag))?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    let word = String::from_utf8(bytes?).ok()?;
    Some(ignore_token(lang, &word))
}

/// What the dictionary thread is asked to do. Every job ends by updating the
/// overlays and waking the loop, so the store round-trip is invisible to the
/// thread that answers a person.
pub enum SpellEdit {
    /// Add one word to the personal dictionary — valid everywhere, roams
    /// across this person's devices.
    AddPersonal { lang: SpellLang, word: String },
    /// Clear the current Profile's personal dictionary in one Store commit.
    ClearPersonal,
    /// Ignore one word in one document, for this person only.
    IgnoreInDocument {
        entity: String,
        lang: SpellLang,
        word: String,
    },
}

enum DictJob {
    SwitchProfile(koru_store_core::ids::ProfileId),
    Load,
    Reload(koru_store_core::ids::ProfileId),
    EnsureIgnores { entity: String },
    Edit(SpellEdit),
}

static DICT_TX: OnceLock<mpsc::Sender<DictJob>> = OnceLock::new();
static DICT_PROFILE: RwLock<Option<koru_store_core::ids::ProfileId>> = RwLock::new(None);

/// Whether the dictionary thread is running (the app has a profile).
pub fn dictionary_connected() -> bool {
    DICT_TX.get().is_some()
}

/// Whether the dictionary worker is bound to this exact Profile.
///
/// Profile switching keeps the process alive, so "a worker exists" is not
/// enough: its personal words and document ignores must follow the active
/// Profile rather than leaking the previous Profile's overlays into the new
/// one.
pub fn dictionary_connected_to(profile: koru_store_core::ids::ProfileId) -> bool {
    dictionary_connected()
        && DICT_PROFILE
            .read()
            .is_ok_and(|active| active.as_ref() == Some(&profile))
}

/// Apply one custom-word edit. **Optimistic**: the overlays update now — the
/// squiggle clears under the click — and the store write follows on the
/// dictionary thread. A failed write is logged and the optimistic state
/// stands for this session: losing an underline is the fail-open direction.
pub fn edit(edit: SpellEdit) {
    match &edit {
        SpellEdit::AddPersonal { lang, word } => {
            if let Ok(mut map) = PERSONAL.write() {
                map.get_or_insert_with(HashMap::new)
                    .entry(*lang)
                    .or_default()
                    .insert(word.to_lowercase());
            }
        }
        SpellEdit::ClearPersonal => {
            if let Ok(mut map) = PERSONAL.write() {
                *map = Some(HashMap::new());
            }
        }
        SpellEdit::IgnoreInDocument { entity, lang, word } => {
            if let Ok(mut map) = DOC_IGNORES.write() {
                map.get_or_insert_with(HashMap::new)
                    .entry(entity.clone())
                    .or_default()
                    .insert(ignore_token(*lang, &word.to_lowercase()));
            }
        }
    }
    bump_overlays();
    if let Some(tx) = DICT_TX.get() {
        let _ = tx.send(DictJob::Edit(edit));
    }
}

/// Make sure this document's ignore list has been fetched. Cheap to call on
/// every document open; the thread fetches each entity once per session.
pub fn ensure_document_ignores(entity: &str) {
    let known = DOC_IGNORES
        .read()
        .is_ok_and(|map| map.as_ref().is_some_and(|map| map.contains_key(entity)));
    if known {
        return;
    }
    if let Some(tx) = DICT_TX.get() {
        let _ = tx.send(DictJob::EnsureIgnores {
            entity: entity.to_owned(),
        });
    }
}

/// Start the dictionary thread once the profile is known. `connect` builds
/// the store client on the worker thread — never on the caller's.
pub fn connect_dictionary(
    proxy: &EventLoopProxy<Wake>,
    profile: koru_store_core::ids::ProfileId,
    connect: impl Fn() -> Result<koru_api_client::Client, koru_store_core::errors::Error>
    + Send
    + 'static,
) {
    let (tx, jobs) = mpsc::channel::<DictJob>();
    let event_tx = tx.clone();
    if let Some(active) = DICT_TX.get() {
        let changed = DICT_PROFILE.write().is_ok_and(|mut current| {
            if current.as_ref() == Some(&profile) {
                false
            } else {
                *current = Some(profile);
                true
            }
        });
        if changed {
            if let Ok(mut personal) = PERSONAL.write() {
                *personal = None;
            }
            if let Ok(mut ignores) = DOC_IGNORES.write() {
                *ignores = None;
            }
            set_profile_language(None);
            bump_overlays();
            let _ = active.send(DictJob::SwitchProfile(profile));
            let _ = active.send(DictJob::Load);
        }
        return;
    }
    if let Ok(mut current) = DICT_PROFILE.write() {
        *current = Some(profile);
    }
    if DICT_TX.set(tx).is_err() {
        return;
    }
    let proxy = proxy.clone();
    let _ = DICT_TX.get().map(|tx| tx.send(DictJob::Load));
    let mut profile = profile;
    let _ = std::thread::Builder::new()
        .name("koru-spell-dictionary".into())
        .spawn(move || {
            let mut client: Option<koru_api_client::Client> = None;
            let mut dictionary_events = None;
            while let Ok(job) = jobs.recv() {
                if client.is_none() {
                    client = connect().ok();
                }
                let Some(active) = client.as_ref() else {
                    // No store: the optimistic overlay state stands for the
                    // session and the write is lost — logged, fail open.
                    tracing::warn!(target: "koru::spell", "dictionary write dropped: no store connection");
                    continue;
                };
                if dictionary_events.is_none() && active.events().available() {
                    let reload = event_tx.clone();
                    dictionary_events = active
                        .events()
                        .on(
                            None,
                            koru_api_client::Filter::names("profile.dictionary."),
                            move |event| {
                                if let Some(profile) = dictionary_change_profile(
                                    event.envelope.name.as_str(),
                                    event.envelope.scope.profile_id.map(|id| {
                                        koru_store_core::ProfileId::from_uuid(id.0)
                                    }),
                                ) {
                                    let _ = reload.send(DictJob::Reload(profile));
                                }
                            },
                        )
                        .ok();
                }
                let outcome = match job {
                    DictJob::SwitchProfile(next) => {
                        profile = next;
                        Ok(())
                    }
                    DictJob::Load => {
                        // The profile's locale is what "automatic" resolves
                        // to (ADR-87: the person's real tag, never the shown
                        // UI language) — read here because this thread
                        // already reads the profile's records.
                        if let Ok(profiles) = active.profiles() {
                            let locale = profiles
                                .iter()
                                .find(|p| p.id == profile)
                                .map(|p| p.locale.as_str().to_owned());
                            set_profile_language(
                                locale
                                    .as_deref()
                                    .and_then(koru_spell::SpellLang::from_locale_tag),
                            );
                            let _ = proxy.send_event(Wake::Spelling);
                        }
                        active.personal_dictionary(profile).map(|response| {
                            install_personal_words(response.words);
                            let _ = proxy.send_event(Wake::Spelling);
                        })
                    }
                    DictJob::Reload(changed) => {
                        if changed != profile {
                            Ok(())
                        } else {
                            active.personal_dictionary(profile).map(|response| {
                                install_personal_words(response.words);
                                let _ = proxy.send_event(Wake::Spelling);
                            })
                        }
                    }
                    DictJob::EnsureIgnores { entity } => {
                        active.personal_attributes(profile, &entity).map(|response| {
                            let words: HashSet<String> = response
                                .attributes
                                .keys()
                                .filter_map(|key| parse_ignore_attribute_key(key))
                                .collect();
                            if let Ok(mut overlay) = DOC_IGNORES.write() {
                                overlay
                                    .get_or_insert_with(HashMap::new)
                                    .entry(entity)
                                    .or_default()
                                    .extend(words);
                            }
                            bump_overlays();
                            let _ = proxy.send_event(Wake::Spelling);
                        })
                    }
                    DictJob::Edit(SpellEdit::AddPersonal { lang, word }) => active
                        .set_dictionary_word(profile, lang.tag(), &word, true)
                        .map(|_| ()),
                    DictJob::Edit(SpellEdit::ClearPersonal) => active
                        .clear_personal_dictionary(profile)
                        .map(|_| ()),
                    DictJob::Edit(SpellEdit::IgnoreInDocument { entity, lang, word }) => active
                        .set_personal_attribute(
                            profile,
                            &entity,
                            &ignore_attribute_key(lang, &word.to_lowercase()),
                            Some(
                                koru_store_core::personal_attributes::PersonalAttributeValue::Bool {
                                    value: true,
                                },
                            ),
                        )
                        .map(|_| ()),
                };
                if let Err(error) = outcome {
                    // A Store restart invalidates sessions; reconnect on the
                    // next job. Never the word itself in the log.
                    tracing::warn!(target: "koru::spell", %error, "dictionary operation failed");
                    client = None;
                }
            }
        });
}

fn install_personal_words(words: std::collections::BTreeMap<String, Vec<String>>) {
    let mut map: HashMap<SpellLang, HashSet<String>> = HashMap::new();
    for (tag, words) in words {
        let Some(lang) = SpellLang::ALL
            .into_iter()
            .find(|lang| lang.tag().eq_ignore_ascii_case(&tag))
        else {
            continue;
        };
        map.entry(lang)
            .or_default()
            .extend(words.into_iter().map(|word| word.to_lowercase()));
    }
    if let Ok(mut overlay) = PERSONAL.write() {
        *overlay = Some(map);
    }
    bump_overlays();
}

fn dictionary_change_profile(
    name: &str,
    profile: Option<koru_store_core::ProfileId>,
) -> Option<koru_store_core::ProfileId> {
    (name == "profile.dictionary.changed")
        .then_some(profile)
        .flatten()
}

#[cfg(test)]
mod dictionary_event_tests {
    use super::*;

    #[test]
    fn only_a_profile_scoped_dictionary_fact_requests_reload() {
        let profile = koru_store_core::ProfileId::new();
        assert_eq!(
            dictionary_change_profile("profile.dictionary.changed", Some(profile)),
            Some(profile)
        );
        assert_eq!(
            dictionary_change_profile("profile.dictionary.changed", None),
            None
        );
        assert_eq!(
            dictionary_change_profile("reminder.settings.changed", Some(profile)),
            None
        );
    }
}

/// Start the worker and install the process-global handle. Called once from
/// `App::new`; a second call is a no-op (the first worker keeps running).
pub fn spawn(proxy: &EventLoopProxy<Wake>, root: Option<std::path::PathBuf>) {
    let (tx, jobs) = mpsc::channel::<Job>();
    if WORKER.set(tx).is_err() {
        return;
    }
    let proxy = proxy.clone();
    let _ = PROXY.set(proxy.clone());
    let _ = std::thread::Builder::new()
        .name("koru-spell-worker".into())
        .spawn(move || {
            let manager = DictionaryPaths::resolve(root.as_deref()).map(DictionaryManager::new);
            let debug = koru_env::switch(koru_env::name::KORU_SPELL_DEBUG);
            if debug {
                tracing::info!(
                    target: "koru::spell",
                    dictionaries = ?manager.as_ref().map(|m| m.paths().root().display().to_string()),
                    "worker started"
                );
            }
            loop {
                let Ok(first) = jobs.recv() else {
                    break;
                };
                // Coalesce the check backlog: a keystroke burst queues the
                // same paragraph repeatedly and only the newest revision
                // matters. Suggestion lookups are never coalesced — each was
                // an explicit menu open. Order is preserved for survivors.
                let mut batch: Vec<Job> = vec![first];
                while let Ok(job) = jobs.try_recv() {
                    if let Work::Check(request) = &job.work {
                        let key = (job.client, request.node, request.cell);
                        batch.retain(|b| match &b.work {
                            Work::Check(r) => (b.client, r.node, r.cell) != key,
                            Work::Suggest { .. } | Work::Reload(_) => true,
                        });
                    }
                    batch.push(job);
                }
                let mut answered = false;
                for job in batch {
                    let Some(manager) = &manager else { continue };
                    let started = std::time::Instant::now();
                    match job.work {
                        Work::Check(request) => {
                            let Ok(provider) = manager.load(request.lang) else {
                                // Missing or unparseable dictionary: no
                                // underlines, and the manager remembers the
                                // failure so this is not re-attempted per
                                // keystroke.
                                continue;
                            };
                            let issues: Vec<Range<usize>> =
                                tokenizer::words(&request.text, &request.exclusions)
                                    .into_iter()
                                    .filter(|token| {
                                        // The overlay outranks the engine
                                        // (spec §22): a word the person taught
                                        // us is correct, whatever the
                                        // dictionary thinks.
                                        if provider.check(&token.check) {
                                            return false;
                                        }
                                        let lower = token.check.to_lowercase();
                                        !personal_contains(request.lang, &lower)
                                            && !workspace_contains(request.lang, &lower)
                                    })
                                    .map(|token| token.range)
                                    .collect();
                            if debug {
                                // Counts and timings only — never a word or
                                // any text.
                                tracing::info!(
                                    target: "koru::spell",
                                    lang = request.lang.tag(),
                                    bytes = request.text.len(),
                                    issues = issues.len(),
                                    micros = started.elapsed().as_micros() as u64,
                                    "checked"
                                );
                            }
                            answered |= job
                                .reply
                                .send(SpellReply::Checked(SpellResult {
                                    node: request.node,
                                    cell: request.cell,
                                    revision: request.revision,
                                    issues,
                                }))
                                .is_ok();
                        }
                        Work::Reload(lang) => {
                            let _ = manager.reload(lang);
                            // Every controller re-resolves on a preference
                            // bump, which re-checks what it shows.
                            bump_prefs();
                            answered = true;
                        }
                        Work::Suggest { lang, word } => {
                            let Ok(provider) = manager.load(lang) else {
                                continue;
                            };
                            let suggestions = provider.suggest(&word, 5);
                            if debug {
                                tracing::info!(
                                    target: "koru::spell",
                                    lang = lang.tag(),
                                    suggestions = suggestions.len(),
                                    micros = started.elapsed().as_micros() as u64,
                                    "suggested"
                                );
                            }
                            answered |= job
                                .reply
                                .send(SpellReply::Suggestions { word, suggestions })
                                .is_ok();
                        }
                    }
                }
                if answered {
                    // One wake per batch: about_to_wait runs every view's
                    // maintain, which drains every reply channel.
                    let _ = proxy.send_event(Wake::Spelling);
                }
            }
        });
}
