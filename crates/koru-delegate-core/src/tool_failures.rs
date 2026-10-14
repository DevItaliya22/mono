// SPDX-License-Identifier: MPL-2.0

//! **Which tools failed in a turn, and why** — bounded, in tokens.
//!
//! `ToolsCompleted` has always said *six tool calls, two of them failed*,
//! which is a number a person can worry about and cannot act on. This is
//! the rest of the sentence, and it is built to be the *only* kind of rest
//! that D45 allows on a synced, permanently retained entry:
//!
//! - **Two closed vocabularies and a count.** [`ToolKind`] and
//!   [`ToolFailure`] are enums an adapter maps its provider onto; neither
//!   is ever the provider's own string, so there is nothing here to redact
//!   and nothing to leak. A provider's error *text* does not reach this
//!   type — it does not reach the session protocol at all.
//! - **A hard size bound, by type.** At most [`MAX_TOOL_FAILURE_KINDS`]
//!   distinct `(tool, failure)` pairs are listed; failures past that are
//!   counted in [`ToolFailures::elided`] so the parts still sum to the
//!   step's `failed`. The whole thing is `Copy` and fits in a few dozen
//!   bytes whether the turn failed twice or sixty times.
//! - **Nothing is dropped and nothing is guessed.** A failure the adapter
//!   could not place arrives as [`ToolFailure::Unclassified`]; a tool it
//!   did not name arrives as [`ToolKind::Other`]. Both are shown as what
//!   they are.

use serde::{Deserialize, Serialize};

pub use koru_session_protocol::{ToolFailure, ToolKind};

/// How many distinct `(tool, failure)` pairs one step lists.
///
/// Eight, for the same reason a step carries at most eight references: a
/// step is a handful of things, and a step that needs more than a handful
/// of rows to explain itself is a report, not a step. The pairs past the
/// eighth are counted, never listed and never lost.
pub const MAX_TOOL_FAILURE_KINDS: usize = 8;

/// One tool, one way it failed, and how many times that happened in the
/// turn.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolFailureTally {
    pub tool: ToolKind,
    pub failure: ToolFailure,
    /// How many calls of this tool failed this way. Never zero in a listed
    /// tally.
    ///
    /// Sixteen bits, because a coalesced step is written at the turn's end
    /// or at the orchestrator's tool bound, whichever comes first — no one
    /// step has ever counted more than a couple of hundred calls, and a
    /// count that somehow did would saturate rather than wrap.
    pub calls: u16,
}

impl ToolFailureTally {
    /// The value an unused slot holds, so two equal breakdowns are equal
    /// and hash alike whatever order they were filled in.
    const EMPTY: Self = Self {
        tool: ToolKind::Other,
        failure: ToolFailure::Unclassified,
        calls: 0,
    };
}

/// **The breakdown of one coalesced tool step**, bounded by type.
///
/// Empty for a turn in which nothing failed — which is most of them — and
/// then it costs nothing on the wire: it serializes to nothing at all.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ToolFailures {
    tallies: [ToolFailureTally; MAX_TOOL_FAILURE_KINDS],
    len: u8,
    elided: u16,
}

impl Default for ToolFailures {
    fn default() -> Self {
        Self {
            tallies: [ToolFailureTally::EMPTY; MAX_TOOL_FAILURE_KINDS],
            len: 0,
            elided: 0,
        }
    }
}

impl ToolFailures {
    /// Count one failed call. A pair already listed is incremented in
    /// place; a new pair takes the next free slot, or is counted in
    /// [`Self::elided`] when there is none.
    pub fn record(&mut self, tool: ToolKind, failure: ToolFailure) {
        for tally in &mut self.tallies[..usize::from(self.len)] {
            if tally.tool == tool && tally.failure == failure {
                tally.calls = tally.calls.saturating_add(1);
                return;
            }
        }
        if usize::from(self.len) == MAX_TOOL_FAILURE_KINDS {
            self.elided = self.elided.saturating_add(1);
            return;
        }
        self.tallies[usize::from(self.len)] = ToolFailureTally {
            tool,
            failure,
            calls: 1,
        };
        self.len += 1;
    }

    /// The pairs, in the order they first failed.
    pub fn iter(&self) -> impl Iterator<Item = ToolFailureTally> + '_ {
        self.tallies[..usize::from(self.len)].iter().copied()
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    /// Whether this step says nothing about what failed — a turn in which
    /// nothing did, or an entry written by a build that did not record it.
    pub fn is_empty(&self) -> bool {
        self.len == 0 && self.elided == 0
    }

    /// Failures whose `(tool, failure)` pair did not fit. The listed calls
    /// plus this is the step's `failed`.
    pub fn elided(&self) -> u32 {
        u32::from(self.elided)
    }

    /// The total this breakdown accounts for, which a caller checks against
    /// the step's own `failed` count.
    pub fn total(&self) -> u32 {
        self.iter().fold(self.elided(), |sum, tally| {
            sum.saturating_add(u32::from(tally.calls))
        })
    }
}

/// How the breakdown is written down: the listed pairs and the tail's
/// count, both absent when there are none. A fixed array would store eight
/// nulls in every entry; this stores nothing in the ordinary case.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StoredToolFailures {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tallies: Vec<ToolFailureTally>,
    #[serde(default, skip_serializing_if = "is_zero")]
    elided: u16,
}

fn is_zero(value: &u16) -> bool {
    *value == 0
}

impl From<ToolFailures> for StoredToolFailures {
    fn from(failures: ToolFailures) -> Self {
        Self {
            tallies: failures.iter().collect(),
            elided: failures.elided,
        }
    }
}

/// **What an older or a newer writer left behind is read, never trusted.**
/// A list longer than this build's bound is folded into the tail count
/// rather than truncated away, and a zero-call tally is dropped: the
/// invariant `listed + elided == accounted for` survives either.
impl From<StoredToolFailures> for ToolFailures {
    fn from(stored: StoredToolFailures) -> Self {
        let mut failures = Self {
            elided: stored.elided,
            ..Self::default()
        };
        for tally in stored.tallies {
            if tally.calls == 0 {
                continue;
            }
            if usize::from(failures.len) == MAX_TOOL_FAILURE_KINDS {
                failures.elided = failures.elided.saturating_add(tally.calls);
                continue;
            }
            failures.tallies[usize::from(failures.len)] = tally;
            failures.len += 1;
        }
        failures
    }
}

impl Serialize for ToolFailures {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        StoredToolFailures::from(*self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolFailures {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        StoredToolFailures::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for ToolFailures {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <StoredToolFailures as schemars::JsonSchema>::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <StoredToolFailures as schemars::JsonSchema>::json_schema(generator)
    }
}
