// SPDX-License-Identifier: MPL-2.0

//! One Profile's living attempt. Each call checks local readiness and performs
//! at most one ceremony/provisioning phase; only the scheduler changes the set.
use super::*;

pub(super) struct Effects<P: Port> {
    discovery: Option<Discovery<P::Candidate, P::Attempt>>,
    candidate: usize,
    uncertain: bool,
    completion: Option<Completion<P::Completion>>,
    phase: Phase,
}

#[derive(Clone, Copy)]
enum Phase {
    Reconcile,
    Discover,
    Enroll,
    ValidateCompletion,
    Provision,
    Remove,
}

impl<P: Port> Default for Effects<P> {
    fn default() -> Self {
        Self {
            discovery: None,
            candidate: 0,
            uncertain: false,
            completion: None,
            phase: Phase::Reconcile,
        }
    }
}

pub(super) struct EffectResult<P: Port> {
    pub effects: Effects<P>,
    pub failure: Option<Failure>,
    pub waiting_code: Option<&'static str>,
    pub durable_revision: Option<u64>,
    pub remove: bool,
    pub interrupted: bool,
    pub cloud_primary: bool,
}

impl<P: Port> Effects<P> {
    pub fn complete(&self) -> bool {
        self.completion.is_some()
    }

    pub fn step(
        mut self,
        port: &mut P,
        profile: ProfileId,
        credential: &P::Credential,
        cancel: &AtomicBool,
    ) -> EffectResult<P> {
        let mut failure = None;
        let mut waiting_code = None;
        let mut remove = false;
        let mut cloud_primary = false;
        if !cancel.load(Ordering::Acquire) {
            failure = port.prepare_effect(profile, credential).err();
        }
        // Reconcile durable trust first. Never abandon an uncertain Join or
        // a committed receipt merely because the person enabled Cloud.
        if !cancel.load(Ordering::Acquire)
            && failure.is_none()
            && matches!(self.phase, Phase::Discover | Phase::Enroll)
            && !self.uncertain
        {
            match port.cloud_primary(profile, credential) {
                Ok(primary) => cloud_primary = primary,
                Err(error) => failure = Some(error),
            }
        }
        if !cancel.load(Ordering::Acquire) && failure.is_none() && !cloud_primary {
            match self.phase {
                Phase::Reconcile => match port.reconcile(profile) {
                    Ok(Some(completion)) => {
                        self.completion = Some(completion);
                        self.phase = Phase::ValidateCompletion;
                    }
                    Ok(None) => self.phase = Phase::Discover,
                    Err(error) => failure = Some(error),
                },
                Phase::Discover => {
                    let discovery = match self.discovery.as_ref() {
                        Some(previous) => {
                            port.refresh_discovery(profile, credential, &previous.attempt, cancel)
                        }
                        None => port.discover(profile, credential, cancel),
                    };
                    match discovery {
                        Ok(discovery) if discovery.candidates.is_empty() => {
                            waiting_code = Some(if discovery.only_older_peers {
                                OLDER_PEER
                            } else {
                                NO_PEER_ONLINE
                            });
                            self.discovery = Some(discovery);
                        }
                        Ok(discovery) => {
                            self.discovery = Some(discovery);
                            self.candidate = 0;
                            self.uncertain = false;
                            self.phase = Phase::Enroll;
                        }
                        Err(error) => failure = Some(error),
                    }
                }
                Phase::Enroll => {
                    let discovery = self.discovery.as_ref().expect("ceremony proof retained");
                    if !self.uncertain && port.attempt_expired(&discovery.attempt) {
                        self.discovery = None;
                        self.phase = Phase::Discover;
                    } else {
                        match port.enroll(
                            profile,
                            &discovery.candidates[self.candidate],
                            &discovery.attempt,
                            cancel,
                        ) {
                            Ok(completion) => {
                                self.completion = Some(completion);
                                self.discovery = None;
                                self.phase = Phase::ValidateCompletion;
                            }
                            Err(Failure::Uncertain) => {
                                self.uncertain = true;
                                failure = Some(Failure::Uncertain);
                            }
                            Err(Failure::Persistence) => {
                                // The responder already completed; local commit
                                // failure cannot make a different peer safe.
                                self.uncertain = true;
                                failure = Some(Failure::Persistence);
                            }
                            Err(error @ (Failure::Offline | Failure::Unavailable)) => {
                                if self.uncertain || cancel.load(Ordering::Acquire) {
                                    // Uncertainty belongs to the original Join, not
                                    // whichever later connection error arrived.
                                    failure = Some(error);
                                } else {
                                    self.candidate += 1;
                                    if self.candidate == discovery.candidates.len() {
                                        self.phase = Phase::Discover;
                                        waiting_code = Some(NO_PEER_ONLINE);
                                    }
                                }
                            }
                            Err(error) => {
                                if error == Failure::BadAttestation {
                                    // Conclusive proof refusal/expiry ends this
                                    // attempt. A manual retry can obtain new proof.
                                    self.discovery = None;
                                    self.phase = Phase::Discover;
                                    self.uncertain = false;
                                }
                                failure = Some(error);
                            }
                        }
                    }
                }
                Phase::ValidateCompletion => {
                    let completion = self.completion.as_ref().expect("durable completion");
                    match port.completion_current(profile, &completion.value) {
                        Ok(true) => self.phase = Phase::Provision,
                        Ok(false) => {
                            self.completion = None;
                            self.phase = Phase::Reconcile;
                        }
                        Err(error) => failure = Some(error),
                    }
                }
                Phase::Provision => {
                    let completion = self.completion.as_ref().expect("durable completion");
                    match port.provision(profile, &completion.value, credential) {
                        Ok(()) => self.phase = Phase::Remove,
                        Err(error) => failure = Some(error),
                    }
                }
                Phase::Remove => remove = true,
            }
        }
        EffectResult {
            durable_revision: self.completion.as_ref().map(|value| value.durable_revision),
            effects: self,
            failure,
            waiting_code,
            remove,
            interrupted: cancel.load(Ordering::Acquire),
            cloud_primary,
        }
    }
}
