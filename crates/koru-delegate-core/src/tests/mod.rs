// SPDX-License-Identifier: MPL-2.0

//! Pure unit tests. Nothing here touches disk, a store, a process or a
//! network; the dependency test shells out to `cargo tree` and nothing
//! else.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::*;

mod boundary;
mod dependency;
mod economics;
mod ids;
mod name;
mod privacy;
#[cfg(feature = "schema")]
mod schema;
mod scope;
mod serde;
mod status;
mod timeline;
mod tool_failures;

/// A deterministic v4 UUID — version nibble 4, variant 8 — so strict ids
/// accept it and a fixture is the same on every run.
pub(crate) fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

pub(crate) fn run_id(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

pub(crate) fn attempt_id(n: u8) -> RunAttemptId {
    RunAttemptId::from_uuid(v4(n))
}

pub(crate) fn turn_id(n: u8) -> AgentTurnId {
    v4(n).hyphenated().to_string().parse().unwrap()
}

pub(crate) fn session_id(n: u8) -> SessionId {
    v4(n).hyphenated().to_string().parse().unwrap()
}

pub(crate) fn device_id(n: u8) -> DeviceId {
    DeviceId::from_uuid(v4(n))
}

pub(crate) fn blob(n: u8) -> BlobHash {
    BlobHash::from_digest("b3", &format!("{n:02x}").repeat(32)).unwrap()
}

pub(crate) const OBJECTIVE_SENTINEL: &str = "OBJECTIVE-SENTINEL do the thing";
pub(crate) const NOTE_SENTINEL: &str = "NOTE-SENTINEL it was fine";
pub(crate) const TITLE_SENTINEL: &str = "TITLE-SENTINEL report.md";
pub(crate) const FOLDER_SENTINEL: &str = "FOLDER-SENTINEL/src/private";

pub(crate) fn request() -> DelegationRequest {
    DelegationRequest {
        run: run_id(1),
        requested_by: ActorRef::Person {
            profile: ProfileId::from_uuid(v4(2)),
        },
        objective: Objective::new(OBJECTIVE_SENTINEL).unwrap(),
        agent: SessionProfileKey::BuiltIn("codex".parse().unwrap()),
        requested_model: None,
        scope: WorkspaceScope::ProjectFolder {
            project: v4(3).hyphenated().to_string().parse().unwrap(),
            folder: RelativeFolder::new(FOLDER_SENTINEL).unwrap(),
        },
        context: vec![
            RunContextItem::Document {
                workspace: WorkspaceId::from_uuid(v4(4)),
                document: DocumentId::from_uuid(v4(5)),
                version: DocumentVersionId::from_uuid(v4(6)),
            },
            RunContextItem::Block {
                workspace: WorkspaceId::from_uuid(v4(4)),
                document: DocumentId::from_uuid(v4(5)),
                version: DocumentVersionId::from_uuid(v4(6)),
                node: BlockNodeId::from_uuid(v4(7)),
            },
            RunContextItem::Blob {
                workspace: WorkspaceId::from_uuid(v4(4)),
                blob: blob(8),
                blob_kind: BlobKind::Pdf,
            },
        ],
        grants: BTreeSet::from([WorkspaceId::from_uuid(v4(4))]),
        executing_device: device_id(9),
        origin: Default::default(),
    }
}

pub(crate) fn header() -> RunHeader {
    let mut header = RunHeader::from_request(request(), 1_000);
    header.artifacts.push(ResultReference {
        artifact: ArtifactVersionId::from_uuid(v4(10)),
        held_by: device_id(9),
        title: ArtifactTitle::new(TITLE_SENTINEL).unwrap(),
        size_bytes: 4_096,
        registered_at_ms: 5_000,
    });
    header.rating = Some(RunRating {
        rating: Rating::Good,
        note: Some(RatingNote::new(NOTE_SENTINEL).unwrap()),
        rated_by: ActorRef::Person {
            profile: ProfileId::from_uuid(v4(2)),
        },
        rated_at_ms: 9_000,
    });
    header.status = RunStatus::Completed;
    header
}

pub(crate) fn entry(kind: StepKind, at_ms: i64) -> TimelineEntry {
    TimelineEntry::new(kind, TimelineRefs::default(), at_ms)
}
