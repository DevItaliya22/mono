// SPDX-License-Identifier: MPL-2.0

//! What a run cost, as facts (§17).
//!
//! A run that finished without recording its tokens can never be analysed,
//! so M1 records and M10 reports (D39). Three rules make the numbers mean
//! something:
//!
//! - **The accounting unit is a turn**, keyed by `(RunAttemptId, TurnId)`.
//!   A provider's report is normalized by its adapter to one total for that
//!   key, whatever the provider sends.
//! - **A fact has a stable identity and a monotonic revision.** A retry
//!   repeats both; a correction raises the revision. [`replay`] selects the
//!   highest revision regardless of delivery order, so 100 → 140 → 100
//!   (replayed) is 140, and a later correction to 90 is 90.
//! - **Unavailable is unavailable, never zero.** A missing count is `None`;
//!   a total that includes a missing count says so ([`TokenTotal`]).
//!
//! Elapsed time and interventions are defined here as functions over the
//! timeline (§17.2), so M10 compares like with like.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    AgentTurnId, ModelRef, ProviderKind, RunAttemptId, StepKind, TimelineEntry, TurnOriginator,
    UsageFactId,
};

/// Bounded ASCII model name as a provider reports it — `gpt-5-codex`,
/// `claude-sonnet-4-5`. Printable ASCII without whitespace, at most 128
/// bytes. Retained metadata, never an event attribute (§17.3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
// Distinct from the session protocol's model name in the combined Gateway schema.
#[cfg_attr(feature = "schema", schemars(rename = "DelegateModelName"))]
#[serde(try_from = "String", into = "String")]
pub struct ModelName(String);

impl std::fmt::Debug for ModelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ModelName(<redacted>)")
    }
}

/// Why a model name was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InvalidModelName;

impl std::fmt::Display for InvalidModelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a model name is 1–128 printable ASCII bytes without whitespace")
    }
}

impl std::error::Error for InvalidModelName {}

impl ModelName {
    pub const MAX_BYTES: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, InvalidModelName> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_BYTES
            && value.bytes().all(|byte| byte.is_ascii_graphic());
        valid.then_some(Self(value)).ok_or(InvalidModelName)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ModelName {
    type Error = InvalidModelName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ModelName> for String {
    fn from(value: ModelName) -> Self {
        value.0
    }
}

/// What a provider said about a model, with an explicit unknown: a provider
/// that says nothing is recorded as saying nothing, never as the last model
/// seen.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelReport {
    Unknown,
    Named { name: ModelName },
}

/// The retained provider/model metadata a [`ModelRef`] points at (§17.3).
/// Keeps the model *requested* and the model *reported* separately, because
/// they differ.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelIdentity {
    pub id: ModelRef,
    pub provider: ProviderKind,
    pub requested: ModelReport,
    pub reported: ModelReport,
}

/// Why a usage fact is incomplete. Recorded rather than guessed, because a
/// wrong assumption here misprices every run silently (§17.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum IncompleteReason {
    /// The provider's note does not say whether `input` includes cached
    /// input, so neither can be trusted alone.
    CachedInputRelationshipUndeclared,
    /// The turn used more than one model and the provider gave no
    /// attributable slices.
    MultipleModels,
    /// The provider reported no usage for the turn at all; the observable
    /// metrics still count.
    NoUsageReported,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Completeness {
    Complete,
    Incomplete { reason: IncompleteReason },
}

/// The monotonic amendment revision of a usage fact. A retry repeats it; a
/// correction raises it.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct AmendmentRevision(pub u32);

impl AmendmentRevision {
    pub const FIRST: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// The accounting key (§17.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TurnKey {
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
}

/// One turn's usage, as a durable typed fact (§17.1: committed in the run's
/// own storage before it is published). Ids, counts and a reference — it
/// copies nothing (§17.2a).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UsageFact {
    /// Stable across retries and amendments alike.
    pub identity: UsageFactId,
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
    pub revision: AmendmentRevision,
    /// `None` is unavailable. It is never zero for missing.
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached_input: Option<u64>,
    pub model: ModelRef,
    pub completeness: Completeness,
}

impl UsageFact {
    pub fn key(&self) -> TurnKey {
        TurnKey {
            attempt: self.attempt,
            turn: self.turn,
        }
    }
}

/// Fold delivered facts to one per turn, selecting the highest revision
/// regardless of delivery order. A repeated delivery of the same revision
/// (a retry) changes nothing; a higher revision (a correction) wins even
/// when it arrives first and the original is replayed after it.
pub fn replay<'a>(facts: impl IntoIterator<Item = &'a UsageFact>) -> BTreeMap<TurnKey, UsageFact> {
    let mut by_turn: BTreeMap<TurnKey, UsageFact> = BTreeMap::new();
    for fact in facts {
        match by_turn.get(&fact.key()) {
            Some(held) if held.revision >= fact.revision => {}
            _ => {
                by_turn.insert(fact.key(), fact.clone());
            }
        }
    }
    by_turn
}

/// A token total that knows what it is missing. `known` sums every turn
/// that reported; `unavailable_turns` counts every turn that did not, so a
/// surface renders "at least 1,200 (2 turns unreported)" rather than
/// "1,200" — or "unavailable" when nothing reported at all.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TokenTotal {
    pub known: u64,
    pub unavailable_turns: u32,
}

impl TokenTotal {
    fn add(&mut self, count: Option<u64>) {
        match count {
            Some(count) => self.known = self.known.saturating_add(count),
            None => self.unavailable_turns = self.unavailable_turns.saturating_add(1),
        }
    }

    /// The total when every turn reported, or `None`. Never zero for
    /// missing.
    pub fn complete(self) -> Option<u64> {
        (self.unavailable_turns == 0).then_some(self.known)
    }
}

/// The run's economics as the header carries them (D39): a projection of
/// the replayed facts and the timeline, recomputable at any time.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EconomicsSummary {
    /// Turns the timeline recorded, by originator.
    pub orchestrator_turns: u32,
    pub human_turns: u32,
    /// Turns with a usage fact, after replay.
    pub turns_with_usage: u32,
    pub input: TokenTotal,
    pub output: TokenTotal,
    pub cached_input: TokenTotal,
    pub tool_calls: u32,
    /// §17.2: control handovers plus human-originated turns.
    pub interventions: u32,
    /// §17.2: wall-clock excluding time spent `Interrupted` or under human
    /// control. `None` until the run has started.
    pub elapsed_ms: Option<u64>,
    /// Every model any turn used, as references.
    #[serde(default)]
    pub models: BTreeSet<ModelRef>,
    /// Whether every usage fact was complete. A single incomplete fact makes
    /// the summary incomplete, because a price over it would be a guess.
    pub complete: bool,
}

impl EconomicsSummary {
    /// Project the summary from replayed facts and the timeline. `now_ms`
    /// closes an interval still open — a run still running counts up to
    /// now; a finished run needs no clock.
    pub fn project(
        facts: &BTreeMap<TurnKey, UsageFact>,
        timeline: &[TimelineEntry],
        now_ms: i64,
    ) -> Self {
        let mut summary = Self {
            complete: true,
            ..Self::default()
        };
        for fact in facts.values() {
            summary.turns_with_usage += 1;
            summary.input.add(fact.input);
            summary.output.add(fact.output);
            summary.cached_input.add(fact.cached_input);
            summary.models.insert(fact.model);
            if fact.completeness != Completeness::Complete {
                summary.complete = false;
            }
        }
        for entry in timeline {
            match entry.kind {
                StepKind::TurnStarted {
                    originator: TurnOriginator::Orchestrator,
                } => summary.orchestrator_turns += 1,
                StepKind::TurnStarted {
                    originator: TurnOriginator::Human,
                } => summary.human_turns += 1,
                StepKind::ToolsCompleted { calls, .. } => {
                    summary.tool_calls = summary.tool_calls.saturating_add(calls);
                }
                _ => {}
            }
        }
        summary.interventions = interventions(timeline);
        summary.elapsed_ms = elapsed_ms(timeline, now_ms);
        summary
    }
}

/// §17.2's definition of an intervention: a control handover, or a
/// human-originated turn, counted separately from the agent's own turns.
/// Both are read from the timeline, so a device that never watched the run
/// computes the same number.
pub fn interventions(timeline: &[TimelineEntry]) -> u32 {
    timeline
        .iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                StepKind::ControlTaken { .. }
                    | StepKind::TurnStarted {
                        originator: TurnOriginator::Human
                    }
            )
        })
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// §17.2's definition of elapsed time: wall-clock from the attempt's start
/// to its end, **excluding** time the run spent `Interrupted` or under
/// human control. The timeline is read in its own order; the clock is used
/// for differences only.
///
/// An interval opens at `Started`, `Resumed` or `ControlReturned` and closes
/// at `Interrupted`, `ControlTaken` or a terminal step. An interval still
/// open at the end counts up to `now_ms`. Returns `None` when the run has
/// not started.
pub fn elapsed_ms(timeline: &[TimelineEntry], now_ms: i64) -> Option<u64> {
    let mut total: u64 = 0;
    let mut open_since: Option<i64> = None;
    let mut started = false;
    for entry in timeline {
        match entry.kind {
            StepKind::Started | StepKind::Resumed { .. } | StepKind::ControlReturned => {
                started = true;
                open_since.get_or_insert(entry.at_ms);
            }
            StepKind::Interrupted { .. } | StepKind::ControlTaken { .. } => {
                close(&mut total, &mut open_since, entry.at_ms);
            }
            kind if kind.is_terminal() => {
                close(&mut total, &mut open_since, entry.at_ms);
            }
            _ => {}
        }
    }
    close(&mut total, &mut open_since, now_ms);
    started.then_some(total)
}

fn close(total: &mut u64, open_since: &mut Option<i64>, at_ms: i64) {
    if let Some(since) = open_since.take() {
        *total = total.saturating_add(at_ms.saturating_sub(since).max(0) as u64);
    }
}
