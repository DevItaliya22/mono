// SPDX-License-Identifier: MPL-2.0

//! Provenance: which run consumed or produced which version, and the chain
//! of `derived_from` a version stands at the end of.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{ArtifactVersion, ArtifactVersionId, RunId};

/// How a run touched a version: read it as an input, or produced it as an
/// output. A closed domain, `CHECK`ed by the database as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinkRole {
    Input,
    Output,
}

impl LinkRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "input" => Some(Self::Input),
            "output" => Some(Self::Output),
            _ => None,
        }
    }
}

/// One link between a run and a version. `(run, version, role)` is the key:
/// a run that reads the same version twice has one input link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunLink {
    pub run: RunId,
    pub version: ArtifactVersionId,
    pub role: LinkRole,
    pub linked_at_ms: i64,
}

/// A version's provenance: the version, every ancestor reached by following
/// `derived_from` (nearest first), and every run link on any of them.
///
/// Three generations read like a sentence: `v3` was produced by run *R2*,
/// which consumed `v2`, which was derived from `v1`, which run *R1*
/// produced. P1.13 draws that sentence; this is the fact it draws from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Provenance {
    pub subject: ArtifactVersion,
    /// Nearest first: `subject.derived_from`, then its `derived_from`, …
    pub ancestors: Vec<ArtifactVersion>,
    /// Links on the subject and on every ancestor, subject's first.
    pub runs: Vec<RunLink>,
    /// **Which of the runs named above no longer exist.**
    ///
    /// A person may delete a run and keep what it produced. The result
    /// stays, and its provenance still names the run that made it — so the
    /// sentence has to say *a run that was deleted* rather than print an id
    /// that resolves to nothing. Recorded when the trash is emptied and
    /// read back here; a run absent from this set is simply a run, whether
    /// it belongs to this device or another one.
    ///
    /// `#[serde(default)]`, so a walk from a store that predates the set
    /// reads as *none deleted* rather than failing to decode.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub deleted_runs: BTreeSet<RunId>,
}

impl Provenance {
    /// How many versions the walk touched: the subject plus its ancestors.
    pub fn generations(&self) -> usize {
        1 + self.ancestors.len()
    }

    /// The subject and its ancestors, nearest first.
    pub fn versions(&self) -> impl Iterator<Item = &ArtifactVersion> {
        std::iter::once(&self.subject).chain(self.ancestors.iter())
    }

    /// The links of one role on one version.
    pub fn links(&self, version: ArtifactVersionId, role: LinkRole) -> Vec<&RunLink> {
        self.runs
            .iter()
            .filter(|link| link.version == version && link.role == role)
            .collect()
    }
}
