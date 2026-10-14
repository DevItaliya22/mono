// SPDX-License-Identifier: MPL-2.0

//! What the surface knows, and nothing about how it is drawn.
//!
//! A **catalog** is one answer from the gateway, folded into the shape the
//! column reads: the workspace's families newest first, each with its
//! versions newest first; the runs of that workspace with what each left in
//! its `.artifacts/<RunId>/` folder; and the set of blob hashes this device
//! actually holds, which is how a version says *the content is elsewhere*
//! (D27) rather than offering something it cannot give.
//!
//! **Newest first is a decision, not an accident.** The store answers by
//! ordinal and by creation, oldest first, because that is the order a
//! history has. What a person opening a result wants is the newest one, and
//! a column that puts version 1 at the top makes them scroll past a hundred
//! of them to find it.

use std::collections::{BTreeMap, HashSet};

use koru_artifact_wire::run_folder::{RunFolder, UnregisteredFile};
use koru_artifact_wire::{
    ArtifactFamily, ArtifactFamilyId, ArtifactVersion, ArtifactVersionId, Provenance, RunId,
};

/// Where the surface's knowledge stands. **Four states and not three**: an
/// empty answer and a gateway that will not answer are different facts, and
/// only one of them is about the person's work.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Load {
    /// Nothing asked for yet — before the column has ever been shown.
    #[default]
    Cold,
    /// Asked, nothing back. Drawn as a state, never as an empty list.
    Loading,
    /// The answer is what is on screen.
    Ready,
    /// The gateway or `artifacts.db` did not answer. What is on screen is
    /// the last thing it said.
    Unavailable,
}

/// One family and the versions under it, newest first.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FamilyEntry {
    pub family: ArtifactFamily,
    /// Newest first — `versions[0]` is the latest.
    pub versions: Vec<ArtifactVersion>,
}

impl FamilyEntry {
    pub fn latest(&self) -> Option<&ArtifactVersion> {
        self.versions.first()
    }

    pub fn version(&self, index: usize) -> Option<&ArtifactVersion> {
        self.versions.get(index)
    }
}

/// One run's folder, as the column reads it (D19, D29, D37).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunEntry {
    pub run: RunId,
    /// `<run-workspace>/.artifacts/<RunId>/` is still there.
    pub folder_present: bool,
    /// How many versions this run has registered. **Zero is D29's state**,
    /// and it is a fact rather than the absence of one.
    pub registered: usize,
    pub files: Vec<UnregisteredFile>,
    /// The folder held more than the scan's bounds allow.
    pub truncated: bool,
}

impl RunEntry {
    /// Whether this run has anything to show at all. A run that registered
    /// something and left nothing behind is not news, and the column does
    /// not draw a heading for it.
    pub fn worth_showing(&self) -> bool {
        !self.files.is_empty() || self.registered == 0
    }
}

/// One answer, folded.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Catalog {
    /// Newest family first.
    pub families: Vec<FamilyEntry>,
    /// Newest run first, and only the ones [`RunEntry::worth_showing`].
    pub runs: Vec<RunEntry>,
    /// The blob hashes this device's blob store holds. A version whose blob
    /// is not in here is named and says the content is elsewhere (D27).
    pub present: HashSet<String>,
    /// Whether the blob listing was answered at all. Without it nothing is
    /// claimed to be elsewhere — a store that did not answer must not turn
    /// every result into a *not here*.
    pub locality_known: bool,
}

impl Catalog {
    /// Fold the gateway's answers into the column's order.
    pub fn build(
        families: Vec<(ArtifactFamily, Vec<ArtifactVersion>)>,
        runs: Vec<RunFolder>,
        present: Option<HashSet<String>>,
    ) -> Self {
        let mut families: Vec<FamilyEntry> = families
            .into_iter()
            .map(|(family, mut versions)| {
                // The store answers by ordinal; the column reads the other
                // way round.
                versions.sort_by(|a, b| b.ordinal.cmp(&a.ordinal));
                FamilyEntry { family, versions }
            })
            .collect();
        families.sort_by(|a, b| {
            b.family
                .created_at_ms
                .cmp(&a.family.created_at_ms)
                .then_with(|| b.family.id.to_storage().cmp(&a.family.id.to_storage()))
        });
        let runs = runs
            .into_iter()
            .map(|folder| RunEntry {
                run: folder.run,
                folder_present: folder.folder_present,
                registered: folder.registered,
                files: folder.unregistered,
                truncated: folder.truncated,
            })
            .filter(RunEntry::worth_showing)
            .collect();
        Self {
            locality_known: present.is_some(),
            present: present.unwrap_or_default(),
            families,
            runs,
        }
    }

    /// Whether this device holds a version's bytes.
    ///
    /// **Unknown is not elsewhere.** Before the blob listing has been
    /// answered every version is treated as here, because claiming a
    /// hundred results are on another device because one request failed
    /// would be a worse lie than the one it is trying to avoid.
    pub fn holds(&self, version: &ArtifactVersion) -> bool {
        !self.locality_known || self.present.contains(version.blob.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.families.is_empty() && self.runs.is_empty()
    }

    pub fn family(&self, id: ArtifactFamilyId) -> Option<&FamilyEntry> {
        self.families.iter().find(|entry| entry.family.id == id)
    }
}

/// What the surface remembers about one version beyond the catalog: its
/// provenance, once asked for.
///
/// Kept beside the catalog rather than inside it because it arrives later
/// and on its own schedule — a refresh must not throw away a sentence the
/// person is reading, and a version's provenance cannot change: it is a
/// walk over immutable rows.
#[derive(Clone, Default)]
pub struct Provenances(BTreeMap<ArtifactVersionId, Provenance>);

impl Provenances {
    pub fn get(&self, version: ArtifactVersionId) -> Option<&Provenance> {
        self.0.get(&version)
    }

    pub fn insert(&mut self, version: ArtifactVersionId, provenance: Provenance) {
        self.0.insert(version, provenance);
    }

    pub fn holds(&self, version: ArtifactVersionId) -> bool {
        self.0.contains_key(&version)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How far a Register has got, per file. The row draws the difference.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Registering {
    /// Sent, no answer yet.
    Sending,
    /// Refused, with the stable code the route answered. The row renders
    /// that code as a sentence in the person's own language; the code
    /// itself is never drawn.
    Refused(String),
}
