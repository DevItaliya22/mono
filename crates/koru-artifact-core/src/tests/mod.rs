// SPDX-License-Identifier: MPL-2.0

//! Pure unit tests. Nothing here touches disk, a store, a process or a
//! network; the dependency test shells out to `cargo tree` and nothing
//! else.

use uuid::Uuid;

use crate::*;

mod access;
mod authority;
mod dependency;
mod error;
mod ids;
mod privacy;
mod provenance;
#[cfg(feature = "schema")]
mod schema;
mod serde;
mod text;

/// A deterministic v4 UUID — version nibble 4, variant 8 — so strict ids
/// accept it and a fixture is the same on every run.
pub(crate) fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

pub(crate) fn family_id(n: u8) -> ArtifactFamilyId {
    ArtifactFamilyId::from_uuid(v4(n))
}

pub(crate) fn version_id(n: u8) -> ArtifactVersionId {
    ArtifactVersionId::from_uuid(v4(n))
}

pub(crate) fn run_id(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

pub(crate) fn blob(n: u8) -> BlobHash {
    BlobHash::from_digest("b3", &format!("{n:02x}").repeat(32)).unwrap()
}

pub(crate) const TITLE_SENTINEL: &str = "TITLE-SENTINEL quarterly report";
pub(crate) const FILENAME_SENTINEL: &str = "FILENAME-SENTINEL report.md";

pub(crate) fn family() -> ArtifactFamily {
    ArtifactFamily {
        id: family_id(1),
        workspace: WorkspaceId::from_uuid(v4(2)),
        title: ArtifactTitle::new(TITLE_SENTINEL).unwrap(),
        created_by_run: Some(run_id(3)),
        created_at_ms: 1_000,
        retired_at_ms: None,
    }
}

pub(crate) fn version(n: u8, derived_from: Option<ArtifactVersionId>) -> ArtifactVersion {
    ArtifactVersion {
        id: version_id(n),
        family: family_id(1),
        ordinal: Ordinal::from_raw(u32::from(n)).unwrap(),
        blob: blob(n),
        size_bytes: 4_096,
        mime: MediaType::new("text/markdown").unwrap(),
        filename: Some(Filename::new(FILENAME_SENTINEL).unwrap()),
        produced_by: Some(ProducedBy {
            run: run_id(3),
            generation: AttemptGeneration::FIRST,
        }),
        registration_id: RegistrationId::from_uuid(v4(0x40 + n)),
        derived_from,
        created_at_ms: 2_000 + i64::from(n),
    }
}
