// SPDX-License-Identifier: MPL-2.0

//! What a ledger write or read can refuse, as stable codes.
//!
//! Every variant is a code a caller branches on (PRINCIPLES, *APIs return
//! stable codes, not prose*), and none carries content: an intent conflict
//! names the run, never either payload; a storage failure carries SQLite's
//! own message, which names tables and constraints and nothing a person
//! wrote.

use std::fmt;

use koru_delegate_core::{AttemptGeneration, RunId, StaleAttempt};

use crate::{AttemptPhase, SlotOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// SQLite refused. `detail` is its message: a constraint, a table, a
    /// code — never a value from a content column.
    Storage {
        operation: &'static str,
        detail: String,
    },
    /// A stored value could not be read back as its type. Named by column,
    /// never by value.
    Corrupt {
        table: &'static str,
        column: &'static str,
    },
    /// The intent payload could not be encoded. Carries nothing of it.
    PayloadUnencodable,
    /// A publication body must be JSON, because the column says so.
    BodyNotJson,
    /// No intent is recorded under this run.
    UnknownRun,
    /// A second `record_intent` for a known run with a different payload or
    /// workspace. The first request stands; nothing is overwritten.
    IntentConflict,
    /// `satisfy_intent` on an intent the person cancelled.
    IntentAbandoned,
    /// `abandon_intent` on an intent whose run already exists: removing a
    /// run is its own action (D31), not a late cancel of its request.
    RunExists,
    /// `open_attempt` before the Run document is committed. A request is
    /// not a run (§7.1), and nothing starts a provider for a request.
    RunNotCreated,
    /// `open_attempt` while an unretired attempt stands. Retiring the old
    /// attempt is what authorises a fresh one (§7.6).
    AttemptLive {
        generation: AttemptGeneration,
    },
    /// The run has no attempt yet.
    NoAttempt,
    /// The generation offered is not the current one.
    StaleAttempt(StaleAttempt),
    /// The attempt was retired; it accepts no further write.
    AttemptRetired {
        generation: AttemptGeneration,
    },
    /// The attempt is in a terminal phase and cannot bind a session.
    AttemptEnded {
        phase: AttemptPhase,
    },
    /// A pair the closed attempt domain does not allow.
    IllegalAttemptTransition {
        from: AttemptPhase,
        to: AttemptPhase,
    },
    /// `retire` before the attempt reached a terminal phase.
    AttemptNotEnded {
        phase: AttemptPhase,
    },
    /// A different key, session or host than the one already bound, or a
    /// key another attempt already holds. A key names one attempt, ever.
    SessionKeyConflict,
    /// No publication is queued under this event id.
    UnknownPublication,
    /// A prepared schedule publication has not yet been confirmed against its
    /// authoritative mutation and therefore cannot be delivered or acknowledged.
    PublicationNotReady,
    /// A delivery attempt cannot be recorded after acknowledgement.
    PublicationAlreadyAcknowledged,
    /// Only an unconfirmed preparation may be abandoned. Once ready, the row
    /// is a retained fact and cannot be deleted or rewritten.
    PublicationAlreadyReady,
    /// Another EventId already names this schedule revision or occurrence.
    /// The existing row stands so restart reconciliation can reuse its identity.
    SchedulePublicationConflict,
    /// A schedule publication's kind-specific invariant was invalid (for
    /// example a Created root not at revision one).
    InvalidSchedulePublicationFact,
    /// A projection cursor only moves forward.
    CursorMovedBackwards {
        current: u64,
        offered: u64,
    },
    /// A schedule revision starts at one.
    RevisionZero,
    /// The single-use schedule proof no longer matches live authority or its
    /// exact bound launch/skip plan.
    InvalidScheduleAuthority,
    InvalidSkipOutcome,
    /// A claimed launch can only settle as fired. Pause or retirement after
    /// the claim never rewrites it into a skip.
    InvalidSettlementOutcome,
    /// The slot is already claimed — by this run, under that revision.
    SlotAlreadyClaimed {
        run: RunId,
        revision: u32,
    },
    /// No claim exists for this slot.
    UnknownSlot,
    /// The slot already has a different outcome recorded.
    SlotOutcomeRecorded {
        outcome: SlotOutcome,
    },
    /// The occurrence has the same outcome but different immutable evidence
    /// (for example another busy holder).
    SlotClaimConflict,
    /// A test-armed crash fired. The ledger is dead from here: every further
    /// call answers with the same point, the way a crashed process answers
    /// nothing.
    #[cfg(any(test, feature = "crash-points"))]
    Crashed(crate::CrashPoint),
}

impl LedgerError {
    pub(crate) fn sql(operation: &'static str, error: rusqlite::Error) -> Self {
        Self::Storage {
            operation,
            detail: error.to_string(),
        }
    }

    /// The closed vocabulary a refusal is reported under.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Storage { .. } => "storage",
            Self::Corrupt { .. } => "corrupt",
            Self::PayloadUnencodable => "payload_unencodable",
            Self::BodyNotJson => "body_not_json",
            Self::UnknownRun => "unknown_run",
            Self::IntentConflict => "intent_conflict",
            Self::IntentAbandoned => "intent_abandoned",
            Self::RunExists => "run_exists",
            Self::RunNotCreated => "run_not_created",
            Self::AttemptLive { .. } => "attempt_live",
            Self::NoAttempt => "no_attempt",
            Self::StaleAttempt(_) => "stale_attempt",
            Self::AttemptRetired { .. } => "attempt_retired",
            Self::AttemptEnded { .. } => "attempt_ended",
            Self::IllegalAttemptTransition { .. } => "illegal_attempt_transition",
            Self::AttemptNotEnded { .. } => "attempt_not_ended",
            Self::SessionKeyConflict => "session_key_conflict",
            Self::UnknownPublication => "unknown_publication",
            Self::PublicationNotReady => "publication_not_ready",
            Self::PublicationAlreadyAcknowledged => "publication_already_acknowledged",
            Self::PublicationAlreadyReady => "publication_already_ready",
            Self::SchedulePublicationConflict => "schedule_publication_conflict",
            Self::InvalidSchedulePublicationFact => "invalid_schedule_publication_fact",
            Self::CursorMovedBackwards { .. } => "cursor_moved_backwards",
            Self::RevisionZero => "revision_zero",
            Self::InvalidScheduleAuthority => "invalid_schedule_authority",
            Self::InvalidSkipOutcome => "invalid_skip_outcome",
            Self::InvalidSettlementOutcome => "invalid_settlement_outcome",
            Self::SlotAlreadyClaimed { .. } => "slot_already_claimed",
            Self::UnknownSlot => "unknown_slot",
            Self::SlotOutcomeRecorded { .. } => "slot_outcome_recorded",
            Self::SlotClaimConflict => "slot_claim_conflict",
            #[cfg(any(test, feature = "crash-points"))]
            Self::Crashed(_) => "crashed",
        }
    }
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage { operation, detail } => write!(f, "could not {operation}: {detail}"),
            Self::Corrupt { table, column } => {
                write!(f, "{table}.{column} holds a value this build cannot read")
            }
            Self::StaleAttempt(stale) => stale.fmt(f),
            Self::AttemptLive { generation } => {
                write!(f, "attempt generation {} is live", generation.get())
            }
            Self::AttemptRetired { generation } => {
                write!(f, "attempt generation {} is retired", generation.get())
            }
            Self::AttemptEnded { phase } => write!(f, "attempt is {}", phase.token()),
            Self::IllegalAttemptTransition { from, to } => {
                write!(
                    f,
                    "attempt cannot move from {} to {}",
                    from.token(),
                    to.token()
                )
            }
            Self::AttemptNotEnded { phase } => {
                write!(f, "attempt is {} and cannot be retired", phase.token())
            }
            Self::CursorMovedBackwards { current, offered } => {
                write!(f, "cursor at {current} cannot move back to {offered}")
            }
            Self::SlotAlreadyClaimed { run, revision } => {
                write!(f, "slot claimed by {run:?} under revision {revision}")
            }
            Self::SlotOutcomeRecorded { outcome } => {
                write!(f, "slot outcome already recorded as {}", outcome.token())
            }
            #[cfg(any(test, feature = "crash-points"))]
            Self::Crashed(point) => write!(f, "crashed at {point:?}"),
            other => f.write_str(other.code()),
        }
    }
}

impl std::error::Error for LedgerError {}

impl From<StaleAttempt> for LedgerError {
    fn from(stale: StaleAttempt) -> Self {
        Self::StaleAttempt(stale)
    }
}
