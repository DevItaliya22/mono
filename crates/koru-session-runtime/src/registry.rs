// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::sync::Mutex;

use koru_session_protocol::{
    AiControlGeneration, ChatOriginId, CreationLookup, ErrorCode, IdempotencyKey, OriginLookup,
    SessionCatalogGeneration, SessionFailure, SessionId, SessionOrigin, SessionSummary,
};

use crate::creation::{CreationLedger, LedgerAdmission};
use crate::{SessionCreate, SessionHandle, SessionOwner, SessionRuntimeConfig};

struct RegistryState {
    sessions: BTreeMap<SessionId, SessionOwner>,
    creations: CreationLedger,
    shutting_down: bool,
    generation: SessionCatalogGeneration,
    /// Session origin ownership (protocol v16), kept beside the ledger so a
    /// lookup never has to take a session's own lock: which chat each live
    /// session belongs to, and which chat each in-flight reservation is for.
    /// Ended sessions leave both at reap, which is what makes an empty
    /// answer mean "nothing is running" rather than "nothing is remembered".
    chat_live: BTreeMap<SessionId, ChatOriginId>,
    chat_reservations: BTreeMap<IdempotencyKey, ChatOriginId>,
    /// The creation generation the most recent `ai_control` hello was
    /// handed; `None` until one registers. Compared at admission under this
    /// same lock, so a successor's registration and an older worker's
    /// create cannot interleave.
    ai_control: Option<AiControlGeneration>,
}

/// Summary-only catalogue over independent session workers.
///
/// The map lock is released before any worker, emulator, PTY, or subscriber
/// operation. One blocked session therefore cannot serialize another one.
pub struct SessionRegistry {
    config: SessionRuntimeConfig,
    state: Mutex<RegistryState>,
}

/// What the registry says when a keyed creation asks to be admitted.
///
/// Only `Reserved` may spawn. Every other answer is the registry knowing
/// better than an evicted replay ledger would: the key is already live, or
/// already ended, or another creation holds it, or a fresh attempt retired
/// it. None of them is a reason to start a second process.
pub enum CreationAdmission<'a> {
    Reserved(CreationReservation<'a>),
    Known(SessionHandle),
    Ended(Box<SessionSummary>),
    InFlight,
    Retired,
    /// A chat-origin creation from a connection whose creation generation
    /// is no longer current (protocol v16): a successor registered since.
    StaleAuthority,
    /// A chat-origin creation from a connection that never registered as
    /// `ai_control`: only the AI worker creates chat sessions.
    Forbidden,
    /// A chat-origin creation for a chat that already has a live session or
    /// an in-flight reservation under another key: one conversation per
    /// chat, decided under the lock that reserved the first.
    OriginOwned,
}

/// A key reserved in the ledger for a creation that has not yet spawned.
///
/// Completed by [`SessionRegistry::create_reserved`] when the session is
/// inserted, and abandoned on drop otherwise — so a spawn that fails leaves
/// nothing behind, and a lookup in the meantime reads `Creating`.
pub struct CreationReservation<'a> {
    registry: &'a SessionRegistry,
    key: IdempotencyKey,
    completed: bool,
}

impl CreationReservation<'_> {
    pub fn key(&self) -> IdempotencyKey {
        self.key
    }
}

impl Drop for CreationReservation<'_> {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self.registry.lock();
            state.creations.abandon(self.key);
            // A spawn that failed frees the chat it was reserved for, at the
            // same instant the key is freed: nothing runs under either.
            state.chat_reservations.remove(&self.key);
        }
    }
}

impl SessionRegistry {
    pub fn new(config: SessionRuntimeConfig) -> Self {
        Self {
            config,
            state: Mutex::new(RegistryState {
                sessions: BTreeMap::new(),
                creations: CreationLedger::default(),
                shutting_down: false,
                generation: SessionCatalogGeneration::new(1).expect("one is positive"),
                chat_live: BTreeMap::new(),
                chat_reservations: BTreeMap::new(),
                ai_control: None,
            }),
        }
    }

    /// An unkeyed creation: the raw argv path, which carries no idempotency
    /// key and therefore no reservation.
    pub fn create(&self, create: SessionCreate) -> Result<SessionHandle, SessionFailure> {
        self.admission_checks(&create)?;
        let owner = SessionOwner::spawn(create, self.config.clone())?;
        self.admit_spawned(owner, None)
    }

    /// Reserve `key` before spawning, or learn why the registry will not.
    ///
    /// Taken under the host's creation admission mutex, and a map insert
    /// only: no I/O joins the thread that answers a lookup in the window
    /// between this and the insert.
    pub fn admit_creation(
        &self,
        key: IdempotencyKey,
        origin: SessionOrigin,
        authority: Option<AiControlGeneration>,
    ) -> CreationAdmission<'_> {
        let mut state = self.lock();
        if let Some(chat) = origin.chat() {
            // The v16 order, all under this one lock: authority, then
            // ownership, then the key. A same-key retry is answered by the
            // ledger below exactly as before; a *different* key for an
            // owned chat is the second conversation that must not start.
            match (authority, state.ai_control) {
                (None, _) => return CreationAdmission::Forbidden,
                (Some(held), Some(current)) if held == current => {}
                (Some(_), _) => return CreationAdmission::StaleAuthority,
            }
            let owned_by_other_key = state
                .chat_reservations
                .iter()
                .any(|(reserved, owner)| *owner == chat && *reserved != key)
                || state.chat_live.values().any(|owner| *owner == chat)
                    && state.creations.live_session(key).is_none();
            if owned_by_other_key {
                return CreationAdmission::OriginOwned;
            }
        }
        match state.creations.admit(key) {
            LedgerAdmission::Reserved => {
                if let Some(chat) = origin.chat() {
                    state.chat_reservations.insert(key, chat);
                }
                CreationAdmission::Reserved(CreationReservation {
                    registry: self,
                    key,
                    completed: false,
                })
            }
            LedgerAdmission::Known(id) => match state.sessions.get(&id) {
                Some(owner) => CreationAdmission::Known(owner.handle()),
                // Reaped between the ledger's answer and this read, which
                // the same lock makes impossible; answer safely anyway.
                None => CreationAdmission::InFlight,
            },
            LedgerAdmission::Ended(summary) => CreationAdmission::Ended(summary),
            LedgerAdmission::InFlight => CreationAdmission::InFlight,
            LedgerAdmission::Retired => CreationAdmission::Retired,
        }
    }

    /// Hand a newly registered `ai_control` connection the next creation
    /// generation. Every chat-origin create from an earlier generation is
    /// refused `StaleAuthority` from this instant, under the same lock that
    /// admits creations, so a successor's registration and an old worker's
    /// create cannot interleave.
    pub fn register_ai_control(&self) -> AiControlGeneration {
        let mut state = self.lock();
        let next = state
            .ai_control
            .map_or(AiControlGeneration::first(), AiControlGeneration::next);
        state.ai_control = Some(next);
        next
    }

    /// What is live or in flight under an origin right now (protocol v16).
    /// Answered from the ownership maps under one lock, then the summaries
    /// are read outside it. Ended sessions are not in the answer.
    pub fn lookup_origin(&self, origin: SessionOrigin) -> OriginLookup {
        self.reap();
        let Some(chat) = origin.chat() else {
            // Only chat origins are owned; anything else answers empty, and
            // a caller has no reason to ask.
            return OriginLookup::default();
        };
        let (handles, creating) = {
            let state = self.lock();
            let handles = state
                .chat_live
                .iter()
                .filter(|(_, owner)| **owner == chat)
                .filter_map(|(id, _)| state.sessions.get(id).map(SessionOwner::handle))
                .collect::<Vec<_>>();
            let creating = state
                .chat_reservations
                .values()
                .filter(|owner| **owner == chat)
                .count();
            (handles, creating)
        };
        OriginLookup {
            live: handles
                .into_iter()
                .filter(|handle| !handle.is_ended())
                .map(|handle| handle.summary())
                .collect(),
            creating: creating.try_into().unwrap_or(u32::MAX),
        }
    }

    /// Spawn under a reservation and complete it in the same critical
    /// section as the insert, so there is no instant at which the session
    /// is in the map and the key is not live, or the reverse.
    pub fn create_reserved(
        &self,
        reservation: CreationReservation<'_>,
        create: SessionCreate,
    ) -> Result<SessionHandle, SessionFailure> {
        self.admission_checks(&create)?;
        let owner = SessionOwner::spawn(create, self.config.clone())?;
        self.admit_spawned(owner, Some(reservation))
    }

    /// The non-creating lookup (§7.6). Never spawns, never retires, never
    /// takes a lease. Answers from the ledger and the map under one lock,
    /// then reads the summary outside it.
    pub fn lookup_creation(&self, key: IdempotencyKey) -> CreationLookup {
        let (lookup, live) = {
            let state = self.lock();
            let live = state
                .creations
                .live_session(key)
                .and_then(|id| state.sessions.get(&id).map(SessionOwner::handle));
            (state.creations.lookup(key, |_| None), live)
        };
        match (lookup, live) {
            (CreationLookup::NoEvidence, Some(handle)) if handle.is_ended() => {
                CreationLookup::KnownEnded {
                    session: handle.summary(),
                }
            }
            (CreationLookup::NoEvidence, Some(handle)) => CreationLookup::Known {
                session: handle.summary(),
            },
            (answer, _) => answer,
        }
    }

    /// Fence the key and say what it held at that moment. A caller that
    /// finds it live reattaches; one that finds nothing may start a fresh
    /// attempt under a new key, and a late create under this one is refused.
    pub fn retire_creation(&self, key: IdempotencyKey) -> CreationLookup {
        self.lock().creations.retire(key);
        self.lookup_creation(key)
    }

    /// Whether a fresh attempt has retired the key. The host asks before it
    /// consults its replay ledger: a retired key is refused whatever the
    /// ledger remembers, because the fence is the point.
    pub fn is_creation_retired(&self, key: IdempotencyKey) -> bool {
        self.lock().creations.is_retired(key)
    }

    fn admission_checks(&self, create: &SessionCreate) -> Result<(), SessionFailure> {
        let state = self.lock();
        if state.shutting_down {
            return Err(SessionFailure {
                code: ErrorCode::HostUnavailable,
                message: ErrorCode::HostUnavailable.as_str().to_owned(),
            });
        }
        if create
            .parent_session_id
            .is_some_and(|parent| !state.sessions.contains_key(&parent))
        {
            return Err(SessionFailure {
                code: ErrorCode::SessionNotFound,
                message: ErrorCode::SessionNotFound.as_str().to_owned(),
            });
        }
        Ok(())
    }

    /// Insert a spawned owner, completing its reservation under the same
    /// lock. `pub(crate)` so the in-crate scenarios can pause a creation
    /// between spawn and insert, which is the window §7.6 is about.
    pub(crate) fn admit_spawned(
        &self,
        owner: SessionOwner,
        reservation: Option<CreationReservation<'_>>,
    ) -> Result<SessionHandle, SessionFailure> {
        let handle = owner.handle();
        let mut state = self.lock();
        if state.shutting_down {
            drop(state);
            drop(owner);
            return Err(SessionFailure {
                code: ErrorCode::HostUnavailable,
                message: ErrorCode::HostUnavailable.as_str().to_owned(),
            });
        }
        state.sessions.insert(handle.id(), owner);
        state.generation = next_generation(state.generation);
        if let Some(mut reservation) = reservation {
            state.creations.complete(reservation.key, handle.id());
            // The chat's reservation becomes its live session in the same
            // critical section, so a lookup never sees the chat unowned in
            // between (protocol v16).
            if let Some(chat) = state.chat_reservations.remove(&reservation.key) {
                state.chat_live.insert(handle.id(), chat);
            }
            reservation.completed = true;
        }
        Ok(handle)
    }

    pub fn get(&self, id: SessionId) -> Result<SessionHandle, ErrorCode> {
        self.reap();
        self.lock()
            .sessions
            .get(&id)
            .map(SessionOwner::handle)
            .ok_or(ErrorCode::SessionNotFound)
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        self.catalog().1
    }

    /// A membership snapshot for compare-and-consent. The generation changes
    /// only when the active set changes, never for output, viewport, or
    /// controller churn.
    pub fn catalog(&self) -> (SessionCatalogGeneration, Vec<SessionSummary>) {
        self.reap();
        let (generation, handles) = {
            let state = self.lock();
            (
                state.generation,
                state
                    .sessions
                    .values()
                    .map(SessionOwner::handle)
                    .collect::<Vec<_>>(),
            )
        };
        (
            generation,
            handles.into_iter().map(|handle| handle.summary()).collect(),
        )
    }

    /// Atomically compare the consent epoch and stop accepting new sessions.
    /// Existing processes continue until the host's later graceful shutdown.
    pub fn prepare_shutdown(
        &self,
        observed: SessionCatalogGeneration,
    ) -> Result<(SessionCatalogGeneration, Vec<SessionSummary>), ErrorCode> {
        self.reap();
        let (generation, handles) = {
            let mut state = self.lock();
            if state.generation != observed {
                return Err(ErrorCode::StaleGeneration);
            }
            state.shutting_down = true;
            (
                state.generation,
                state
                    .sessions
                    .values()
                    .map(SessionOwner::handle)
                    .collect::<Vec<_>>(),
            )
        };
        Ok((
            generation,
            handles.into_iter().map(|handle| handle.summary()).collect(),
        ))
    }

    /// Re-open creation after an update handoff is refused or aborted.
    pub fn cancel_shutdown(&self) {
        self.lock().shutting_down = false;
    }

    /// Remove completed workers after their final event has committed. The
    /// ledger keeps each one's last summary, so a lookup after the reap
    /// still says the session ran and ended rather than nothing at all.
    pub fn reap(&self) {
        let ended: Vec<_> = {
            let state = self.lock();
            state
                .sessions
                .iter()
                .filter_map(|(id, owner)| {
                    let handle = owner.handle();
                    handle.is_ended().then_some((*id, handle))
                })
                .collect()
        };
        if ended.is_empty() {
            return;
        }
        let ended: Vec<_> = ended
            .into_iter()
            .map(|(id, handle)| (id, handle.summary()))
            .collect();
        let owners: Vec<_> = {
            let mut state = self.lock();
            let mut owners = Vec::with_capacity(ended.len());
            for (id, summary) in ended {
                if let Some(owner) = state.sessions.remove(&id) {
                    state.creations.ended(id, summary);
                    // An ended session frees its chat: the origin maps hold
                    // only what runs, which is what a lookup promises.
                    state.chat_live.remove(&id);
                    owners.push(owner);
                }
            }
            if !owners.is_empty() {
                state.generation = next_generation(state.generation);
            }
            owners
        };
        drop(owners);
    }

    /// Final host shutdown: stop all process trees outside the catalogue lock.
    pub fn shutdown(&self) {
        let owners = {
            let mut state = self.lock();
            state.shutting_down = true;
            let owners = std::mem::take(&mut state.sessions);
            if !owners.is_empty() {
                state.generation = next_generation(state.generation);
            }
            owners
        };
        drop(owners);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn insert_owner_for_test(&self, owner: SessionOwner) -> SessionHandle {
        let handle = owner.handle();
        self.lock().sessions.insert(handle.id(), owner);
        handle
    }
}

fn next_generation(current: SessionCatalogGeneration) -> SessionCatalogGeneration {
    current.next().unwrap_or(current)
}

impl Drop for SessionRegistry {
    fn drop(&mut self) {
        self.shutdown();
    }
}
