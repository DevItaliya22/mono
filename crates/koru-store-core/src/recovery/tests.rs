// SPDX-License-Identifier: MPL-2.0

//! The recovery *policy*, driven against an in-memory repository.
//!
//! A fake rather than SQLite, for two reasons: core cannot depend on
//! `koru-store-sqlite` (it is the other way round), and damage is far easier to
//! stage here — corrupting a `Vec<u8>` in a map takes one line, where the same
//! thing through SQL is a migration's worth of setup. The end-to-end proof,
//! that a real database recovers and *opens*, lives in `koru-store-sqlite`.

use super::*;
use std::collections::HashMap;

use crate::ids::WorkspaceId;
use crate::lifecycle::LiveDocument;
use crate::manifest::Manifest;
use crate::model::{Blob, BlobHash, DocumentKind, Timestamp};
use crate::ports::{
    Checkpoint, CheckpointHeader, DocumentState, StoredUpdate, UpdateBatch, UpdateOrigin,
};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000 + ms)
}

// ------------------------------------------------------------------ the fake

#[derive(Default)]
struct FakeRepo {
    states: HashMap<DocumentId, DocumentState>,
    checkpoints: HashMap<DocumentId, Vec<Checkpoint>>,
    updates: HashMap<DocumentId, Vec<StoredUpdate>>,
}

impl FakeRepo {
    fn add_checkpoint(&mut self, id: DocumentId, cp: Checkpoint) {
        if let Some(s) = self.states.get_mut(&id) {
            s.latest_checkpoint_seq = Some(cp.seq);
        }
        self.checkpoints.entry(id).or_default().push(cp);
    }

    fn add_update(&mut self, id: DocumentId, seq: u64, bytes: Vec<u8>) {
        self.updates.entry(id).or_default().push(StoredUpdate {
            seq,
            bytes,
            received_at: at(0),
            origin: UpdateOrigin::Store,
        });
    }

    /// Rot a checkpoint's bytes, leaving its recorded checksum alone — which is
    /// exactly what a bad sector looks like from here.
    fn damage_checkpoint(&mut self, id: DocumentId, seq: u64) {
        for cp in self.checkpoints.get_mut(&id).unwrap() {
            if cp.seq == seq {
                cp.snapshot = vec![0; cp.snapshot.len()];
            }
        }
    }
}

impl WorkspaceRepo for FakeRepo {
    fn documents(&self) -> Result<Vec<DocumentState>> {
        Ok(self.states.values().cloned().collect())
    }
    fn document(&self, id: DocumentId) -> Result<Option<DocumentState>> {
        Ok(self.states.get(&id).cloned())
    }
    fn create_document(&mut self, id: DocumentId, kind: DocumentKind, at: Timestamp) -> Result<()> {
        self.states.insert(
            id,
            DocumentState {
                id,
                kind,
                latest_checkpoint_seq: None,
                version_vector: Vec::new(),
                frontiers: Vec::new(),
                updated_at: at,
            },
        );
        Ok(())
    }
    fn drop_document(&mut self, id: DocumentId) -> Result<()> {
        self.states.remove(&id);
        self.checkpoints.remove(&id);
        self.updates.remove(&id);
        Ok(())
    }
    fn checkpoints(&self, id: DocumentId) -> Result<Vec<CheckpointHeader>> {
        let mut out: Vec<CheckpointHeader> = self
            .checkpoints
            .get(&id)
            .map(|v| {
                v.iter()
                    .map(|c| CheckpointHeader {
                        seq: c.seq,
                        size: c.snapshot.len() as u64,
                        checksum: c.checksum.clone(),
                        created_at: c.created_at,
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| b.seq.cmp(&a.seq));
        Ok(out)
    }
    fn checkpoint(&self, id: DocumentId, seq: u64) -> Result<Option<Checkpoint>> {
        Ok(self
            .checkpoints
            .get(&id)
            .and_then(|v| v.iter().find(|c| c.seq == seq).cloned()))
    }
    fn latest_sound_checkpoint(&self, id: DocumentId) -> Result<Option<Checkpoint>> {
        let mut all = self.checkpoints.get(&id).cloned().unwrap_or_default();
        all.sort_by(|a, b| b.seq.cmp(&a.seq));
        Ok(all
            .into_iter()
            .find(|c| checksum(&c.snapshot) == c.checksum))
    }
    fn updates_after(&self, id: DocumentId, after: Option<u64>) -> Result<Vec<StoredUpdate>> {
        let mut out: Vec<StoredUpdate> = self
            .updates
            .get(&id)
            .map(|v| {
                v.iter()
                    .filter(|u| u.seq > after.unwrap_or(0))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|u| u.seq);
        Ok(out)
    }
    fn latest_update_origin(
        &self,
        id: DocumentId,
    ) -> Result<Option<(crate::model::Timestamp, crate::ports::UpdateOrigin)>> {
        Ok(self
            .updates
            .get(&id)
            .and_then(|v| v.iter().max_by_key(|u| u.seq))
            .map(|u| (u.received_at, u.origin.clone())))
    }
    fn update_count(&self, id: DocumentId) -> Result<u64> {
        Ok(self.updates.get(&id).map(|v| v.len() as u64).unwrap_or(0))
    }
    fn journal_since(
        &self,
        id: DocumentId,
        after: Option<u64>,
    ) -> Result<crate::ports::JournalSince> {
        let mut since = crate::ports::JournalSince::default();
        for update in self.updates.get(&id).into_iter().flatten() {
            if after.is_some_and(|after| update.seq <= after) {
                continue;
            }
            since.head = Some(since.head.map_or(update.seq, |head| head.max(update.seq)));
            since.bytes += update.bytes.len() as u64;
        }
        Ok(since)
    }
    fn commit_updates(&mut self, _batch: &UpdateBatch) -> Result<u64> {
        unimplemented!("the recovery policy only reads")
    }
    fn commit_checkpoint(&mut self, _id: DocumentId, _cp: &Checkpoint) -> Result<usize> {
        unimplemented!("the recovery policy only reads")
    }
    fn put_blob(&mut self, _blob: &Blob) -> Result<()> {
        Ok(())
    }
    fn blob(&self, _hash: &BlobHash) -> Result<Option<Blob>> {
        Ok(None)
    }
    fn blobs(&self) -> Result<Vec<Blob>> {
        Ok(Vec::new())
    }
    fn size_bytes(&self) -> Result<u64> {
        Ok(0)
    }
}

// ------------------------------------------------------------------ fixtures

/// A manifest, a snapshot of it, and updates that carry it further.
struct Story {
    id: DocumentId,
    snapshot: Vec<u8>,
    updates: Vec<Vec<u8>>,
    /// How many documents the manifest holds once every update has applied.
    final_documents: usize,
}

fn story() -> Story {
    let id = DocumentId::new();
    let mut manifest = Manifest::create(WorkspaceId::new(), "W", at(0)).unwrap();
    manifest
        .add_document(DocumentId::new(), "Plan trimestriel", "page", None)
        .unwrap();
    let snapshot = manifest.export_snapshot().unwrap();

    let mut updates = Vec::new();
    for title in ["الخطة الفصلية", "四半期計画", "Quartalsplanung"] {
        let before = manifest.version();
        manifest
            .add_document(DocumentId::new(), title, "page", None)
            .unwrap();
        updates.push(manifest.export_updates_since(&before).unwrap());
    }
    Story {
        id,
        snapshot,
        updates,
        final_documents: 4,
    }
}

/// A whole history as successive deltas, the first starting from **nothing**.
///
/// The trap this exists to avoid: a delta exported from a non-empty version
/// names causal ancestors its receiver does not have, and Loro parks it as
/// pending — `import` returns `Ok` and applies nothing. A journal built that
/// way replays into an empty document and looks like data loss.
///
/// The real store never hits this, because a client syncing against a document
/// the store has never seen is asked for everything from an empty version
/// vector. A test writing a journal by hand has to do the same.
fn history_from_nothing(titles: &[&str]) -> Vec<Vec<u8>> {
    let mut manifest = Manifest::create(WorkspaceId::new(), "W", at(0)).unwrap();
    let mut prev = loro::VersionVector::new();
    let mut out = Vec::new();
    for title in titles {
        manifest
            .add_document(DocumentId::new(), title, "page", None)
            .unwrap();
        out.push(manifest.export_updates_since(&prev).unwrap());
        prev = manifest.version();
    }
    out
}

fn documents_in(live: &LiveDocument) -> usize {
    Manifest::from_snapshot(&live.export_snapshot().unwrap())
        .unwrap()
        .entries()
        .len()
}

fn seeded() -> (FakeRepo, Story) {
    let s = story();
    let mut repo = FakeRepo::default();
    repo.create_document(s.id, DocumentKind::Manifest, at(0))
        .unwrap();
    (repo, s)
}

// ------------------------------------------------------------------ the cases

#[test]
fn an_unknown_document_opens_fresh_and_empty() {
    let repo = FakeRepo::default();
    let opened = open_document(&repo, DocumentId::new()).unwrap();
    assert!(matches!(opened, Opened::Fresh(_)));
    assert!(opened.recovery().is_none());
}

#[test]
fn a_created_but_unwritten_document_opens_normally() {
    // Empty is correct here, and correct is not a recovery.
    let (repo, s) = seeded();
    let opened = open_document(&repo, s.id).unwrap();
    assert!(matches!(opened, Opened::Normal(_)));
    assert_eq!(documents_in(opened.value()), 0);
}

#[test]
fn a_checkpoint_and_its_journal_reconstruct_the_document() {
    let (mut repo, s) = seeded();
    let live = LiveDocument::from_snapshot(s.id, &s.snapshot).unwrap();
    repo.add_checkpoint(s.id, live.checkpoint(1, at(0)).unwrap());
    for (i, u) in s.updates.iter().enumerate() {
        repo.add_update(s.id, 2 + i as u64, u.clone());
    }

    let opened = open_document(&repo, s.id).unwrap();
    assert!(
        matches!(opened, Opened::Normal(_)),
        "{:?}",
        opened.recovery()
    );
    assert_eq!(documents_in(opened.value()), s.final_documents);
}

#[test]
fn the_journal_alone_rebuilds_a_document_with_no_checkpoint() {
    // The state before the first checkpoint is a whole document, not a
    // degenerate case.
    let (mut repo, s) = seeded();
    for (i, update) in history_from_nothing(&["A", "B"]).into_iter().enumerate() {
        repo.add_update(s.id, 1 + i as u64, update);
    }

    let opened = open_document(&repo, s.id).unwrap();
    assert!(matches!(opened, Opened::Normal(_)));
    assert_eq!(documents_in(opened.value()), 2);
}

#[test]
fn a_damaged_newest_checkpoint_falls_back_to_an_older_one() {
    let (mut repo, s) = seeded();
    let live = LiveDocument::from_snapshot(s.id, &s.snapshot).unwrap();
    repo.add_checkpoint(s.id, live.checkpoint(1, at(0)).unwrap());

    // A second, newer checkpoint that then rots.
    let mut later = LiveDocument::from_snapshot(s.id, &s.snapshot).unwrap();
    for u in &s.updates {
        later.import(u).unwrap();
    }
    repo.add_checkpoint(s.id, later.checkpoint(4, at(10)).unwrap());
    // The journal above the *older* checkpoint is still there, which is what
    // makes the fall-back lossless.
    for (i, u) in s.updates.iter().enumerate() {
        repo.add_update(s.id, 2 + i as u64, u.clone());
    }
    repo.damage_checkpoint(s.id, 4);

    let opened = open_document(&repo, s.id).unwrap();
    let recovery = opened.recovery().expect("a fall-back is a recovery");
    assert_eq!(recovery.from_checkpoint, Some(1));
    assert_eq!(recovery.skipped_updates, 0);
    assert!(
        recovery.problem.contains("checkpoint 4"),
        "{}",
        recovery.problem
    );
    // Nothing damaged was deleted — it is named as evidence.
    assert_eq!(recovery.quarantined, vec!["checkpoint 4"]);
    assert!(repo.checkpoint(s.id, 4).unwrap().is_some());

    // And it is lossless: the journal made up the difference.
    assert_eq!(documents_in(opened.value()), s.final_documents);
}

#[test]
fn every_checkpoint_damaged_rebuilds_from_the_journal() {
    let (mut repo, s) = seeded();
    let live = LiveDocument::from_snapshot(s.id, &s.snapshot).unwrap();
    repo.add_checkpoint(s.id, live.checkpoint(1, at(0)).unwrap());
    repo.damage_checkpoint(s.id, 1);

    // The whole history is in the journal, from the beginning.
    for (i, update) in history_from_nothing(&["A", "B", "C"])
        .into_iter()
        .enumerate()
    {
        repo.add_update(s.id, 1 + i as u64, update);
    }

    let opened = open_document(&repo, s.id).unwrap();
    let recovery = opened.recovery().expect("a rebuild is a recovery");
    assert_eq!(recovery.from_checkpoint, None);
    assert!(
        recovery.problem.contains("no earlier one survived"),
        "{}",
        recovery.problem
    );
    assert_eq!(documents_in(opened.value()), 3);
}

#[test]
fn a_replay_stops_at_a_bad_update_rather_than_stepping_over_it() {
    // The rule that separates *behind* from *wrong*. Loro's history is causal,
    // so applying update 3 after skipping update 2 would build a document from
    // a history that never happened — and syncing cannot repair that.
    let (mut repo, s) = seeded();
    let live = LiveDocument::from_snapshot(s.id, &s.snapshot).unwrap();
    repo.add_checkpoint(s.id, live.checkpoint(1, at(0)).unwrap());
    repo.add_update(s.id, 2, s.updates[0].clone());
    repo.add_update(s.id, 3, b"torn".to_vec());
    repo.add_update(s.id, 4, s.updates[2].clone());

    let opened = open_document(&repo, s.id).unwrap();
    let recovery = opened.recovery().expect("a stopped replay is a recovery");
    assert_eq!(recovery.from_checkpoint, Some(1));
    assert_eq!(
        recovery.skipped_updates, 2,
        "update 4 must not have been applied"
    );
    assert!(
        recovery.problem.contains("update 3"),
        "{}",
        recovery.problem
    );
    // The checkpoint plus the one good update: a prefix of the truth.
    assert_eq!(documents_in(opened.value()), 2);
}

#[test]
fn a_recovered_document_is_still_usable() {
    // The point of returning the document alongside the recovery: a partial
    // document a person can read beats a refusal to open.
    let (mut repo, s) = seeded();
    let live = LiveDocument::from_snapshot(s.id, &s.snapshot).unwrap();
    repo.add_checkpoint(s.id, live.checkpoint(1, at(0)).unwrap());
    repo.add_update(s.id, 2, b"torn".to_vec());

    let opened = open_document(&repo, s.id).unwrap();
    assert!(opened.recovery().is_some());
    let recovered = opened.into_value();
    assert!(recovered.export_snapshot().is_ok());
    assert_eq!(documents_in(&recovered), 1);
}
