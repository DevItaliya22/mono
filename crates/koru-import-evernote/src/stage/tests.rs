// SPDX-License-Identifier: MPL-2.0

use super::*;

use std::io::{Cursor, Read};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use koru_import::{ImportProgress, ImportStage, StoredBlob};

#[test]
fn fresh_system_stays_in_memory_until_stage_is_committed() {
    let mut stage = SourceStage::new("notes.enex".into(), None).expect("stage");
    stage.create_tag("work", 0).expect("staged tag");

    let committed = stage.commit().expect("commit stage");
    let opened = SystemDoc::from_snapshot(
        committed
            .system_snapshot
            .as_deref()
            .expect("changed system snapshot"),
    )
    .expect("system snapshot");

    assert!(committed.register_system);
    assert_eq!(opened.tags()[0].name, "work");
}

#[test]
fn existing_system_is_not_registered_again() {
    let existing = SystemDoc::create().expect("system");
    let snapshot = existing.export_snapshot().expect("snapshot");
    let stage = SourceStage::new("notes.enex".into(), Some(&snapshot)).expect("stage");

    assert!(!stage.commit().expect("commit stage").register_system);
}

#[test]
fn a_source_without_tags_has_no_system_publication() {
    let stage = SourceStage::new("notes.enex".into(), None).expect("stage");
    let committed = stage.commit().expect("commit stage");

    assert!(committed.system_snapshot.is_none());
    assert!(!committed.register_system);
}

#[test]
fn stage_accepts_an_in_memory_spool_backend_without_touching_disk() {
    let (spools, probe) = crate::resources::SpoolSet::memory();
    let stage = SourceStage::new_with_spools("notes.enex".into(), None, spools).expect("stage");

    assert!(stage.commit().is_ok());
    assert_eq!(probe.live(), 0);
}

struct CancelProgress(AtomicBool);

impl ImportProgress for CancelProgress {
    fn update(&self, _: ImportStage, _: usize, _: Option<usize>) {}

    fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

struct CancelDuringPut<'a> {
    progress: &'a CancelProgress,
    puts: Mutex<Vec<Vec<u8>>>,
}

impl BlobSink for CancelDuringPut<'_> {
    fn put(&self, _: Vec<u8>, _: &str, _: Option<&str>) -> Result<StoredBlob, ImportError> {
        unreachable!("resource stages use put_from")
    }

    fn put_from(
        &self,
        factory: &(dyn Fn() -> Box<dyn Read + Send> + Send + Sync),
        mime: &str,
        _: Option<&str>,
    ) -> Result<StoredBlob, ImportError> {
        let mut bytes = Vec::new();
        factory().read_to_end(&mut bytes).unwrap();
        self.puts.lock().unwrap().push(bytes);
        self.progress.0.store(true, Ordering::SeqCst);
        Ok(StoredBlob {
            hash: "completed-in-flight".into(),
            mime: mime.into(),
        })
    }
}

#[test]
fn cancellation_during_a_blob_put_finishes_that_put_then_drops_the_stage() {
    let progress = CancelProgress(AtomicBool::new(false));
    let sink = CancelDuringPut {
        progress: &progress,
        puts: Mutex::new(Vec::new()),
    };
    let (spools, probe) = crate::resources::SpoolSet::memory();
    let mut stage = SourceStage::new_with_spools("cancel.enex".into(), None, spools).unwrap();
    let data = crate::resources::spool_resource_data(
        Cursor::new(
            b"<en-export><note><resource><data encoding=\"base64\">c3RyZWFtZWQ=</data></resource></note></en-export>",
        ),
        Path::new("cancel.enex"),
        None,
        stage.spools_mut(),
    )
    .unwrap();
    let spool = data[0].spool.unwrap();
    stage.create_tag("must-not-publish", 1).unwrap();
    stage.bind_resource(
        koru_store_core::ids::DocumentId::new(),
        NodeId::new(),
        spool,
        "application/octet-stream".into(),
        None,
    );

    let error = match stage.commit_with_blobs_progress(&sink, Some(&progress)) {
        Ok(_) => panic!("cancelled stage committed"),
        Err(error) => error,
    };

    assert_eq!(error.to_string(), "import cancelled");
    assert_eq!(sink.puts.lock().unwrap().as_slice(), [b"streamed"]);
    assert_eq!(probe.live(), 0, "all completed spools were dropped");
    assert_eq!(probe.pending(), 0, "no partial spool survived");
}
