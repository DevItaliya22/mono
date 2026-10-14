// SPDX-License-Identifier: MPL-2.0

use koru_socials::{Delivery, DeliveryState, Post, Publication, PublicationTarget};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub workspace: Uuid,
    pub post: Uuid,
    pub channel: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Ready,
    Sending,
    Published,
    Failed,
    NeedsConnection,
    Unconfirmed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub target: PublicationTarget,
    pub id: Uuid,
    pub device: Uuid,
    pub actor: String,
    pub parts: Vec<String>,
    pub published: Vec<Publication>,
    pub phase: Phase,
    pub next_at_ms: Option<i64>,
    pub failures: u32,
    pub request: Option<Uuid>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Store,
    Ledger,
}

pub trait Repository {
    fn load(&mut self, key: Key) -> Result<Option<Post>, Fault>;
    fn save(&mut self, key: Key, post: &Post, base: &Post) -> Result<(), Fault>;
}
pub trait Journal {
    fn load(&mut self, key: Key) -> Result<Option<Attempt>, Fault>;
    /// A successful return means this exact transition is durable.
    fn store(&mut self, key: Key, value: &Attempt) -> Result<(), Fault>;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    NeedsConnection,
    Retryable { retry_after_ms: u64 },
    Rejected,
    Unconfirmed,
    Unavailable,
}
pub trait Provider {
    fn prepare(&mut self, target: &PublicationTarget, actor: &str) -> Result<(), ProviderError>;
    fn send(
        &mut self,
        target: &PublicationTarget,
        actor: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Publication, ProviderError>;
}

pub struct Engine<S, L, P> {
    pub repository: S,
    pub journal: L,
    pub provider: P,
    pub device: Uuid,
}

impl<S: Repository, L: Journal, P: Provider> Engine<S, L, P> {
    /// At most one external mutation per step. No clock, sleeps, sockets or
    /// files are hidden here; tests drive every crash boundary with ports.
    pub fn step(&mut self, key: Key, now: i64) -> Result<Option<i64>, Fault> {
        self.step_with_clock(key, || now)
    }

    pub fn step_with_clock(
        &mut self,
        key: Key,
        clock: impl Fn() -> i64,
    ) -> Result<Option<i64>, Fault> {
        let now = clock();
        let Some(post) = self.repository.load(key)? else {
            return Ok(None);
        };
        // Ownership is assigned at creation and never taken over by a replica.
        if post.execution_device != Some(self.device) {
            return Ok(None);
        }
        let Some(destination) = post
            .destinations
            .iter()
            .find(|d| d.channel.id == key.channel)
        else {
            return Ok(None);
        };
        let prior = self.journal.load(key)?;
        if prior.is_none() && (!post.scheduled || post.deleted) && !post.cancel_requested {
            return Ok(None);
        }
        if prior.is_none() && !post.due_on(self.device, now) && !post.cancel_requested {
            return Ok(post.scheduled_at_ms);
        }
        let mut attempt = match prior {
            Some(a) => a,
            None => {
                // Legacy/foreign delivery evidence must never be interpreted
                // as an unused local attempt after a lost/deleted ledger.
                if destination.delivery.content.is_some()
                    || !destination.delivery.published.is_empty()
                {
                    let a = Attempt {
                        target: destination
                            .delivery
                            .target
                            .clone()
                            .unwrap_or_else(|| destination.target()),
                        id: destination.delivery.attempt.unwrap_or_else(Uuid::new_v4),
                        device: self.device,
                        actor: destination.channel.remote_actor.clone().unwrap_or_default(),
                        parts: destination
                            .delivery
                            .content
                            .clone()
                            .unwrap_or_else(|| destination.parts(&post.body)),
                        published: destination.delivery.published.clone(),
                        phase: if destination.delivery.state == DeliveryState::Published {
                            Phase::Published
                        } else {
                            Phase::Unconfirmed
                        },
                        next_at_ms: None,
                        failures: 0,
                        request: post.retry_request,
                        error: Some("local_record_missing".into()),
                    };
                    self.journal.store(key, &a)?;
                    self.mirror(key, &a)?;
                    return Ok(None);
                }
                let parts = destination.parts(&post.body);
                let valid = !parts.is_empty()
                    && parts.iter().all(|p| !p.trim().is_empty())
                    && !koru_socials::validate(&post, now)
                        .iter()
                        .any(|(id, _)| *id == Some(key.channel));
                let actor = destination.channel.remote_actor.clone().unwrap_or_default();
                let phase = if post.cancel_requested {
                    Phase::Cancelled
                } else if !valid {
                    Phase::Failed
                } else if actor.is_empty() {
                    Phase::NeedsConnection
                } else {
                    Phase::Ready
                };
                let a = Attempt {
                    target: destination.target(),
                    id: Uuid::new_v4(),
                    device: self.device,
                    actor,
                    parts,
                    published: vec![],
                    phase,
                    next_at_ms: None,
                    failures: 0,
                    request: post.retry_request,
                    error: (!valid).then(|| "invalid_content".into()),
                };
                self.journal.store(key, &a)?;
                a
            }
        };
        if attempt.device != self.device {
            return Ok(None);
        }
        // A persisted Sending state belongs to a previous interrupted step.
        // There is no proof the external mutation did not occur.
        if attempt.phase == Phase::Sending {
            attempt.phase = Phase::Unconfirmed;
            attempt.error = Some("response_unknown".into());
            attempt.next_at_ms = None;
            self.journal.store(key, &attempt)?;
        }
        if matches!(
            attempt.phase,
            Phase::Published | Phase::Unconfirmed | Phase::Cancelled
        ) {
            self.mirror(key, &attempt)?;
            return Ok(None);
        }
        if post.deleted || !post.scheduled || post.cancel_requested {
            attempt.phase = Phase::Cancelled;
            attempt.next_at_ms = None;
            self.journal.store(key, &attempt)?;
            self.mirror(key, &attempt)?;
            return Ok(None);
        }
        if post.retry_request.is_some()
            && post.retry_request != attempt.request
            && matches!(attempt.phase, Phase::Failed | Phase::NeedsConnection)
        {
            attempt.phase = Phase::Ready;
            attempt.request = post.retry_request;
            attempt.failures = 0;
            attempt.next_at_ms = None;
            attempt.error = None;
            self.journal.store(key, &attempt)?;
        }
        self.mirror(key, &attempt)?;
        if attempt.phase != Phase::Ready {
            return Ok(None);
        }
        if let Some(at) = attempt.next_at_ms.filter(|at| *at > now) {
            return Ok(Some(at));
        }
        if !post.due_on(self.device, now) {
            return Ok(post.scheduled_at_ms);
        }
        if destination.channel.remote_actor.as_deref() != Some(attempt.actor.as_str())
            || destination.target() != attempt.target
        {
            return self.failed(key, attempt, ProviderError::Rejected, now);
        }
        if let Err(error) = self.provider.prepare(&attempt.target, &attempt.actor) {
            // Preparation never posts. Even an ambiguous token response cannot
            // mean a social post was published.
            return self.failed(
                key,
                attempt,
                match error {
                    ProviderError::Unconfirmed => ProviderError::NeedsConnection,
                    other => other,
                },
                clock(),
            );
        }
        // Preparation may wait on the network. Observe cancellation/reschedule
        // again afterwards, before recording and issuing the remote request.
        let Some(latest) = self.repository.load(key)? else {
            return Ok(None);
        };
        let now = clock();
        if latest.execution_device != Some(self.device) {
            return Ok(None);
        }
        if latest.deleted || !latest.scheduled || latest.cancel_requested {
            attempt.phase = Phase::Cancelled;
            attempt.next_at_ms = None;
            self.journal.store(key, &attempt)?;
            self.mirror(key, &attempt)?;
            return Ok(None);
        }
        if !latest.due_on(self.device, now) {
            return Ok(latest.scheduled_at_ms);
        }
        if !latest.destinations.iter().any(|d| {
            d.channel.id == key.channel
                && d.target() == attempt.target
                && d.channel.remote_actor.as_deref() == Some(attempt.actor.as_str())
        }) {
            return self.failed(key, attempt, ProviderError::Rejected, now);
        }
        let index = attempt.published.len();
        let Some(text) = attempt.parts.get(index).cloned() else {
            return self.failed(key, attempt, ProviderError::Rejected, now);
        };
        attempt.phase = Phase::Sending;
        attempt.next_at_ms = None;
        self.journal.store(key, &attempt)?;
        // Do not issue anything until both local intent and synced frozen
        // content are acknowledged. Failure here is conservative on restart.
        self.mirror(key, &attempt)?;
        // The durable Store exchange can itself take time. Check once more
        // after it; cancellation observed here still precedes every POST byte.
        let Some(latest) = self.repository.load(key)? else {
            return Ok(None);
        };
        if latest.execution_device != Some(self.device) {
            return Ok(None);
        }
        if latest.cancel_requested || latest.deleted || !latest.scheduled {
            attempt.phase = Phase::Cancelled;
            self.journal.store(key, &attempt)?;
            self.mirror(key, &attempt)?;
            return Ok(None);
        }
        if !latest.destinations.iter().any(|d| {
            d.channel.id == key.channel
                && d.target() == attempt.target
                && d.channel.remote_actor.as_deref() == Some(attempt.actor.as_str())
        }) {
            return self.failed(key, attempt, ProviderError::Rejected, clock());
        }
        if !latest.due_on(self.device, clock()) {
            attempt.phase = Phase::Ready;
            self.journal.store(key, &attempt)?;
            self.mirror(key, &attempt)?;
            return Ok(latest.scheduled_at_ms);
        }
        match self.provider.send(
            &attempt.target,
            &attempt.actor,
            &text,
            attempt.published.last().map(|p| p.id.as_str()),
        ) {
            Ok(mut publication) => {
                publication.published_at_ms = clock();
                attempt.published.push(publication);
                attempt.failures = 0;
                attempt.error = None;
                attempt.phase = if attempt.published.len() == attempt.parts.len() {
                    Phase::Published
                } else {
                    Phase::Ready
                };
                // Persist the remote ID before another thread part or Store
                // update. A Store outage cannot cause a confirmed part resend.
                self.journal.store(key, &attempt)?;
                self.mirror(key, &attempt)?;
                Ok((attempt.phase == Phase::Ready).then_some(now.saturating_add(1)))
            }
            Err(error) => self.failed(key, attempt, error, clock()),
        }
    }

    fn failed(
        &mut self,
        key: Key,
        mut a: Attempt,
        error: ProviderError,
        now: i64,
    ) -> Result<Option<i64>, Fault> {
        a.failures = a.failures.saturating_add(1);
        let (phase, code, retry) = match error {
            ProviderError::NeedsConnection => (Phase::NeedsConnection, "needs_connection", None),
            ProviderError::Rejected => (Phase::Failed, "provider_rejected", None),
            ProviderError::Unconfirmed => (Phase::Unconfirmed, "response_unknown", None),
            ProviderError::Unavailable => (Phase::Ready, "provider_unavailable", Some(30_000)),
            ProviderError::Retryable { retry_after_ms } => {
                (Phase::Ready, "retry_wait", Some(retry_after_ms))
            }
        };
        a.phase = if retry.is_some() && a.failures >= 8 {
            Phase::Failed
        } else {
            phase
        };
        a.error = Some(code.into());
        a.next_at_ms = if a.phase == Phase::Ready {
            retry.map(|delay| {
                let backoff = 15_000u64.saturating_mul(1u64 << a.failures.min(8));
                now.saturating_add(delay.max(backoff).max(1_000).min(i64::MAX as u64) as i64)
            })
        } else {
            None
        };
        self.journal.store(key, &a)?;
        self.mirror(key, &a)?;
        Ok(a.next_at_ms)
    }

    fn mirror(&mut self, key: Key, a: &Attempt) -> Result<(), Fault> {
        let Some(base) = self.repository.load(key)? else {
            return Ok(());
        };
        if base.execution_device != Some(self.device) {
            return Err(Fault::Store);
        }
        let mut post = base.clone();
        let Some(d) = post
            .destinations
            .iter_mut()
            .find(|d| d.channel.id == key.channel)
        else {
            return Err(Fault::Store);
        };
        d.delivery = Delivery {
            target: Some(a.target.clone()),
            state: match a.phase {
                Phase::Ready => DeliveryState::Pending,
                Phase::Sending => DeliveryState::Publishing,
                Phase::Published => DeliveryState::Published,
                Phase::Failed => DeliveryState::Failed,
                Phase::NeedsConnection => DeliveryState::NeedsConnection,
                Phase::Unconfirmed => DeliveryState::Unconfirmed,
                Phase::Cancelled => DeliveryState::Cancelled,
            },
            published: a.published.clone(),
            content: Some(a.parts.clone()),
            attempt: Some(a.id),
            retry_at_ms: a.next_at_ms,
            error_code: a.error.clone(),
        };
        if post != base {
            self.repository.save(key, &post, &base)?;
        }
        Ok(())
    }
}
