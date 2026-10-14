// SPDX-License-Identifier: MPL-2.0

//! Deleting a run: the one question the trash asks, and what emptying it
//! did to the folder the run worked in.
//!
//! Both are vocabulary rather than machinery. The ledger stores the answer,
//! the wire carries it, the orchestrator acts on it and a surface says what
//! happened — four crates that never see each other and must not each have
//! their own word for *keep* or for *we left that folder alone*.

use serde::{Deserialize, Serialize};

/// **What a person said should happen to a deleted run's results.**
///
/// A run that produced nothing is never asked. A run that did is, once,
/// when it goes into the trash — and the answer waits there with it, because
/// trashing changes nothing in the artifact store and emptying the trash is
/// what acts.
///
/// **Keep is the default and the safe one.** A run with no answer recorded
/// keeps what it produced: a build that predates the question, a recording
/// that was lost, a run that arrived by sync. Nothing infers *delete*.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ArtifactDisposition {
    /// The results stay in the Artifacts column with their provenance
    /// intact. That provenance names a run that no longer exists, and the
    /// Artifact Store records the deletion so the sentence says the run was
    /// deleted rather than printing an id nothing resolves.
    #[default]
    Keep,
    /// The results go with the run: every family the run created is
    /// retired. Their versions stay — a version is immutable and the store
    /// retires rather than erases — but the column stops offering them.
    Delete,
}

impl ArtifactDisposition {
    pub const ALL: [Self; 2] = [Self::Keep, Self::Delete];

    /// The stable token: the database's `CHECK`ed value and the wire's.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Delete => "delete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|disposition| disposition.as_str() == value)
    }

    pub fn deletes(self) -> bool {
        matches!(self, Self::Delete)
    }
}

/// **What emptying the trash did to a run's working folder.**
///
/// The trash says what it did, because an operation that quietly does less
/// than it offered teaches a person that it did more. Three answers, and
/// the middle one is the important one: a run scoped to a project ran
/// inside a folder that was the person's before it started and is theirs
/// after, so the run is forgotten and the directory is left exactly alone.
///
/// A folder the filesystem refused to remove is not here: that refuses the
/// whole deletion, so the run stays in the trash and can be deleted again.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FolderOutcome {
    /// Koru made it, and it is gone.
    Removed,
    /// **The person's own project folder, left alone.** Not silence: this
    /// is what the trash tells them it did.
    Kept,
    /// Koru made it and it was not there — an earlier delete, a tidied
    /// temporary directory, a machine that lost it.
    Absent,
}

impl FolderOutcome {
    pub const ALL: [Self; 3] = [Self::Removed, Self::Kept, Self::Absent];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Kept => "kept",
            Self::Absent => "absent",
        }
    }
}
