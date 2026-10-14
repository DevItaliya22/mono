// SPDX-License-Identifier: MPL-2.0

//! The creation ledger: what the registry knows about each creation key.
//!
//! A creation key is reserved **before** its process is spawned, marked
//! live when the session is inserted, and remembered after the session
//! ends — so a non-creating lookup during the spawn window answers
//! `Creating` rather than reading an empty map as nothing running (§7.6).
//! A key a fresh attempt has retired is refused admission from then on: a
//! read alone cannot exclude the future, and this is the fence that does.
//!
//! The ledger is plain data. The registry owns it under its own lock and
//! keeps every operation a map insert, so no I/O joins the thread that
//! answers the question.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use koru_session_protocol::{CreationLookup, IdempotencyKey, SessionId, SessionSummary};

/// How many ended creations and how many retired keys the ledger remembers,
/// each oldest-first. Reserved and live entries are never evicted: one is
/// bounded by in-flight creations and the other by live sessions. A retired
/// key falls out only after this many later retirements, which is far
/// beyond the lifetime of any create still in a host's blocking lane.
pub const CREATION_MEMORY_CAPACITY: usize = 4_096;

enum Record {
    Reserved,
    Live(SessionId),
    Ended(Box<SessionSummary>),
}

/// What the ledger says when a creation asks to be admitted.
pub(crate) enum LedgerAdmission {
    /// The key was free and is now reserved for this creation.
    Reserved,
    /// A live session already carries the key.
    Known(SessionId),
    /// A session carried the key and has ended; this is its last summary.
    Ended(Box<SessionSummary>),
    /// Another creation holds the reservation right now.
    InFlight,
    /// A fresh attempt retired the key; nothing may start under it.
    Retired,
}

#[derive(Default)]
pub(crate) struct CreationLedger {
    records: BTreeMap<IdempotencyKey, Record>,
    by_session: BTreeMap<SessionId, IdempotencyKey>,
    ended_order: VecDeque<IdempotencyKey>,
    retired: BTreeSet<IdempotencyKey>,
    retired_order: VecDeque<IdempotencyKey>,
}

impl CreationLedger {
    /// Reserve `key` for a creation about to spawn, unless the ledger
    /// already knows better. Checking retirement first is the fence.
    pub(crate) fn admit(&mut self, key: IdempotencyKey) -> LedgerAdmission {
        if self.retired.contains(&key) {
            return LedgerAdmission::Retired;
        }
        match self.records.get(&key) {
            Some(Record::Reserved) => LedgerAdmission::InFlight,
            Some(Record::Live(id)) => LedgerAdmission::Known(*id),
            Some(Record::Ended(summary)) => LedgerAdmission::Ended(summary.clone()),
            None => {
                self.records.insert(key, Record::Reserved);
                LedgerAdmission::Reserved
            }
        }
    }

    /// The reservation's process is now a registered session.
    pub(crate) fn complete(&mut self, key: IdempotencyKey, id: SessionId) {
        if matches!(self.records.get(&key), Some(Record::Reserved)) {
            self.records.insert(key, Record::Live(id));
            self.by_session.insert(id, key);
        }
    }

    /// The reservation's spawn failed: nothing runs under this key.
    pub(crate) fn abandon(&mut self, key: IdempotencyKey) {
        if matches!(self.records.get(&key), Some(Record::Reserved)) {
            self.records.remove(&key);
        }
    }

    /// A live session was reaped. The ledger keeps its last summary so a
    /// lookup still answers `KnownEnded` after the map has forgotten it.
    pub(crate) fn ended(&mut self, id: SessionId, summary: SessionSummary) {
        let Some(key) = self.by_session.remove(&id) else {
            return;
        };
        if matches!(self.records.get(&key), Some(Record::Live(live)) if *live == id) {
            self.records.insert(key, Record::Ended(Box::new(summary)));
            self.ended_order.push_back(key);
            while self.ended_order.len() > CREATION_MEMORY_CAPACITY {
                if let Some(evicted) = self.ended_order.pop_front()
                    && matches!(self.records.get(&evicted), Some(Record::Ended(_)))
                {
                    self.records.remove(&evicted);
                }
            }
        }
    }

    /// Fence the key: no creation is admitted under it from now on. The
    /// record it holds is untouched, so a lookup still says what ran.
    pub(crate) fn retire(&mut self, key: IdempotencyKey) {
        if self.retired.insert(key) {
            self.retired_order.push_back(key);
            while self.retired_order.len() > CREATION_MEMORY_CAPACITY {
                if let Some(evicted) = self.retired_order.pop_front() {
                    self.retired.remove(&evicted);
                }
            }
        }
    }

    pub(crate) fn is_retired(&self, key: IdempotencyKey) -> bool {
        self.retired.contains(&key)
    }

    /// The record's answer, given the map's view of a live id. `live` is
    /// asked only for a `Live` record and returns the session's summary
    /// with whether it has ended; the registry answers it from a handle it
    /// already holds, so the ledger stays a map.
    pub(crate) fn lookup(
        &self,
        key: IdempotencyKey,
        live: impl FnOnce(SessionId) -> Option<(SessionSummary, bool)>,
    ) -> CreationLookup {
        match self.records.get(&key) {
            Some(Record::Reserved) => CreationLookup::Creating,
            Some(Record::Live(id)) => match live(*id) {
                Some((session, false)) => CreationLookup::Known { session },
                Some((session, true)) => CreationLookup::KnownEnded { session },
                None => CreationLookup::NoEvidence,
            },
            Some(Record::Ended(session)) => CreationLookup::KnownEnded {
                session: (**session).clone(),
            },
            None => CreationLookup::NoEvidence,
        }
    }

    /// The live id a `Live` record names, for a lookup that must fetch the
    /// summary outside the registry lock.
    pub(crate) fn live_session(&self, key: IdempotencyKey) -> Option<SessionId> {
        match self.records.get(&key) {
            Some(Record::Live(id)) => Some(*id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: SessionId) -> SessionSummary {
        use koru_session_protocol::*;
        SessionSummary {
            id,
            parent_session_id: None,
            profile: SessionProfileKey::BuiltIn("codex".parse().unwrap()),
            display_name: "fixture".into(),
            working_directory: None,
            mode: SessionMode::RawPty,
            process: ProcessState::Exited { code: Some(0) },
            controller: ControllerState::Detached,
            managed: None,
            active_turn_id: None,
            attention: AttentionState::default(),
            pending_attachment_count: 0,
            capabilities: CapabilitySet::default(),
            project_id: None,
            handover: HandoverState::Idle,
            model: None,
            origin: Default::default(),
            created_at_ms: 1,
        }
    }

    /// Reserve, complete, end: each state answers the lookup differently,
    /// and none of them is an empty map.
    #[test]
    fn a_key_moves_from_reserved_to_live_to_ended_and_each_answers() {
        let mut ledger = CreationLedger::default();
        let key = IdempotencyKey::mint();
        let id = SessionId::mint();
        assert!(matches!(
            ledger.lookup(key, |_| None),
            CreationLookup::NoEvidence
        ));
        assert!(matches!(ledger.admit(key), LedgerAdmission::Reserved));
        assert!(matches!(ledger.admit(key), LedgerAdmission::InFlight));
        assert!(matches!(
            ledger.lookup(key, |_| None),
            CreationLookup::Creating
        ));

        ledger.complete(key, id);
        assert!(matches!(ledger.admit(key), LedgerAdmission::Known(live) if live == id));
        assert_eq!(ledger.live_session(key), Some(id));
        assert!(matches!(
            ledger.lookup(key, |live| Some((summary(live), false))),
            CreationLookup::Known { session } if session.id == id
        ));
        assert!(matches!(
            ledger.lookup(key, |live| Some((summary(live), true))),
            CreationLookup::KnownEnded { session } if session.id == id
        ));

        ledger.ended(id, summary(id));
        assert!(matches!(ledger.admit(key), LedgerAdmission::Ended(session) if session.id == id));
        assert!(matches!(
            ledger.lookup(key, |_| panic!("an ended record never asks the map")),
            CreationLookup::KnownEnded { session } if session.id == id
        ));
    }

    /// A spawn that failed leaves nothing: the next create under the key
    /// is admitted afresh, and a lookup in between says nothing ran.
    #[test]
    fn an_abandoned_reservation_leaves_no_evidence() {
        let mut ledger = CreationLedger::default();
        let key = IdempotencyKey::mint();
        assert!(matches!(ledger.admit(key), LedgerAdmission::Reserved));
        ledger.abandon(key);
        assert!(matches!(
            ledger.lookup(key, |_| None),
            CreationLookup::NoEvidence
        ));
        assert!(matches!(ledger.admit(key), LedgerAdmission::Reserved));
        // Abandoning a live record is not a thing: only a reservation goes.
        let id = SessionId::mint();
        ledger.complete(key, id);
        ledger.abandon(key);
        assert_eq!(ledger.live_session(key), Some(id));
    }

    /// Retirement refuses admission whatever the record held, and does not
    /// erase what the record says.
    #[test]
    fn a_retired_key_is_refused_but_still_answers_what_ran() {
        let mut ledger = CreationLedger::default();
        let fresh = IdempotencyKey::mint();
        ledger.retire(fresh);
        assert!(ledger.is_retired(fresh));
        assert!(matches!(ledger.admit(fresh), LedgerAdmission::Retired));
        assert!(matches!(
            ledger.lookup(fresh, |_| None),
            CreationLookup::NoEvidence
        ));

        let live = IdempotencyKey::mint();
        let id = SessionId::mint();
        assert!(matches!(ledger.admit(live), LedgerAdmission::Reserved));
        ledger.complete(live, id);
        ledger.retire(live);
        assert!(matches!(ledger.admit(live), LedgerAdmission::Retired));
        assert!(matches!(
            ledger.lookup(live, |live| Some((summary(live), false))),
            CreationLookup::Known { .. }
        ));
    }

    /// The two memories are bounded oldest-first, and the bound never
    /// touches a reservation or a live session.
    #[test]
    fn ended_and_retired_memories_are_bounded_oldest_first() {
        let mut ledger = CreationLedger::default();
        let oldest_ended = IdempotencyKey::mint();
        let oldest_id = SessionId::mint();
        assert!(matches!(
            ledger.admit(oldest_ended),
            LedgerAdmission::Reserved
        ));
        ledger.complete(oldest_ended, oldest_id);
        ledger.ended(oldest_id, summary(oldest_id));
        let reserved = IdempotencyKey::mint();
        assert!(matches!(ledger.admit(reserved), LedgerAdmission::Reserved));
        let live = IdempotencyKey::mint();
        let live_id = SessionId::mint();
        assert!(matches!(ledger.admit(live), LedgerAdmission::Reserved));
        ledger.complete(live, live_id);
        for _ in 0..CREATION_MEMORY_CAPACITY {
            let key = IdempotencyKey::mint();
            let id = SessionId::mint();
            assert!(matches!(ledger.admit(key), LedgerAdmission::Reserved));
            ledger.complete(key, id);
            ledger.ended(id, summary(id));
        }
        assert!(matches!(
            ledger.lookup(oldest_ended, |_| None),
            CreationLookup::NoEvidence
        ));
        assert!(matches!(
            ledger.lookup(reserved, |_| None),
            CreationLookup::Creating
        ));
        assert_eq!(ledger.live_session(live), Some(live_id));

        let oldest_retired = IdempotencyKey::mint();
        ledger.retire(oldest_retired);
        for _ in 0..CREATION_MEMORY_CAPACITY {
            ledger.retire(IdempotencyKey::mint());
        }
        assert!(!ledger.is_retired(oldest_retired));
        ledger.retire(oldest_retired);
        ledger.retire(oldest_retired);
        assert!(ledger.is_retired(oldest_retired));
    }
}
