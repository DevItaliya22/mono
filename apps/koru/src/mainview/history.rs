// SPDX-License-Identifier: MPL-2.0

//! Deadline-driven adapter between Store history and chrome's generic rows.
//!
//! HTTP, snapshot decoding and semantic diffing stay on one worker thread.
//! Drawing only reads cached state; [`HistoryPanel::maintain`] owns target
//! changes, bounded refresh, completion draining and request serialization.

use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use crate::panes::context::HistoryRow;
use koru_history::snapshot_diff::snapshot_compare_timed;
use koru_history::unified::{DiffOptions, unified_with_correspondence};
use koru_i18n::Key;
use koru_store_core::ids::{DocumentId, HistoryEntryId, WorkspaceId};
use koru_store_wire::{CaptureHistory, HistoryQuery};

use crate::diff::Comparison;

const REFRESH_EVERY: Duration = Duration::from_secs(5);

/// One compare, resolved to reading order at enqueue time: `from` is always
/// the older entry regardless of click order, so a diff never reads backwards
/// by accident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareRequest {
    from_key: String,
    /// `None` compares against the document as it is now — which saves
    /// nothing: a comparison is a question, not an event.
    to_key: Option<String>,
    from_recorded_at_ms: i64,
    to_recorded_at_ms: i64,
    from_reason: Key,
    to_reason: Key,
    from_label: Option<String>,
    to_label: Option<String>,
}

/// "Nothing changed since this entry" — what a Ctrl+S probe answers with, so
/// the prompt can offer a rename of it instead of a duplicate save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameTarget {
    pub key: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Work {
    List,
    Save {
        label: Option<String>,
    },
    Compare(CompareRequest),
    /// Is the document as it stands identical to the newest saved version?
    Probe,
    Relabel {
        key: String,
        label: Option<String>,
    },
}

/// One fetched history entry, kept with its raw moment so the relative label
/// can age ("1 min" becomes "2 min") without another fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    key: String,
    reason: Key,
    recorded_at_ms: i64,
    /// The user-given name, shown in place of the reason when present.
    label: Option<String>,
}

#[derive(Debug)]
enum Answer {
    Rows(Vec<Entry>),
    Saved,
    Compared(Box<Comparison>),
    /// `Some` when nothing changed since the newest entry — the rename offer.
    Probed(Option<RenameTarget>),
    Relabeled,
    Failed(String),
}

#[derive(Debug)]
struct StampedAnswer {
    target: (WorkspaceId, DocumentId),
    generation: u64,
    answer: Answer,
}

#[derive(Default)]
pub struct HistoryPanel {
    client: Option<koru_api_client::Client>,
    target: Option<(WorkspaceId, DocumentId)>,
    generation: u64,
    visible: bool,
    pending: VecDeque<Work>,
    refresh_queued: bool,
    in_flight: Option<Work>,
    receiver: Option<Receiver<StampedAnswer>>,
    next_refresh: Option<Instant>,
    lang: koru_i18n::Lang,
    entries: Vec<Entry>,
    /// The minute the row labels were last built for. Relative labels change
    /// at minute granularity at their finest, so this is what makes aging
    /// cost one rebuild per minute instead of one per maintenance tick.
    labels_minute: i64,
    rows: Vec<HistoryRow>,
    notice: Option<String>,
    comparison_status: Option<String>,
    /// The last finished comparison, for the diff view.
    comparison: Option<Comparison>,
    /// A comparison that finished since the container last asked — what makes
    /// the diff tab open exactly once per compare.
    fresh_comparison: Option<Comparison>,
    /// A probe answer the container has not read yet. `Some(Some(_))` offers
    /// a rename, `Some(None)` says the document changed — keep saving.
    fresh_probe: Option<Option<RenameTarget>>,
}

impl HistoryPanel {
    /// Cache the Store client supplied during drawing. No request is made here;
    /// the view's maintenance hook owns all background work.
    pub fn set_client(&mut self, client: Option<koru_api_client::Client>) {
        self.client = client;
    }

    /// The language row labels and tooltips are written in. Resolved once by
    /// the view from the OS locale (ADR-87: shown language is the nearest
    /// supported one).
    pub fn set_lang(&mut self, lang: koru_i18n::Lang) {
        self.lang = lang;
    }

    /// Save the document as it stands, optionally under a name.
    pub fn save_with_label(&mut self, label: Option<String>) {
        self.pending.push_back(Work::Save { label });
    }

    /// Ask whether anything changed since the newest saved version. The
    /// answer arrives through [`take_probe`](Self::take_probe).
    pub fn probe(&mut self) {
        self.pending.push_back(Work::Probe);
    }

    /// Rename a saved version. `None` clears the name back to its reason.
    pub fn relabel(&mut self, key: String, label: Option<String>) {
        self.pending.push_back(Work::Relabel { key, label });
    }

    /// The probe answer, once. `Some(Some(_))` offers a rename of the entry
    /// nothing has diverged from; `Some(None)` confirms there are changes.
    pub fn take_probe(&mut self) -> Option<Option<RenameTarget>> {
        self.fresh_probe.take()
    }

    /// One compare endpoint as the panel knows it: when it happened, why it
    /// was taken, and the name the person gave it, if any.
    fn endpoint_meta(&self, key: &str) -> (i64, Key, Option<String>) {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| (entry.recorded_at_ms, entry.reason, entry.label.clone()))
            .unwrap_or((0, Key::HistoryReasonVersion, None))
    }

    pub fn compare(&mut self, left: String, right: String) {
        let (left_ms, left_reason, left_label) = self.endpoint_meta(&left);
        let (right_ms, right_reason, right_label) = self.endpoint_meta(&right);
        // Reading order is older → newer, whatever order the rows were
        // clicked in.
        let (from, to) = if left_ms <= right_ms {
            (
                (left, left_ms, left_reason, left_label),
                (right, right_ms, right_reason, right_label),
            )
        } else {
            (
                (right, right_ms, right_reason, right_label),
                (left, left_ms, left_reason, left_label),
            )
        };
        self.pending.push_back(Work::Compare(CompareRequest {
            from_key: from.0,
            to_key: Some(to.0),
            from_recorded_at_ms: from.1,
            to_recorded_at_ms: to.1,
            from_reason: from.2,
            to_reason: to.2,
            from_label: from.3,
            to_label: to.3,
        }));
    }

    /// Compare one saved version against the document as it is now.
    pub fn compare_with_current(&mut self, key: String) {
        let (from_ms, from_reason, from_label) = self.endpoint_meta(&key);
        self.pending.push_back(Work::Compare(CompareRequest {
            from_key: key,
            to_key: None,
            from_recorded_at_ms: from_ms,
            to_recorded_at_ms: chrono::Utc::now().timestamp_millis(),
            from_reason,
            to_reason: Key::HistoryCurrent,
            from_label,
            to_label: None,
        }));
    }

    /// The only clock History contributes to the event loop. Hidden history
    /// has no deadline and therefore no background traffic.
    pub fn deadline(&self) -> Option<Instant> {
        (self.visible && self.target.is_some())
            .then_some(self.next_refresh)
            .flatten()
    }

    /// Maintain the visible target and its one serialized worker.
    ///
    /// Returns true only when cached UI state changed and needs a redraw.
    pub fn maintain(
        &mut self,
        target: Option<(WorkspaceId, DocumentId)>,
        visible: bool,
        now: Instant,
    ) -> bool {
        let mut changed = false;

        if target != self.target {
            self.target = target;
            self.generation = self.generation.wrapping_add(1);
            self.entries.clear();
            self.rows.clear();
            self.notice = target.map(|_| koru_i18n::t(self.lang, Key::HistoryLoading).into());
            self.comparison_status = None;
            self.comparison = None;
            self.fresh_comparison = None;
            self.fresh_probe = None;
            self.pending.clear();
            self.refresh_queued = false;
            self.next_refresh = (self.visible && target.is_some()).then_some(now);
            changed = true;
        }

        if visible != self.visible {
            self.visible = visible;
            if visible && self.target.is_some() {
                if self.rows.is_empty() {
                    self.notice = Some(koru_i18n::t(self.lang, Key::HistoryLoading).into());
                }
                self.next_refresh = Some(now);
            } else {
                self.next_refresh = None;
                self.pending.retain(|work| !matches!(work, Work::List));
                self.refresh_queued = false;
            }
            changed = true;
        }

        if let Some(receiver) = &self.receiver
            && let Ok(stamped) = receiver.try_recv()
        {
            self.receiver = None;
            let completed = self.in_flight.take();
            let current = Some(stamped.target) == self.target
                && stamped.generation == self.generation
                && self.visible;
            if current {
                changed |= self.apply_answer(stamped.answer);
            }
            if matches!(completed, Some(Work::List)) && self.refresh_queued {
                self.refresh_queued = false;
                self.queue_refresh();
            }
        }

        if self.visible
            && self.target.is_some()
            && self.next_refresh.is_some_and(|deadline| deadline <= now)
        {
            self.queue_refresh();
            self.next_refresh = Some(now + REFRESH_EVERY);
        }

        if self.receiver.is_none() {
            changed |= self.start_next();
        }

        // Relative labels age. A rebuild costs a few string formats over at
        // most fifty rows, so it happens when a minute boundary passes, never
        // per tick — and `changed` stays false unless a label actually moved.
        if self.visible && !self.entries.is_empty() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if now_ms / 60_000 != self.labels_minute {
                changed |= self.rebuild_rows(now_ms);
            }
        }
        changed
    }

    /// Rebuild the display rows from the cached entries at `now_ms`.
    fn rebuild_rows(&mut self, now_ms: i64) -> bool {
        self.labels_minute = now_ms / 60_000;
        let rows: Vec<HistoryRow> = self
            .entries
            .iter()
            .map(|entry| HistoryRow {
                key: entry.key.clone(),
                label: format!(
                    "{} · {}",
                    // A name the person gave outranks the reason it was taken.
                    entry
                        .label
                        .as_deref()
                        .filter(|label| !label.is_empty())
                        .unwrap_or(koru_i18n::t(self.lang, entry.reason)),
                    koru_i18n::time::relative_narrow(self.lang, entry.recorded_at_ms, now_ms)
                ),
                detail: crate::format::absolute_local(self.lang, entry.recorded_at_ms),
            })
            .collect();
        let changed = rows != self.rows;
        self.rows = rows;
        changed
    }

    fn queue_refresh(&mut self) {
        if matches!(self.in_flight, Some(Work::List)) {
            self.refresh_queued = true;
        } else if !self.pending.iter().any(|work| matches!(work, Work::List)) {
            self.pending.push_back(Work::List);
        }
    }

    fn start_next(&mut self) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let Some(target) = self.target else {
            return false;
        };
        let Some(client) = self.client.clone() else {
            let unavailable = koru_i18n::t(self.lang, Key::HistoryUnavailable);
            let changed = self.notice.as_deref() != Some(unavailable);
            self.notice = Some(unavailable.into());
            return changed;
        };
        let work = self.pending.pop_front().expect("checked non-empty above");
        let generation = self.generation;
        let worker_work = work.clone();
        let (send, receive) = mpsc::channel();
        match std::thread::Builder::new()
            .name("koru-history-ui".into())
            .spawn(move || {
                let answer =
                    run(&client, target.0, target.1, worker_work).unwrap_or_else(Answer::Failed);
                let _ = send.send(StampedAnswer {
                    target,
                    generation,
                    answer,
                });
            }) {
            Ok(_) => {
                self.in_flight = Some(work);
                self.receiver = Some(receive);
                false
            }
            Err(error) => {
                self.notice = Some(koru_i18n::fill(
                    koru_i18n::t(self.lang, Key::HistoryStartFailed),
                    &[&error.to_string()],
                ));
                true
            }
        }
    }

    fn apply_answer(&mut self, answer: Answer) -> bool {
        match answer {
            Answer::Rows(entries) => {
                let mut changed = self.notice.is_some();
                if entries != self.entries {
                    self.entries = entries;
                    changed |= self.rebuild_rows(chrono::Utc::now().timestamp_millis());
                }
                self.notice = None;
                changed
            }
            Answer::Saved => {
                self.notice = Some(koru_i18n::t(self.lang, Key::HistorySaved).into());
                self.queue_refresh();
                true
            }
            Answer::Compared(comparison) => {
                let stats = comparison.diff.stats;
                let changed_blocks =
                    stats.added + stats.removed + stats.modified + stats.moved + stats.reordered;
                let summary = koru_i18n::fill(
                    koru_i18n::t(self.lang, Key::DiffSummary),
                    &[&changed_blocks.to_string(), &stats.unchanged.to_string()],
                );
                self.comparison = Some((*comparison).clone());
                self.fresh_comparison = Some(*comparison);
                self.comparison_status = Some(summary);
                true
            }
            Answer::Probed(target) => {
                self.fresh_probe = Some(target);
                false
            }
            Answer::Relabeled => {
                self.queue_refresh();
                true
            }
            Answer::Failed(error) => {
                let changed = self.notice.as_deref() != Some(error.as_str());
                self.notice = Some(error);
                changed
            }
        }
    }

    /// A comparison that finished since the container last asked. The
    /// container drains this and opens (or refreshes) the diff tab.
    pub fn take_new_comparison(&mut self) -> Option<Comparison> {
        self.fresh_comparison.take()
    }

    pub fn rows(&self) -> Vec<HistoryRow> {
        self.rows.clone()
    }

    pub fn status(&self) -> Option<String> {
        match (&self.comparison_status, &self.notice) {
            (Some(comparison), Some(notice)) => Some(format!("{comparison} · {notice}")),
            (Some(comparison), None) => Some(comparison.clone()),
            (None, Some(notice)) => Some(notice.clone()),
            (None, None) => None,
        }
    }
}

fn run(
    client: &koru_api_client::Client,
    workspace: WorkspaceId,
    document: DocumentId,
    work: Work,
) -> Result<Answer, String> {
    match work {
        Work::List => {
            let page = client
                .history(
                    workspace,
                    document,
                    &HistoryQuery {
                        limit: Some(50),
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?;
            Ok(Answer::Rows(
                page.entries
                    .into_iter()
                    .map(|entry| Entry {
                        key: entry.id.to_storage(),
                        reason: friendly_reason(&entry.reason),
                        recorded_at_ms: entry.recorded_at,
                        label: entry.label,
                    })
                    .collect(),
            ))
        }
        Work::Save { label } => {
            client
                .capture_history(
                    workspace,
                    document,
                    &CaptureHistory {
                        reason: "manual".into(),
                        frontiers: None,
                        label,
                        message: None,
                        boundary: None,
                    },
                )
                .map_err(|e| e.to_string())?;
            Ok(Answer::Saved)
        }
        Work::Probe => {
            let page = client
                .history(
                    workspace,
                    document,
                    &HistoryQuery {
                        limit: Some(1),
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?;
            let Some(newest) = page.entries.into_iter().next() else {
                // No versions yet: there is nothing to rename, so a save is
                // the only sensible offer.
                return Ok(Answer::Probed(None));
            };
            let old = client
                .history_snapshot(workspace, newest.id)
                .map_err(|e| e.to_string())?;
            let new = client
                .snapshot(workspace, document)
                .map_err(|e| e.to_string())?;
            let old =
                koru_history::SemanticDocument::from_snapshot(&old).map_err(|e| e.to_string())?;
            let new =
                koru_history::SemanticDocument::from_snapshot(&new).map_err(|e| e.to_string())?;
            let unchanged = koru_history::compare(&old, &new).changes.is_empty();
            Ok(Answer::Probed(unchanged.then(|| RenameTarget {
                key: newest.id.to_storage(),
                label: newest.label,
            })))
        }
        Work::Relabel { key, label } => {
            let entry = HistoryEntryId::parse(&key).ok_or("invalid history id")?;
            client
                .relabel_history(workspace, entry, &koru_store_wire::RelabelHistory { label })
                .map_err(|e| e.to_string())?;
            Ok(Answer::Relabeled)
        }
        Work::Compare(request) => {
            let from = HistoryEntryId::parse(&request.from_key).ok_or("invalid history id")?;
            let started = Instant::now();
            let old_bytes = client
                .history_snapshot(workspace, from)
                .map_err(|e| e.to_string())?;
            let new_bytes = match &request.to_key {
                Some(to) => {
                    let to = HistoryEntryId::parse(to).ok_or("invalid history id")?;
                    client
                        .history_snapshot(workspace, to)
                        .map_err(|e| e.to_string())?
                }
                // The live state, fetched rather than read in-process so the
                // O(document) walk stays on this worker either way.
                None => client
                    .snapshot(workspace, document)
                    .map_err(|e| e.to_string())?,
            };
            let fetched = Instant::now();
            let old = koru_history::SemanticDocument::from_snapshot(&old_bytes)
                .map_err(|e| e.to_string())?;
            let new = koru_history::SemanticDocument::from_snapshot(&new_bytes)
                .map_err(|e| e.to_string())?;
            let decoded = Instant::now();
            let (diff, correspondence, timing) =
                snapshot_compare_timed(&old, &new, DiffOptions::default());
            let compared = Instant::now();
            let unified = unified_with_correspondence(
                &old,
                &new,
                &diff,
                &correspondence,
                DiffOptions::default(),
            );
            if history_debug() {
                eprintln!(
                    "[Koru][history-compare] old_bytes={} new_bytes={} old_blocks={} new_blocks={} rows={} fetch_ms={} decode_ms={} project_us={} outer_us={} refine_us={} compare_ms={} unify_ms={}",
                    old_bytes.len(),
                    new_bytes.len(),
                    old.nodes.len(),
                    new.nodes.len(),
                    unified.rows.len(),
                    (fetched - started).as_millis(),
                    (decoded - fetched).as_millis(),
                    timing.project_us,
                    timing.outer_us,
                    timing.refine_us,
                    (compared - decoded).as_millis(),
                    compared.elapsed().as_millis(),
                );
            }
            Ok(Answer::Compared(Box::new(Comparison {
                diff: unified,
                from_recorded_at_ms: request.from_recorded_at_ms,
                to_recorded_at_ms: request.to_recorded_at_ms,
                from_reason: request.from_reason,
                to_reason: request.to_reason,
                from_label: request.from_label,
                to_label: request.to_label,
            })))
        }
    }
}

/// One branch when off ([PRINCIPLES: free when off](../../../../guide/contributing/PRINCIPLES.md)).
fn history_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_HISTORY_DEBUG))
}

fn friendly_reason(reason: &str) -> Key {
    match reason {
        "manual" => Key::HistoryReasonManual,
        "checkpoint" => Key::HistoryReasonCheckpoint,
        "automatic_active" => Key::HistoryReasonActive,
        "automatic_idle" => Key::HistoryReasonIdle,
        "mcp_before" => Key::HistoryReasonMcpBefore,
        "mcp_after" => Key::HistoryReasonMcpAfter,
        "restore_before" => Key::HistoryReasonRestoreBefore,
        "restore_after" => Key::HistoryReasonRestoreAfter,
        _ => Key::HistoryReasonVersion,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> (WorkspaceId, DocumentId) {
        (WorkspaceId::new(), DocumentId::new())
    }

    fn entry(key: &str) -> Entry {
        Entry {
            key: key.into(),
            reason: Key::HistoryReasonManual,
            recorded_at_ms: 0,
            label: None,
        }
    }

    fn complete(panel: &mut HistoryPanel, target: (WorkspaceId, DocumentId), answer: Answer) {
        let (send, receive) = mpsc::channel();
        panel.in_flight = Some(Work::List);
        panel.receiver = Some(receive);
        send.send(StampedAnswer {
            target,
            generation: panel.generation,
            answer,
        })
        .unwrap();
    }

    #[test]
    fn opening_and_each_five_seconds_queue_one_refresh() {
        let now = Instant::now();
        let target = target();
        let mut panel = HistoryPanel::default();
        // Hold a worker slot so this state-machine test can observe the queue
        // without needing a live Store client.
        let (_send, receive) = mpsc::channel();
        panel.receiver = Some(receive);
        panel.maintain(Some(target), true, now);
        assert_eq!(panel.pending, VecDeque::from([Work::List]));
        assert_eq!(panel.deadline(), Some(now + REFRESH_EVERY));

        panel.maintain(Some(target), true, now + REFRESH_EVERY);
        assert_eq!(panel.pending, VecDeque::from([Work::List]));
    }

    #[test]
    fn hidden_history_has_no_deadline_or_refresh() {
        let now = Instant::now();
        let target = target();
        let mut panel = HistoryPanel::default();
        panel.maintain(Some(target), true, now);
        panel.maintain(Some(target), false, now);

        assert_eq!(panel.deadline(), None);
        assert!(panel.pending.is_empty());
        panel.maintain(Some(target), false, now + REFRESH_EVERY * 2);
        assert!(panel.pending.is_empty());
    }

    #[test]
    fn changing_the_visible_document_queues_an_immediate_refresh() {
        let now = Instant::now();
        let old = target();
        let new = (old.0, DocumentId::new());
        let mut panel = HistoryPanel::default();
        panel.target = Some(old);
        panel.visible = true;
        panel.next_refresh = Some(now + REFRESH_EVERY);
        let (_send, receive) = mpsc::channel();
        panel.receiver = Some(receive);

        panel.maintain(Some(new), true, now);
        assert_eq!(panel.pending, VecDeque::from([Work::List]));
        assert_eq!(panel.deadline(), Some(now + REFRESH_EVERY));
    }

    #[test]
    fn an_in_flight_list_coalesces_repeated_deadlines() {
        let now = Instant::now();
        let target = target();
        let mut panel = HistoryPanel::default();
        panel.target = Some(target);
        panel.visible = true;
        panel.next_refresh = Some(now);
        let (_send, receive) = mpsc::channel();
        panel.in_flight = Some(Work::List);
        panel.receiver = Some(receive);

        panel.maintain(Some(target), true, now);
        panel.maintain(Some(target), true, now + REFRESH_EVERY);
        assert!(panel.refresh_queued);
        assert!(panel.pending.is_empty());
    }

    #[test]
    fn click_order_does_not_affect_diff_direction() {
        let mut panel = HistoryPanel::default();
        panel.entries = vec![
            Entry {
                key: "older".into(),
                reason: Key::HistoryReasonManual,
                recorded_at_ms: 100,
                label: Some("v1.0".into()),
            },
            Entry {
                key: "newer".into(),
                reason: Key::HistoryReasonActive,
                recorded_at_ms: 200,
                label: None,
            },
        ];
        panel.compare("newer".into(), "older".into());
        let Some(Work::Compare(request)) = panel.pending.front() else {
            panic!("expected a compare: {:?}", panel.pending);
        };
        assert_eq!(request.from_key, "older");
        assert_eq!(request.to_key.as_deref(), Some("newer"));
        assert_eq!(request.from_recorded_at_ms, 100);
        assert_eq!(request.to_recorded_at_ms, 200);
        // The snapshot's given name rides along, for the diff header.
        assert_eq!(request.from_label.as_deref(), Some("v1.0"));
        assert_eq!(request.to_label, None);
    }

    #[test]
    fn compare_with_current_is_always_older_to_now() {
        let mut panel = HistoryPanel::default();
        panel.entries = vec![Entry {
            key: "saved".into(),
            reason: Key::HistoryReasonManual,
            recorded_at_ms: 100,
            label: None,
        }];
        panel.compare_with_current("saved".into());
        let Some(Work::Compare(request)) = panel.pending.front() else {
            panic!("expected a compare: {:?}", panel.pending);
        };
        assert_eq!(request.from_key, "saved");
        assert_eq!(request.to_key, None);
        assert_eq!(request.to_reason, Key::HistoryCurrent);
        assert!(request.to_recorded_at_ms >= request.from_recorded_at_ms);
    }

    #[test]
    fn a_finished_comparison_is_kept_for_the_diff_view() {
        let now = Instant::now();
        let target = target();
        let mut panel = HistoryPanel::default();
        panel.target = Some(target);
        panel.visible = true;
        complete(
            &mut panel,
            target,
            Answer::Compared(Box::new(Comparison {
                diff: Default::default(),
                from_recorded_at_ms: 100,
                to_recorded_at_ms: 200,
                from_reason: Key::HistoryReasonManual,
                to_reason: Key::HistoryReasonActive,
                from_label: None,
                to_label: None,
            })),
        );

        assert!(panel.maintain(Some(target), true, now));
        assert!(panel.comparison.is_some());
        assert_eq!(
            panel.comparison_status.as_deref(),
            Some("0 changed blocks · 0 unchanged")
        );
    }

    #[test]
    fn a_probe_answer_is_handed_over_once() {
        let now = Instant::now();
        let target = target();
        let mut panel = HistoryPanel::default();
        panel.target = Some(target);
        panel.visible = true;
        complete(
            &mut panel,
            target,
            Answer::Probed(Some(RenameTarget {
                key: "entry".into(),
                label: Some("Milestone".into()),
            })),
        );
        panel.maintain(Some(target), true, now);
        assert_eq!(
            panel.take_probe(),
            Some(Some(RenameTarget {
                key: "entry".into(),
                label: Some("Milestone".into()),
            }))
        );
        assert_eq!(panel.take_probe(), None, "an answer is read once");
    }

    #[test]
    fn a_named_entry_shows_its_name_instead_of_its_reason() {
        let mut panel = HistoryPanel::default();
        panel.entries = vec![
            Entry {
                key: "named".into(),
                reason: Key::HistoryReasonManual,
                recorded_at_ms: 0,
                label: Some("Milestone".into()),
            },
            Entry {
                key: "plain".into(),
                reason: Key::HistoryReasonManual,
                recorded_at_ms: 0,
                label: None,
            },
        ];
        panel.rebuild_rows(0);
        assert!(panel.rows[0].label.starts_with("Milestone ·"));
        assert!(panel.rows[1].label.starts_with("Saved ·"));
    }

    #[test]
    fn refreshed_rows_preserve_comparison_status() {
        let now = Instant::now();
        let target = target();
        let mut panel = HistoryPanel::default();
        panel.target = Some(target);
        panel.visible = true;
        panel.comparison_status = Some("2 changed blocks".into());
        complete(&mut panel, target, Answer::Rows(vec![entry("new")]));

        assert!(panel.maintain(Some(target), true, now));
        assert_eq!(
            panel
                .rows
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            ["new"]
        );
        assert_eq!(panel.comparison_status.as_deref(), Some("2 changed blocks"));
    }

    #[test]
    fn stale_results_do_not_leak_between_documents() {
        let now = Instant::now();
        let old = target();
        let new = (old.0, DocumentId::new());
        let mut panel = HistoryPanel::default();
        panel.target = Some(old);
        panel.visible = true;
        complete(&mut panel, old, Answer::Rows(vec![entry("old")]));

        panel.maintain(Some(new), true, now);
        assert!(panel.rows.is_empty());
        assert!(!panel.rows.iter().any(|row| row.key == "old"));
    }
}
