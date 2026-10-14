// SPDX-License-Identifier: MPL-2.0

//! The closed domains, and the typed conversion at the boundary.
//!
//! SQLite has no enum, so each domain is `CHECK`ed in the DDL and converted
//! here — one token per variant, spelled once, refused twice (§8: *checked
//! closed domains and typed conversion at the boundary, per
//! `016_agent_profile_cleanup.sql`*). A token this build does not know is a
//! [`crate::LedgerError::Corrupt`], named by column and never by value.

use koru_delegate_core::PublicationKind;

use crate::LedgerError;

/// Where a creation intent is (§7.1 steps 1–4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IntentState {
    /// Recorded; the Run document is not yet committed. Listed as a request,
    /// never as a run.
    Pending,
    /// The Run document exists and is named on the row.
    Satisfied,
    /// The person cancelled before the run existed, or a capture failed.
    Abandoned,
}

impl IntentState {
    pub fn token(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Satisfied => "satisfied",
            Self::Abandoned => "abandoned",
        }
    }

    pub(crate) fn parse(token: &str) -> Result<Self, LedgerError> {
        match token {
            "pending" => Ok(Self::Pending),
            "satisfied" => Ok(Self::Satisfied),
            "abandoned" => Ok(Self::Abandoned),
            _ => Err(LedgerError::Corrupt {
                table: "run_intents",
                column: "state",
            }),
        }
    }
}

/// How far one attempt got on this machine. **There is no `Failed`** (D10):
/// a restart interrupts an attempt, and what recovery cannot prove is
/// `Unknown`, flagged for a person.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AttemptPhase {
    /// The row exists; nothing has been asked of a host.
    Created,
    /// Admission, materialization and session creation are under way.
    Starting,
    /// The provider is up.
    Running,
    /// A restart ended the attempt with nothing surviving; a fresh attempt
    /// is safe.
    Interrupted,
    /// The provider ended, cleanly or not; the Run document says which.
    Ended,
    /// No evidence on a host that did not restart (§7.6's last row). Never
    /// auto-resumed.
    Unknown,
}

impl AttemptPhase {
    pub const ALL: [Self; 6] = [
        Self::Created,
        Self::Starting,
        Self::Running,
        Self::Interrupted,
        Self::Ended,
        Self::Unknown,
    ];

    pub fn token(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Interrupted => "interrupted",
            Self::Ended => "ended",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(token: &str) -> Result<Self, LedgerError> {
        Self::ALL
            .into_iter()
            .find(|phase| phase.token() == token)
            .ok_or(LedgerError::Corrupt {
                table: "run_attempts",
                column: "state",
            })
    }

    /// `Interrupted`, `Ended` or `Unknown`: nothing moves the attempt again;
    /// a resume is a fresh attempt.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Interrupted | Self::Ended | Self::Unknown)
    }

    /// Whether an attempt may move from `self` to `next` — the ladder
    /// `created → starting → running`, and any non-terminal phase may end
    /// in any terminal one. Moving to the phase already held is not a
    /// transition; the caller treats it as already applied.
    pub fn may_advance_to(self, next: Self) -> bool {
        if self.is_terminal() {
            return false;
        }
        match (self, next) {
            (Self::Created, Self::Starting) | (Self::Starting, Self::Running) => true,
            (_, terminal) => terminal.is_terminal(),
        }
    }
}

/// A projection of the run's retained facts, with its own cursor (§17.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Projection {
    /// The Run header's economics summary.
    HeaderSummary,
    /// The event bus.
    Bus,
}

impl Projection {
    pub fn token(self) -> &'static str {
        match self {
            Self::HeaderSummary => "header_summary",
            Self::Bus => "bus",
        }
    }
}

/// How a claimed slot ended (§7.7, D25, D34).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SlotOutcome {
    /// The run it minted was started.
    Fired,
    /// The occurrence was already past when the device woke; skipped and
    /// recorded, never caught up (D25).
    SkippedMissed,
    /// The working folder was held by another run (D34's scheduled half).
    SkippedBusy,
    /// The schedule was retired between the claim and the firing.
    SkippedRetired,
}

impl SlotOutcome {
    pub const ALL: [Self; 4] = [
        Self::Fired,
        Self::SkippedMissed,
        Self::SkippedBusy,
        Self::SkippedRetired,
    ];

    pub fn token(self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::SkippedMissed => "skipped_missed",
            Self::SkippedBusy => "skipped_busy",
            Self::SkippedRetired => "skipped_retired",
        }
    }
}

/// The `outcome` column: claimed, or settled with a [`SlotOutcome`]. Split
/// so `record_outcome` cannot be asked to record "claimed".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SlotState {
    Claimed,
    Settled(SlotOutcome),
}

impl SlotState {
    pub fn token(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Settled(outcome) => outcome.token(),
        }
    }

    pub(crate) fn parse(token: &str) -> Result<Self, LedgerError> {
        if token == "claimed" {
            return Ok(Self::Claimed);
        }
        SlotOutcome::ALL
            .into_iter()
            .find(|outcome| outcome.token() == token)
            .map(Self::Settled)
            .ok_or(LedgerError::Corrupt {
                table: "schedule_claims",
                column: "outcome",
            })
    }
}

/// The publication kinds, as the DDL spells them — the same tokens the
/// vocabulary crate serializes, so one fact has one spelling on both sides
/// of this boundary.
pub(crate) fn publication_token(kind: PublicationKind) -> &'static str {
    match kind {
        PublicationKind::Lifecycle => "lifecycle",
        PublicationKind::ArtifactRegistration => "artifact_registration",
        PublicationKind::Usage => "usage",
    }
}

pub(crate) fn parse_publication_kind(token: &str) -> Result<PublicationKind, LedgerError> {
    match token {
        "lifecycle" => Ok(PublicationKind::Lifecycle),
        "artifact_registration" => Ok(PublicationKind::ArtifactRegistration),
        "usage" => Ok(PublicationKind::Usage),
        _ => Err(LedgerError::Corrupt {
            table: "pending_publications",
            column: "kind",
        }),
    }
}

#[cfg(test)]
mod tests;
