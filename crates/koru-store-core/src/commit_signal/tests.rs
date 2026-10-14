// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Signal(Mutex<Vec<(WorkspaceId, DocumentId)>>);
impl CommitSignal for Signal {
    fn committed(&self, workspace: WorkspaceId, document: DocumentId) {
        self.0.lock().unwrap().push((workspace, document));
    }
}

fn record(seq: u64) -> Committed {
    Committed {
        workspace: WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
        document: DocumentId::from_uuid(uuid::Uuid::from_u128(2)),
        seq,
        version_vector: vec![],
        is_manifest: false,
    }
}

#[test]
fn wakes_do_not_wait_for_or_consume_the_projection_feed() {
    let signal = Arc::new(Signal::default());
    let mut commits = Commits {
        signal: Some(signal.clone()),
        ..Default::default()
    };
    for seq in 1..=2 {
        let value = record(seq);
        commits.insert((value.workspace, value.document), value);
    }
    assert_eq!(signal.0.lock().unwrap().len(), 2);
    assert_eq!(commits.take(), vec![record(2)]);
    assert!(commits.take().is_empty());
}

#[test]
fn a_panicking_signal_cannot_lose_a_durable_record() {
    struct Broken;
    impl CommitSignal for Broken {
        fn committed(&self, _: WorkspaceId, _: DocumentId) {
            panic!("observer failed");
        }
    }
    let mut commits = Commits {
        signal: Some(Arc::new(Broken)),
        ..Default::default()
    };
    let value = record(1);
    commits.insert((value.workspace, value.document), value.clone());
    assert_eq!(commits.take(), vec![value]);
}
