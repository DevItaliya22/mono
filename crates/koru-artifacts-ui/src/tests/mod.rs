// SPDX-License-Identifier: MPL-2.0

//! The column's deterministic states, driven without a gateway.
//!
//! Every test here builds a [`Catalog`] by hand and pushes it in through
//! [`ArtifactsSurface::take_catalog`] — the same seam the reader uses — so
//! nothing in this suite dials a store, opens a socket or touches a disk.
//! `dependency` pins that.

use std::collections::HashSet;

use koru_artifact_wire::run_folder::{RelativePath, RunFolder, UnregisteredFile};
use koru_artifact_wire::{
    ArtifactFamily, ArtifactFamilyId, ArtifactTitle, ArtifactVersion, ArtifactVersionId, BlobHash,
    Filename, LinkRole, MediaType, Ordinal, ProducedBy, Provenance, RegistrationId, RunId, RunLink,
    WorkspaceId,
};
use koru_draw::geometry::Rect;
use uuid::Uuid;

use crate::model::Catalog;
use crate::{ArtifactsSurface, Row};

mod dependency;
mod locales;
mod ratio;
mod reading;
mod register;
mod rows_inspection;
mod states;

/// The column, as the navigator hands it over: 260 wide, rows starting a
/// little under the mode row.
pub(crate) const TOP: f32 = 80.0;

pub(crate) fn area() -> Rect {
    Rect::new(0.0, 0.0, 260.0, 720.0)
}

/// A tall column, so a test that means to see every row is not looking at
/// the cull.
pub(crate) fn tall() -> Rect {
    Rect::new(0.0, 0.0, 260.0, 100_000.0)
}

/// **The number goes in the first group**, so two ids differ where a
/// person reads them: a run is named by the first characters of its id, and
/// fixtures that share a prefix would make a test pass on ids that all
/// print the same.
pub(crate) fn v4(n: u32) -> Uuid {
    format!("{n:08x}-0000-4000-8000-000000000000")
        .parse()
        .unwrap()
}

pub(crate) fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(v4(0xa0))
}

pub(crate) fn run(n: u32) -> RunId {
    RunId::from_uuid(v4(0x1000 + n))
}

pub(crate) fn blob(n: u32) -> BlobHash {
    BlobHash::from_digest("b3", &format!("{n:04x}").repeat(16)).unwrap()
}

pub(crate) fn family(n: u32, title: &str, created_at_ms: i64) -> ArtifactFamily {
    ArtifactFamily {
        id: ArtifactFamilyId::from_uuid(v4(0x2000 + n)),
        workspace: workspace(),
        title: ArtifactTitle::new(title).unwrap(),
        created_by_run: Some(run(1)),
        created_at_ms,
        retired_at_ms: None,
    }
}

pub(crate) fn version(
    family: &ArtifactFamily,
    ordinal: u32,
    by: Option<RunId>,
    derived_from: Option<ArtifactVersionId>,
) -> ArtifactVersion {
    ArtifactVersion {
        id: ArtifactVersionId::from_uuid(v4(0x30000 + ordinal)),
        family: family.id,
        ordinal: Ordinal::from_raw(ordinal).unwrap(),
        blob: blob(ordinal),
        size_bytes: 1_024 * u64::from(ordinal),
        mime: MediaType::new("text/markdown").unwrap(),
        filename: Some(Filename::new("findings.md").unwrap()),
        produced_by: by.map(|run| ProducedBy {
            run,
            generation: koru_artifact_wire::AttemptGeneration::FIRST,
        }),
        registration_id: RegistrationId::from_uuid(v4(0x40000 + ordinal)),
        derived_from,
        created_at_ms: 1_000 * i64::from(ordinal),
    }
}

/// A family with `count` versions, oldest first as the store answers them.
pub(crate) fn history(family: &ArtifactFamily, count: u32) -> Vec<ArtifactVersion> {
    let mut versions = Vec::new();
    let mut previous = None;
    for ordinal in 1..=count {
        let made = version(family, ordinal, Some(run(1)), previous);
        previous = Some(made.id);
        versions.push(made);
    }
    versions
}

pub(crate) fn path(spelling: &str) -> RelativePath {
    RelativePath::new(spelling).unwrap()
}

pub(crate) fn unregistered(spelling: &str, size_bytes: u64) -> UnregisteredFile {
    UnregisteredFile {
        path: path(spelling),
        size_bytes,
        modified_at_ms: Some(4_000),
    }
}

pub(crate) fn folder(run: RunId, registered: usize, files: Vec<UnregisteredFile>) -> RunFolder {
    RunFolder {
        run,
        workspace: workspace(),
        folder_present: true,
        registered,
        unregistered: files,
        truncated: false,
    }
}

/// Every blob a catalog names, so nothing reads as *elsewhere* by default.
pub(crate) fn all_present(families: &[(ArtifactFamily, Vec<ArtifactVersion>)]) -> HashSet<String> {
    families
        .iter()
        .flat_map(|(_, versions)| versions.iter().map(|v| v.blob.as_str().to_owned()))
        .collect()
}

/// A surface showing `catalog`, with a fixed clock so every relative time
/// is a golden.
pub(crate) fn showing(catalog: Catalog) -> ArtifactsSurface {
    let mut surface = ArtifactsSurface::without_reader();
    surface.set_now(10_000_000);
    surface.take_catalog(Ok(catalog));
    surface
}

/// The provenance walk the gateway would answer for `subject`.
pub(crate) fn walk(
    subject: &ArtifactVersion,
    ancestors: Vec<ArtifactVersion>,
    inputs: Vec<RunId>,
) -> Provenance {
    let mut runs: Vec<RunLink> = Vec::new();
    if let Some(by) = subject.produced_by {
        runs.push(RunLink {
            run: by.run,
            version: subject.id,
            role: LinkRole::Output,
            linked_at_ms: subject.created_at_ms,
        });
    }
    for run in inputs {
        runs.push(RunLink {
            run,
            version: subject.id,
            role: LinkRole::Input,
            linked_at_ms: subject.created_at_ms,
        });
    }
    Provenance {
        subject: subject.clone(),
        ancestors,
        runs,
        deleted_runs: Default::default(),
    }
}

/// Every row the column has, ignoring the band — what a test asserting
/// *what is shown* wants.
pub(crate) fn rows_of(surface: &mut ArtifactsSurface) -> Vec<Row> {
    surface
        .rows(tall(), TOP)
        .into_iter()
        .map(|(row, _)| row)
        .collect()
}
