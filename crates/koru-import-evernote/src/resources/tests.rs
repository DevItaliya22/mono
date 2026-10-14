// SPDX-License-Identifier: MPL-2.0

use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::engine::general_purpose::STANDARD;
use koru_import::{ImportProgress, ImportStage};

use super::*;

fn enex(payloads: &[String]) -> Vec<u8> {
    let mut xml = String::from(
        "<en-export><note><title>resources</title><content>&lt;en-note/&gt;</content>",
    );
    for payload in payloads {
        xml.push_str("<resource><data encoding=\"base64\">");
        xml.push_str(payload);
        xml.push_str("</data><mime>application/octet-stream</mime></resource>");
    }
    xml.push_str("</note></en-export>");
    xml.into_bytes()
}

#[test]
fn one_large_data_text_decodes_and_writes_in_bounded_chunks() {
    let source_bytes = vec![0x5a; 3 * 1024 * 1024];
    let xml = enex(&[STANDARD.encode(&source_bytes)]);
    let (mut spools, probe) = SpoolSet::memory();

    let resources =
        spool_resource_data(Cursor::new(xml), Path::new("large.enex"), None, &mut spools).unwrap();

    assert_eq!(resources.len(), 1);
    assert!(!resources[0].invalid);
    assert_eq!(
        resources[0].md5,
        Some(format!("{:x}", md5::compute(&source_bytes)))
    );
    assert!(probe.max_write() <= 64 * 1024);
    assert_eq!(probe.live(), 1);
    drop(spools);
    assert_eq!(probe.live(), 0);
}

#[test]
fn bad_base64_is_resource_local_and_leaves_no_finished_spool() {
    let xml = enex(&["%%%not-base64%%%".into(), STANDARD.encode(b"sibling")]);
    let (mut spools, probe) = SpoolSet::memory();

    let resources =
        spool_resource_data(Cursor::new(xml), Path::new("bad.enex"), None, &mut spools).unwrap();

    assert!(resources[0].invalid);
    assert!(!resources[1].invalid);
    assert_eq!(probe.live(), 1);
    assert_eq!(probe.pending(), 0);
}

#[test]
fn failing_third_spool_is_a_job_error_and_drop_cleans_the_first_two() {
    let xml = enex(&[
        STANDARD.encode(b"first"),
        STANDARD.encode(b"second"),
        STANDARD.encode(b"third"),
    ]);
    let (mut spools, probe) = SpoolSet::memory_failing(Some(2));

    let error = spool_resource_data(Cursor::new(xml), Path::new("full.enex"), None, &mut spools)
        .unwrap_err();

    assert!(error.to_string().contains("disk full"));
    assert_eq!(probe.live(), 2);
    assert_eq!(probe.pending(), 0);
    drop(spools);
    assert_eq!(probe.live(), 0);
}

#[test]
fn write_failure_aborts_the_job_and_cleans_prior_and_current_spools() {
    let xml = enex(&[
        STANDARD.encode(b"first"),
        STANDARD.encode(b"second"),
        STANDARD.encode(b"third"),
    ]);
    // Each short payload flushes once during `push` and once at `finish`, so
    // write four is the first write of the third resource.
    let (mut spools, probe) = SpoolSet::memory_write_failing(Some(4));

    let error = spool_resource_data(
        Cursor::new(xml),
        Path::new("write-full.enex"),
        None,
        &mut spools,
    )
    .unwrap_err();

    assert!(error.to_string().contains("write disk full"));
    assert!(error.to_string().contains("write-full.enex"));
    assert_eq!(
        probe.live(),
        2,
        "the two prior completed spools remain owned"
    );
    assert_eq!(probe.pending(), 0, "the failing current spool was dropped");
    drop(spools);
    assert_eq!(
        probe.live(),
        0,
        "dropping the stage cleans prior spools too"
    );
}

struct CancelAfter {
    polls: AtomicUsize,
    after: usize,
}

impl ImportProgress for CancelAfter {
    fn update(&self, _: ImportStage, _: usize, _: Option<usize>) {}

    fn cancelled(&self) -> bool {
        self.polls.fetch_add(1, Ordering::SeqCst) >= self.after
    }
}

#[test]
fn cancellation_during_data_chunks_drops_the_partial_spool() {
    let xml = enex(&[STANDARD.encode(vec![7_u8; 2 * 1024 * 1024])]);
    let progress = CancelAfter {
        polls: AtomicUsize::new(0),
        after: 8,
    };
    let (mut spools, probe) = SpoolSet::memory();

    let error = spool_resource_data(
        Cursor::new(xml),
        Path::new("cancel.enex"),
        Some(&progress),
        &mut spools,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "import cancelled");
    assert_eq!(probe.live(), 0);
    assert_eq!(probe.pending(), 0);
}

#[test]
fn signatures_are_recorded_without_trusting_the_declared_mime() {
    let xml = enex(&[STANDARD.encode(b"%PDF-1.7\nbody")]);
    let (mut spools, _) = SpoolSet::memory();

    let resources =
        spool_resource_data(Cursor::new(xml), Path::new("mime.enex"), None, &mut spools).unwrap();

    assert_eq!(resources[0].detected_mime, Some("application/pdf"));
}

#[test]
fn decoded_limit_is_enforced_before_an_over_limit_chunk_is_written() {
    let (mut spools, probe) = SpoolSet::memory();
    let pending = spools.begin().unwrap();
    let mut decoder = ChunkDecoder::new(pending);
    decoder.total = MAX_IMPORT_BLOB_BYTES;

    let error = decoder.push(b"QUJDREVG", &spools).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(probe.max_write(), 0);
}
