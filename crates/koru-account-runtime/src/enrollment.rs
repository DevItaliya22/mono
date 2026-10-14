// SPDX-License-Identifier: MPL-2.0

//! Shared automatic Profile enrollment coordination.
//!
//! The coordinator is deliberately transport-neutral. Its production port
//! owns hosted account calls, the enrollment ALPN, and durable trust; tests
//! drive the same state machine with fake time and effects. The host loop
//! only reads a cached whole report and can therefore answer requests while any of
//! those effects are slow.

use crate::ports::{Clock, SystemClock};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use koru_account_core::ProfileId;
use koru_agent_wire::EnrollmentState;

mod control;
mod member;
mod production;
mod scheduler;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub use control::Control;

/// Object-safe adapter boundary for the same shared scheduler. Production and
/// injected host fixtures differ only in the effect port behind this driver.
pub trait Driver: Send + Sync {
    fn select_profiles(
        &self,
        profiles: BTreeSet<ProfileId>,
        authority: EnrollmentAuthorization,
    ) -> Result<Snapshot, &'static str>;
    fn retry(&self, authority: EnrollmentAuthorization, profile: Option<ProfileId>) -> bool;
    fn cancel_durable(&self) -> Result<(), &'static str>;
    fn cancel_durable_for(&self, authority: EnrollmentAuthorization) -> Result<(), &'static str>;
    fn update_authorization(&self, authority: EnrollmentAuthorization) -> bool;
    fn activate_owned(&self, authority: Option<EnrollmentAuthorization>, owner_scope: u64) -> bool;
    fn refresh_authorization(
        &self,
        previous: EnrollmentAuthorization,
        next: EnrollmentAuthorization,
    ) -> bool;
    fn set_foreground(&self, foreground: bool);
    fn wake(&self) -> bool;
    fn snapshot(&self) -> Snapshot;
}
pub use production::{ProductionPort, ProductionResponder, ResponderSessionGate};

/// Issued only after the account directory and vault agree on the session owner.
#[derive(Clone)]
pub struct EnrollmentAuthorization {
    pub access: koru_account_wire::AccessToken,
    pub owner: crate::identity::AccountOwner,
    pub namespace: String,
    pub record_version: i64,
    pub session_id: String,
}

const WAKE_CAPACITY: usize = 1;
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

pub const NO_PEER_ONLINE: &str = "no_peer_online";
pub const OLDER_PEER: &str = "older_peer";
pub const ACCESS_REVOKED: &str = "profile_access_revoked";
pub const SESSION_EXPIRED: &str = "session_revoked";
pub const BAD_ATTESTATION: &str = "attestation_invalid";
pub const PERSISTENCE_FAILED: &str = "trust_persistence_failed";
pub const PROVISIONING_PENDING: &str = "enrollment_provisioning_pending";
pub const ENROLLMENT_UNAVAILABLE: &str = "enrollment_unavailable";
pub const ENROLLMENT_RATE_LIMITED: &str = "enrollment_rate_limited";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub state: EnrollmentState,
    pub failure_code: Option<&'static str>,
    pub retry_at_millis: Option<i64>,
    /// Set only for the transition whose durable trust should be handed to the
    /// existing authorization refresh path.
    pub durable_revision: Option<u64>,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            state: EnrollmentState::Idle,
            failure_code: None,
            retry_at_millis: None,
            durable_revision: None,
        }
    }
}

/// One coherent cached observation of the worker's target and its report.
/// This is a host-side diagnostic value, not an Agent protocol message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub owner_scope: Option<u64>,
    pub account_id: Option<String>,
    pub profiles: Vec<ProfileReport>,
    /// Profile described by the report. Complete retains its attribution after
    /// the coordinator clears unfinished work; cancellation/Idle clears it.
    /// This projection never selects or restores an enrollment target.
    pub target: Option<ProfileId>,
    pub report: Report,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileReport {
    pub profile: ProfileId,
    pub report: Report,
}

impl ProfileReport {
    pub fn wire(&self) -> koru_agent_wire::AccountProfileEnrollment {
        koru_agent_wire::AccountProfileEnrollment {
            profile_id: self.profile.to_string(),
            state: self.report.state,
            failure_code: self.report.failure_code.map(str::to_owned),
            retry_at_millis: self.report.retry_at_millis,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Offline,
    AccessRevoked,
    SessionExpired,
    BadAttestation,
    Persistence,
    Unavailable,
    RateLimited(Duration),
    /// A Join may have committed remotely. Its exact candidate and proof must
    /// survive subsequent connection failures until resolution or expiry.
    Uncertain,
}

pub struct Discovery<Candidate, Attempt> {
    pub candidates: Vec<Candidate>,
    pub only_older_peers: bool,
    pub attempt: Attempt,
}

pub struct Completion<Value> {
    pub value: Value,
    pub durable_revision: u64,
}

/// Blocking effects run only on [`Control`]'s one bounded worker.
pub trait Port: Send + 'static {
    type Scope: Clone + Ord + Send + 'static;
    type Credential: Clone + Send + 'static;
    type Candidate: Clone + Send + 'static;
    type Attempt: Send + 'static;
    type Completion: Clone + Send + 'static;

    /// A fixed effect lane. It shares verified authorization snapshots with
    /// the set owner, but never writes the selected set itself.
    fn fork_effects(&self) -> Result<Self, Failure>
    where
        Self: Sized;

    fn scope(credential: &Self::Credential) -> Self::Scope;

    fn snapshot_account_id(_credential: &Self::Credential) -> Option<String> {
        None
    }

    /// An explicitly activated newer ready directory revision can replace an
    /// old incarnation; ordinary selection cannot upgrade its captured owner.
    fn authority_successor(_previous: &Self::Credential, _next: &Self::Credential) -> bool {
        false
    }

    /// Pure same-session continuity, before hosted revalidation or any visible
    /// report is relabeled with a new acknowledged directory revision.
    fn refresh_continuity(_previous: &Self::Credential, _next: &Self::Credential) -> bool {
        false
    }

    /// Explicit continuity from an acknowledged refresh, never inferred at
    /// startup. Verify the current ready version and unchanged target set in
    /// one transaction before rebinding retained progress.
    fn refresh_authority(
        &mut self,
        previous: &Self::Credential,
        next: &Self::Credential,
        expected: &BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure>;

    /// Load the complete account-owned set only after authenticated restoration.
    fn restored_targets(
        &mut self,
        credential: &Self::Credential,
    ) -> Result<BTreeSet<ProfileId>, Failure>;

    /// Compare and replace one captured account's complete set.
    fn replace_targets(
        &mut self,
        credential: &Self::Credential,
        expected: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure>;

    fn authorize_profile(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
    ) -> Result<bool, Failure>;

    fn same_authority(previous: &Self::Credential, next: &Self::Credential) -> bool;

    /// Cloud-enabled Profiles do not automatically establish new peer trust.
    /// Checked on an effect lane; a read failure must not authorize enrollment.
    fn cloud_primary(
        &mut self,
        _profile: ProfileId,
        _credential: &Self::Credential,
    ) -> Result<bool, Failure> {
        Ok(false)
    }

    /// Reconcile a completion that may have committed before a lost reply or
    /// process restart. It must never infer trust from hosted metadata.
    /// Check the current local authority before a lane starts its next effect.
    /// Temporary refusal must retain the member's exact attempt and receipt.
    fn prepare_effect(
        &mut self,
        _profile: ProfileId,
        _credential: &Self::Credential,
    ) -> Result<(), Failure> {
        Ok(())
    }

    fn reconcile(
        &mut self,
        profile: ProfileId,
    ) -> Result<Option<Completion<Self::Completion>>, Failure>;

    fn completion_current(
        &mut self,
        _profile: ProfileId,
        _completion: &Self::Completion,
    ) -> Result<bool, Failure> {
        Ok(true)
    }

    /// Obtain fresh attestation evidence and the bounded compatible roster.
    fn discover(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
        cancel: &AtomicBool,
    ) -> Result<Discovery<Self::Candidate, Self::Attempt>, Failure>;

    /// Refresh the roster after a conclusive offline result, retaining usable
    /// evidence. Uncertain ceremonies never enter this path.
    fn refresh_discovery(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
        _previous: &Self::Attempt,
        cancel: &AtomicBool,
    ) -> Result<Discovery<Self::Candidate, Self::Attempt>, Failure> {
        self.discover(profile, credential, cancel)
    }

    /// One candidate ceremony. The returned revision means reciprocal trust
    /// is already durable locally; transport success by itself is not success.
    fn enroll(
        &mut self,
        profile: ProfileId,
        candidate: &Self::Candidate,
        attempt: &Self::Attempt,
        cancel: &AtomicBool,
    ) -> Result<Completion<Self::Completion>, Failure>;

    /// Expired proof may be renewed only before an uncertain remote commit.
    fn attempt_expired(&self, _attempt: &Self::Attempt) -> bool {
        false
    }

    /// Hosted discovery metadata follows local durable trust and may retry
    /// independently without taking direct replication down.
    fn provision(
        &mut self,
        profile: ProfileId,
        completion: &Self::Completion,
        credential: &Self::Credential,
    ) -> Result<(), Failure>;

    /// Delete only provisional enrollment material, never established trust.
    fn discard(&mut self, profile: ProfileId);
}

fn failure_code(failure: Failure) -> &'static str {
    match failure {
        Failure::Offline => NO_PEER_ONLINE,
        Failure::AccessRevoked => ACCESS_REVOKED,
        Failure::SessionExpired => SESSION_EXPIRED,
        Failure::BadAttestation => BAD_ATTESTATION,
        Failure::Persistence => PERSISTENCE_FAILED,
        Failure::Unavailable | Failure::Uncertain => ENROLLMENT_UNAVAILABLE,
        Failure::RateLimited(_) => ENROLLMENT_RATE_LIMITED,
    }
}
fn failed(code: &'static str) -> Report {
    Report {
        state: EnrollmentState::Failed,
        failure_code: Some(code),
        retry_at_millis: None,
        durable_revision: None,
    }
}

fn checked_retry_at(now_millis: i64, delay: Duration) -> Option<i64> {
    i64::try_from(delay.as_millis())
        .ok()
        .and_then(|delay| now_millis.checked_add(delay))
}

#[cfg(test)]
mod tests;
