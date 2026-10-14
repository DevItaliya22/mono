// SPDX-License-Identifier: MPL-2.0

//! Document-local spell checking. The controller belongs to one document tab —
//! the same charter as [`crate::search`]'s local find: issues are derived from
//! the editor's row cache, only presentation crosses into
//! `ViewState::spelling`, and nothing here ever enters Loro.
//!
//! ## The lifecycle of one underline
//!
//! ```text
//! edit → tracker sees the node → its old issues *stay up* → pending
//!      → quiet 200 ms (or a completed word: immediately; or ceiling 1 s)
//!      → extract prose + exclusions → enqueue on the spell worker
//!      → Wake::Spelling → maintain drains → revision matches? → issues
//!      → ViewState::spelling → next frame draws the squiggle
//! ```
//!
//! Old issues survive the recheck window on purpose: clearing them at the
//! keystroke made every underline in the row blink off and on while typing
//! (acceptance feedback). A kept range can be stale against the new text for
//! the debounce window — [`SpellCheck::sync`] drops any that no longer slice
//! the row, and the fresh result replaces the whole set.
//!
//! Stale answers die twice: a result for a node edited since it was asked
//! (`pending` holds the node again) is discarded, and a result whose echoed
//! revision is not the one most recently asked for (`inflight`) is discarded —
//! the projection worker's compared-never-ordered idiom, per block.
//!
//! ## What is deliberately not here
//!
//! No timer walks the document — the tracker's `touched` list is the only
//! feed, plus one full pass when a document opens. No dictionary I/O — the
//! worker owns that, off this thread. And no reach into the engine: requests
//! go through `koru_workspace_host::spelling`, whose worker is the one door
//! to `koru-spell` (plan D1).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use koru_core::NodeId;
use koru_core::change::ChangeTracker;
use koru_core::node::Kind;
use koru_core::schema::WorkspaceDoc;
use koru_editor::view_state::SpellingUnderline;
use koru_editor::{Editor, trigger};
use koru_workspace_host::spelling::{
    SpellClient, SpellEdit, SpellLang, SpellReply, SpellRequest, client, document_ignored,
    edit as spell_edit, effective_language, enabled as spelling_enabled, ensure_document_ignores,
    overlay_version, personal_contains, prefs_version, workspace_contains,
};

/// The quiet-plus-ceiling debounce (the projection worker's idiom, tightened
/// for a person watching the screen): recheck when typing pauses, and no later
/// than a second into continuous typing.
const QUIET: Duration = Duration::from_millis(200);
const CEILING: Duration = Duration::from_millis(1000);

// There is deliberately no reveal timeout: the word under the caret stays
// unflagged for as long as the caret touches it. The spec's idle timeout
// flagged "Everno" while "Evernote" was still being typed (acceptance
// feedback) — a word is finalized by a boundary or by the caret leaving,
// never by the clock.

/// When one edited node became due for a recheck.
struct Pending {
    first: Instant,
    last: Instant,
    /// A word boundary landed in this node — skip the quiet period; the word
    /// is finished and the person is owed the answer inside 100 ms.
    immediate: bool,
}

/// One checked unit: a prose row, or one rendered table cell of a row.
type IssueKey = (NodeId, Option<(usize, usize)>);

pub struct SpellCheck {
    /// `None` when no worker runs in this process, or no dictionary matches
    /// the language — spell checking is then quietly off (plan D6/D7), and
    /// every method here is a cheap no-op.
    client: Option<SpellClient>,
    lang: Option<SpellLang>,
    tracker: Option<ChangeTracker>,
    /// The current issues per checked unit, as byte ranges over that unit's
    /// text (the row's, or the rendered cell's).
    issues: HashMap<IssueKey, Vec<Range<usize>>>,
    /// Edited nodes waiting out the debounce. Per node, not per cell: one
    /// keystroke in a table re-renders the whole grid anyway.
    pending: HashMap<NodeId, Pending>,
    /// The revision each in-flight request was asked at; only the newest
    /// answer per unit is accepted.
    inflight: HashMap<IssueKey, Vec<u8>>,
    /// Where the caret last stood, so a move — typing, an arrow, a click —
    /// re-evaluates the caret-word suppression without waiting for an edit.
    last_caret: Option<(NodeId, usize)>,
    /// The suggestion cache, keyed by lowercase word — the plan's LRU (D13),
    /// approximated as a capped map because the miss cost is one worker
    /// round-trip, not a disk. Cleared wholesale at the cap.
    suggestions: HashMap<String, Vec<String>>,
    /// Words ignored for this session ("Ignore" in the menu), lowercase.
    ignored: HashSet<String>,
    /// This document's stable address (`document:<ws>:<doc>:<root>`), built
    /// once the workspace and root are known — the key the per-document
    /// ignore overlay files under, and what a reseat or document swap resets.
    entity: Option<(NodeId, String)>,
    /// The custom-word overlay version last acted on; a bump means somebody
    /// (this view or another) taught the checker a word, so standing issues
    /// are purged and everything rechecks.
    overlay_seen: u64,
    /// The preference version last resolved against; a bump means the toggle
    /// or a language choice moved (this window's Settings tab or another's),
    /// so the effective language is re-derived.
    prefs_seen: u64,
    /// How many times [`observe`](Self::observe) has been called.
    ///
    /// Counted for the same reason the search's `refreshes` is: an observation
    /// drains a tracker, so a second one for the same document change is a
    /// silent no-op and leaves nothing to assert on. Q-B4's fix had to keep the
    /// count of these unchanged in both hosts, and reading the code is not
    /// evidence. Incremented before every guard, so it counts *calls*.
    observations: u64,
}

/// What the context menu was opened on, frozen at open (D13, as amended at
/// acceptance): the word and range never change under the pointer, and the
/// one placeholder a cold cache shows is filled in place the moment the
/// worker answers — reopening the menu to see suggestions was rejected.
#[derive(Clone, Debug)]
pub struct SpellSnapshot {
    pub node: NodeId,
    pub range: Range<usize>,
    pub word: String,
    /// `None` while the worker has not answered yet — the placeholder row.
    pub suggestions: Option<Vec<String>>,
    /// The click had no selection: the menu is the corrections alone.
    pub only: bool,
}

/// How many distinct words' suggestions are remembered before the cache is
/// simply emptied. A person who has right-clicked a thousand distinct words
/// in one session can pay one extra round-trip.
const SUGGESTION_CACHE_CAP: usize = 1000;

impl SpellCheck {
    pub fn new() -> SpellCheck {
        SpellCheck {
            // Asked again lazily in `observe`: a view can be built before the
            // shell has spawned the worker, and a `None` taken then must not
            // become "off forever".
            client: client(),
            // Resolved in `retune` from the chain the plan fixes (D4/D6/D9,
            // D6 amended at acceptance): the document's own language, else
            // the person's setting, else their profile locale, else en-US.
            lang: None,
            tracker: None,
            issues: HashMap::new(),
            pending: HashMap::new(),
            inflight: HashMap::new(),
            last_caret: None,
            suggestions: HashMap::new(),
            ignored: HashSet::new(),
            entity: None,
            overlay_seen: overlay_version(),
            // Zero, deliberately: the first observe resolves the language.
            prefs_seen: 0,
            observations: 0,
        }
    }

    /// How many times this controller has been shown the document. See
    /// [`observations`](Self::observations).
    pub fn observations(&self) -> u64 {
        self.observations
    }

    /// Re-derive the effective language: the document's declared prose
    /// language when it names a dictionary we have, else the person's
    /// preference chain — and recheck everything when the answer moved.
    fn retune(&mut self, editor: &mut Editor, ws: &WorkspaceDoc) {
        let document = editor
            .body
            .root
            .and_then(|root| ws.read(root).ok())
            .and_then(|node| node.meta.prose_language)
            .as_deref()
            .and_then(SpellLang::from_locale_tag);
        // The override outranks the person's chain but not the off switch: a
        // document's language must not turn a disabled checker back on — and
        // it *does* check a German document for a `ja-JP` reader whose own
        // chain falls back to en-US (D4 over D6, D6 amended at acceptance).
        let effective = spelling_enabled()
            .then(|| document.or_else(effective_language))
            .flatten();
        if effective == self.lang {
            return;
        }
        self.lang = effective;
        self.issues.clear();
        self.inflight.clear();
        self.pending.clear();
        // Suggestions are per-language; a cache warmed in English must not
        // answer for German.
        self.suggestions.clear();
        if self.lang.is_some() {
            let now = Instant::now();
            for row in &editor.body.rows {
                self.pending.insert(
                    row.id,
                    Pending {
                        first: now,
                        last: now - QUIET,
                        immediate: false,
                    },
                );
            }
        }
        self.sync(editor);
    }

    /// This document's entity address, for the menu's document-scoped rows.
    pub fn entity(&self) -> Option<&str> {
        self.entity.as_ref().map(|(_, entity)| entity.as_str())
    }

    /// The language checks run in, for the menu's word actions.
    pub fn lang(&self) -> Option<SpellLang> {
        self.lang
    }

    /// "Add to personal dictionary": valid everywhere, roams across this
    /// person's devices. Optimistic — the module updates the shared overlay
    /// now and writes to the store off-thread; the purge below clears the
    /// word's underlines in this view immediately.
    pub fn add_to_dictionary(&mut self, editor: &mut Editor, word: &str) {
        let Some(lang) = self.lang else { return };
        spell_edit(SpellEdit::AddPersonal {
            lang,
            word: word.to_string(),
        });
        self.overlay_seen = overlay_version();
        self.purge(editor);
    }

    /// "Add to workspace dictionary": raise the want the shell drains — only
    /// it can reach the workspace's system document. The word clears here
    /// optimistically through the session set; the shell's overlay refresh
    /// makes it durable and shared within the same loop pass.
    pub fn add_to_workspace(&mut self, editor: &mut Editor, word: &str) {
        let Some(lang) = self.lang else { return };
        editor.view.wants_workspace_word = Some((lang.tag().to_owned(), word.to_owned()));
        self.ignored.insert(word.to_lowercase());
        self.purge(editor);
    }

    /// "Ignore in this document": correct here, for this person, still
    /// flagged everywhere else.
    pub fn ignore_in_document(&mut self, editor: &mut Editor, word: &str) {
        let (Some(lang), Some((_, entity))) = (self.lang, self.entity.as_ref()) else {
            return;
        };
        spell_edit(SpellEdit::IgnoreInDocument {
            entity: entity.clone(),
            lang,
            word: word.to_string(),
        });
        self.overlay_seen = overlay_version();
        self.purge(editor);
    }

    /// Whether the overlays or the session set say `word` is not an issue.
    fn suppressed(&self, word_lower: &str) -> bool {
        if self.ignored.contains(word_lower) {
            return true;
        }
        let Some(lang) = self.lang else { return false };
        if personal_contains(lang, word_lower) || workspace_contains(lang, word_lower) {
            return true;
        }
        self.entity
            .as_ref()
            .is_some_and(|(_, entity)| document_ignored(entity, lang, word_lower))
    }

    /// Drop every standing issue the overlays now cover, and resync.
    fn purge(&mut self, editor: &mut Editor) {
        for ((node, cell), ranges) in self.issues.iter_mut() {
            if cell.is_some() {
                continue;
            }
            let Some(row) = editor.body.row(*node) else {
                continue;
            };
            let (ignored, lang, entity) = (&self.ignored, self.lang, self.entity.as_ref());
            ranges.retain(|range| {
                let Some(text) = row.text.get(range.clone()) else {
                    return true;
                };
                let lower = text.to_lowercase();
                if ignored.contains(&lower) {
                    return false;
                }
                let Some(lang) = lang else { return true };
                if personal_contains(lang, &lower) || workspace_contains(lang, &lower) {
                    return false;
                }
                !entity.is_some_and(|(_, entity)| document_ignored(entity, lang, &lower))
            });
        }
        self.issues.retain(|_, ranges| !ranges.is_empty());
        self.sync(editor);
    }

    /// The prose issue containing byte `at` of `node`, for a right-click.
    pub fn issue_at(&self, node: NodeId, at: usize) -> Option<Range<usize>> {
        self.issues
            .get(&(node, None))?
            .iter()
            .find(|range| range.start <= at && at <= range.end)
            .cloned()
    }

    /// Freeze what the menu will show for `word`: the cached suggestions, or
    /// a placeholder — in which case the worker is asked now, and the view
    /// fills the open menu the moment the answer lands.
    pub fn snapshot(&mut self, node: NodeId, range: Range<usize>, word: &str) -> SpellSnapshot {
        let cached = self.suggestions.get(&word.to_lowercase()).cloned();
        if cached.is_none()
            && let (Some(client), Some(lang)) = (&self.client, self.lang)
        {
            client.suggest(lang, word.to_string());
        }
        SpellSnapshot {
            node,
            range,
            word: word.to_string(),
            suggestions: cached,
            only: false,
        }
    }

    /// The cached suggestions for `word`, if the worker has answered — what
    /// the view pours into an open menu's placeholder.
    pub fn cached_suggestions(&self, word: &str) -> Option<&[String]> {
        self.suggestions
            .get(&word.to_lowercase())
            .map(Vec::as_slice)
    }

    /// "Ignore": this word stops being an issue for the rest of the session,
    /// in every block, for this view.
    pub fn ignore_once(&mut self, editor: &mut Editor, word: &str) {
        self.ignored.insert(word.to_lowercase());
        self.purge(editor);
    }

    /// Notice what changed. Called from the view's `draw` (a frame always
    /// follows a local edit) and from `adopt` (a peer's edit arrives with no
    /// local frame promised). Costs a mutex lock and an empty vec when
    /// nothing changed.
    pub fn observe(&mut self, editor: &mut Editor, ws: &WorkspaceDoc) {
        self.observations = self.observations.saturating_add(1);
        if self.client.is_none() {
            // The worker may have started after this view was built.
            self.client = client();
        }
        if self.client.is_none() {
            return;
        }
        // Preferences may have moved (a Settings tab, this window's or
        // another's), and the first observe resolves the language at all.
        let prefs = prefs_version();
        if prefs != self.prefs_seen {
            self.prefs_seen = prefs;
            self.retune(editor, ws);
        }
        if self.lang.is_none() {
            // Off, or no dictionary for anybody's chain. Keep the tracker
            // drained so a long off session does not hoard changes.
            if let Some(tracker) = &self.tracker {
                let _ = tracker.drain(ws);
            }
            return;
        }
        // The document's address, for per-document ignores. Rebuilt when the
        // root changes (a tab now showing a different document); the shell
        // pushes the workspace id after construction, so keep trying.
        if self
            .entity
            .as_ref()
            .map(|(root, _)| Some(*root) != editor.body.root)
            .unwrap_or(true)
            && let (Some(workspace), Some(root)) = (editor.view.workspace, editor.body.root)
        {
            let uuid = root.as_uuid();
            let entity = format!("document:{}:{}:{}", workspace.to_storage(), uuid, uuid);
            ensure_document_ignores(&entity);
            self.entity = Some((root, entity));
        }
        if self.tracker.is_none() {
            // First sight of the document: subscribe, then check all of it
            // once — the checking is off-thread, so an open costs this thread
            // one walk of the row cache.
            self.tracker = Some(ws.track_changes());
            let now = Instant::now();
            for row in &editor.body.rows {
                self.pending.insert(
                    row.id,
                    Pending {
                        first: now,
                        last: now - QUIET,
                        immediate: false,
                    },
                );
            }
        }
        let batch = self.tracker.as_ref().expect("just set").drain(ws);
        let completed = editor.view.word_completed.take();
        // The caret-word suppression follows the caret, not just the edits: an
        // arrow or a click leaving a flagged word must reveal it (and entering
        // one must hold it back), with no clock involved.
        let caret = (editor.view.active && editor.body.focus < editor.body.rows.len()).then(|| {
            let row = &editor.body.rows[editor.body.focus];
            (row.id, row.caret())
        });
        let caret_moved = caret != self.last_caret;
        self.last_caret = caret;
        if batch.touched.is_empty() && !batch.structure_dirty && completed.is_none() {
            if caret_moved {
                self.sync(editor);
            }
            return;
        }
        let now = Instant::now();
        let mut resync = caret_moved;
        let root = editor.body.root;
        for node in batch.touched {
            // A touched root may be a changed document language (the picker,
            // or a peer's) — re-derive, which is a no-op when it is not.
            if Some(node) == root {
                self.retune(editor, ws);
            }
            // The block's old underlines stay up while the recheck is in
            // flight — clearing them here blinked every squiggle in the row
            // on each keystroke. A table is the exception: its cells re-render
            // wholesale and a kept per-cell range has nothing to validate
            // against, so its overlay clears the way it always did.
            if editor
                .body
                .row(node)
                .is_some_and(|row| row.kind == Kind::Table)
            {
                self.issues.retain(|key, _| key.0 != node);
            }
            self.inflight.retain(|key, _| key.0 != node);
            let entry = self.pending.entry(node).or_insert(Pending {
                first: now,
                last: now,
                immediate: false,
            });
            entry.last = now;
            if completed == Some(node) {
                entry.immediate = true;
            }
            resync = true;
        }
        if batch.structure_dirty {
            // Deleted rows must not hold issues forever; rows that moved keep
            // theirs — the ranges are content-relative, not position-relative.
            let body = &editor.body;
            self.issues.retain(|key, _| body.row(key.0).is_some());
            resync = true;
        }
        if resync {
            // Re-evaluated so the caret-word suppression follows the typing;
            // the kept issues themselves only move when a result lands.
            self.sync(editor);
        }
    }

    /// The next moment work is due, for the event loop's one `WaitUntil`.
    pub fn maintenance_deadline(&self) -> Option<Instant> {
        self.pending
            .values()
            .map(|p| {
                if p.immediate {
                    p.last
                } else {
                    (p.last + QUIET).min(p.first + CEILING)
                }
            })
            .min()
    }

    /// Drain answers, send due questions. Returns whether what a frame draws
    /// changed. Free when idle: one empty `try_recv` and two empty-map looks.
    pub fn maintain(&mut self, editor: &mut Editor, ws: &WorkspaceDoc, now: Instant) -> bool {
        if self.client.is_none() {
            return false;
        }
        let mut changed = false;
        // A Settings change re-derives the language — and clearing every
        // underline when the toggle goes off is exactly this path.
        let prefs = prefs_version();
        if prefs != self.prefs_seen {
            self.prefs_seen = prefs;
            self.retune(editor, ws);
            changed = true;
        }
        // Somebody taught the checker a word (this view or another): purge
        // what the overlays now cover and recheck everything, so a word
        // *removed* from the dictionary later gets re-flagged too.
        let overlay = overlay_version();
        if overlay != self.overlay_seen {
            self.overlay_seen = overlay;
            self.purge(editor);
            let now = Instant::now();
            for row in &editor.body.rows {
                self.pending.entry(row.id).or_insert(Pending {
                    first: now,
                    last: now - QUIET,
                    immediate: false,
                });
            }
            changed = true;
        }
        let Some(client) = &self.client else {
            return false;
        };
        for reply in client.drain() {
            let result = match reply {
                SpellReply::Checked(result) => result,
                SpellReply::Suggestions { word, suggestions } => {
                    // Into the cache; the *view* pours it into an open menu's
                    // placeholder (`fill_context_suggestions`) — the split
                    // keeps this controller ignorant of menus.
                    if self.suggestions.len() >= SUGGESTION_CACHE_CAP {
                        self.suggestions.clear();
                    }
                    self.suggestions.insert(word.to_lowercase(), suggestions);
                    continue;
                }
            };
            if self.pending.contains_key(&result.node) {
                // Edited again after this was asked; a fresh answer is coming.
                continue;
            }
            let key = (result.node, result.cell);
            if self.inflight.get(&key) != Some(&result.revision) {
                continue;
            }
            self.inflight.remove(&key);
            changed = true;
            // Session ignores and the per-document overlay are filtered on
            // arrival, so both hold across rechecks without teaching the
            // engine anything. (The personal dictionary is the worker's
            // filter; this repeats it only through `suppressed`, which is
            // what makes an add land before the recheck does.)
            let mut issues = result.issues;
            if result.cell.is_none()
                && let Some(row) = editor.body.row(result.node)
            {
                issues.retain(|range| {
                    row.text
                        .get(range.clone())
                        .is_none_or(|text| !self.suppressed(&text.to_lowercase()))
                });
            }
            if issues.is_empty() {
                self.issues.remove(&key);
            } else {
                self.issues.insert(key, issues);
            }
        }
        // Send what the debounce has released — only while a language is in
        // force; with none (off, or nothing resolvable) the backlog is moot.
        if self.lang.is_none() {
            self.pending.clear();
        }
        let due: Vec<NodeId> = self
            .pending
            .iter()
            .filter(|(_, p)| p.immediate || now >= (p.last + QUIET).min(p.first + CEILING))
            .map(|(id, _)| *id)
            .collect();
        if !due.is_empty()
            && let Some(lang) = self.lang
        {
            let revision = ws.version().encode();
            let mut send = |key: IssueKey, text: String, exclusions: Vec<Range<usize>>| {
                self.inflight.insert(key, revision.clone());
                client.enqueue(SpellRequest {
                    node: key.0,
                    cell: key.1,
                    revision: revision.clone(),
                    lang,
                    text,
                    exclusions,
                });
            };
            for node in due {
                self.pending.remove(&node);
                let Some(row) = editor.body.row(node) else {
                    continue;
                };
                // A rendered table checks per cell — the row's own text is
                // Markdown table syntax, which must never be flagged (D12).
                // In source mode the syntax *is* on screen, so nothing is
                // checked at all.
                if row.kind == Kind::Table {
                    if editor.view.table_source != Some(node)
                        && let Some(parsed) = row
                            .table
                            .as_ref()
                            .and_then(|t| t.grid.as_ref())
                            .map(|g| &g.parsed)
                    {
                        for (table_row, cells) in std::iter::once(&parsed.header)
                            .chain(parsed.rows.iter())
                            .enumerate()
                        {
                            for (column, cell) in cells.iter().enumerate() {
                                if cell.text.is_empty() {
                                    continue;
                                }
                                let mut exclusions: Vec<Range<usize>> =
                                    trigger::urls_in(&cell.text)
                                        .into_iter()
                                        .map(|(r, _)| r)
                                        .collect();
                                exclusions.extend(address_runs(&cell.text));
                                send(
                                    (node, Some((table_row, column))),
                                    cell.text.clone(),
                                    exclusions,
                                );
                            }
                        }
                    }
                    continue;
                }
                let Some((text, exclusions)) = extract(row) else {
                    // Nothing checkable (a code block, an image): whatever it
                    // was flagged as before it became this, forget.
                    let before = self.issues.len();
                    self.issues.retain(|key, _| key.0 != node);
                    changed |= self.issues.len() != before;
                    continue;
                };
                send((node, None), text, exclusions);
            }
        }
        if changed {
            self.sync(editor);
        }
        changed
    }

    /// Rebuild `ViewState::spelling` from the issue store. The one filter:
    /// while the caret touches a flagged word its underline is held back —
    /// the word being typed is not a misspelling yet, and only the caret
    /// leaving (a boundary typed, an arrow, a click) reveals it.
    fn sync(&mut self, editor: &mut Editor) {
        let caret = (editor.view.active && editor.body.focus < editor.body.rows.len()).then(|| {
            let row = &editor.body.rows[editor.body.focus];
            (row.id, row.caret())
        });
        let mut underlines = Vec::new();
        for ((node, cell), ranges) in &self.issues {
            // Issues survive the recheck debounce (see the module doc), so a
            // range may be stale against text edited since it was found. One
            // that no longer slices the row is dropped rather than drawn over
            // whatever bytes now sit there.
            let row = (cell.is_none()).then(|| editor.body.row(*node)).flatten();
            for range in ranges {
                if cell.is_none() && !row.is_some_and(|row| row.text.get(range.clone()).is_some()) {
                    continue;
                }
                if cell.is_none()
                    && let Some((focus, at)) = caret
                    && focus == *node
                    && range.start <= at
                    && at <= range.end
                {
                    // The caret is touching this word — still being typed,
                    // not yet a misspelling. Revealed by the caret leaving,
                    // never by a clock (see the note by QUIET).
                    continue;
                }
                underlines.push(SpellingUnderline {
                    node: *node,
                    range: range.clone(),
                    table_cell: *cell,
                });
            }
        }
        editor.view.spelling = underlines;
    }
}

/// One row's checkable text and the byte ranges the tokenizer must skip, or
/// `None` when the row has nothing to check.
fn extract(row: &koru_editor::Paragraph) -> Option<(String, Vec<Range<usize>>)> {
    match row.kind {
        // Code is not prose; tables get per-cell extraction later in this
        // phase; the rest carry no text at all.
        Kind::Code | Kind::Table | Kind::Image | Kind::Divider | Kind::Canvas | Kind::Trash => {
            return None;
        }
        _ => {}
    }
    // A live dictation composes provisional text into `row.text`; checking it
    // would flag words the person has not finished saying.
    if row.composition.is_some() {
        return None;
    }
    if row.text.is_empty() {
        return None;
    }
    let mut exclusions: Vec<Range<usize>> = row
        .runs
        .iter()
        .filter(|run| run.code)
        .map(|run| run.range())
        .collect();
    // Raw URLs that were never linkified into pills (a paste can leave them).
    exclusions.extend(trigger::urls_in(&row.text).into_iter().map(|(r, _)| r));
    // Email addresses and filesystem paths: the maximal non-whitespace run
    // around an `@`, a `\` or a `/` is an address, not prose.
    exclusions.extend(address_runs(&row.text));
    Some((row.text.clone(), exclusions))
}

/// Every maximal non-whitespace run containing `@`, `\` or `/`.
fn address_runs(text: &str) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut start = None;
    let mut marked = false;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take()
                && marked
            {
                runs.push(s..i);
            }
            marked = false;
        } else {
            start.get_or_insert(i);
            marked |= matches!(c, '@' | '\\' | '/');
        }
    }
    if let Some(s) = start
        && marked
    {
        runs.push(s..text.len());
    }
    runs
}

#[cfg(test)]
mod tests;
