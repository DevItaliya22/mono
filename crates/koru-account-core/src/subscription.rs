// SPDX-License-Identifier: MPL-2.0

//! Billing coordination only; no workspace quota or content authority.
use crate::{KoruIdentityId, PortFuture};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    Incomplete,
    IncompleteExpired,
    Trialing,
    Active,
    PastDue,
    Canceled,
    Unpaid,
    Paused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubscriptionTarget {
    Checkout(String),
    Subscription(String),
    Invoice(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BillingError {
    Invalid,
    Unauthorized,
    Conflict,
    Unavailable,
    Unsupported,
}

/// One authenticated Stripe delivery. JSON is retained for operator inspection,
/// never used as current entitlement authority. Do not log this value.
pub struct WebhookDelivery {
    pub event_id: String,
    pub event_type: String,
    pub livemode: bool,
    pub provider_created_at_millis: i64,
    pub received_at_millis: i64,
    pub payload: String,
    pub object_id: Option<String>,
    pub subscription_id: Option<String>,
    pub customer_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebhookCompletion {
    Processed,
    Ignored,
    Failed(BillingError),
}

pub trait WebhookRepository: Send + Sync {
    /// A separate cross-replica notification lease. Never acquire it while
    /// holding a subscription update; dropping it releases the lock.
    fn begin_notification<'a>(
        &'a self,
        key: &'a str,
    ) -> PortFuture<'a, Result<Box<dyn NotificationLease>, BillingError>>;
    /// Commit the received row before returning its independently minted ID.
    /// Repeated event IDs remain separate deliveries, including exact retries.
    fn receive_webhook(
        &self,
        delivery: WebhookDelivery,
    ) -> PortFuture<'_, Result<uuid::Uuid, BillingError>>;
    /// Commit the terminal outcome before the handler acknowledges delivery.
    fn complete_webhook(
        &self,
        receipt: uuid::Uuid,
        completion: WebhookCompletion,
        completed_at_millis: i64,
        subscription_id: Option<String>,
        customer_id: Option<String>,
    ) -> PortFuture<'_, Result<(), BillingError>>;
}

pub trait NotificationLease: Send {}

/// Authoritative provider snapshot. Purchase identity is immutable after insertion.
#[derive(Clone)]
pub struct SubscriptionSnapshot {
    pub subscription_id: String,
    pub customer_id: String,
    pub checkout_session_id: String,
    pub purchase_email: String,
    pub status: SubscriptionStatus,
    pub price_id: String,
    pub livemode: bool,
    pub invoice_paid: bool,
    pub current_period_end_millis: i64,
    pub cancel_at_period_end: bool,
    pub cancel_at_millis: Option<i64>,
    pub canceled_at_millis: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SubscriptionSummary {
    pub subscription_id: String,
    pub customer_id: String,
    pub status: SubscriptionStatus,
    pub cloud_access: bool,
    pub storage_allowance_bytes: u64,
    pub paid_through_millis: Option<i64>,
    pub cancel_at_period_end: bool,
    pub cancel_at_millis: Option<i64>,
    pub canceled_at_millis: Option<i64>,
    pub account_linked: bool,
}

impl SubscriptionSummary {
    /// A provider/lock wait can cross the paid boundary. Recheck immediately
    /// before delivery; this can only remove eligibility, never create it.
    pub fn at(mut self, now: i64) -> Self {
        if self.paid_through_millis.is_none_or(|end| end <= now)
            || self.cancel_at_millis.is_some_and(|end| end <= now)
        {
            self.cloud_access = false;
            self.storage_allowance_bytes = 0;
        }
        self
    }
}

impl SubscriptionSnapshot {
    pub fn summary(&self, account_linked: bool, now: i64) -> SubscriptionSummary {
        let access = account_linked
            && self.status == SubscriptionStatus::Active
            && self.invoice_paid
            && self.current_period_end_millis > now
            && self.cancel_at_millis.is_none_or(|end| end > now);
        SubscriptionSummary {
            subscription_id: self.subscription_id.clone(),
            customer_id: self.customer_id.clone(),
            status: self.status,
            cloud_access: access,
            storage_allowance_bytes: if access { 100_000_000_000 } else { 0 },
            paid_through_millis: self.invoice_paid.then_some(self.current_period_end_millis),
            cancel_at_period_end: self.cancel_at_period_end,
            cancel_at_millis: self.cancel_at_millis,
            canceled_at_millis: self.canceled_at_millis,
            account_linked,
        }
    }
}

pub trait SubscriptionProvider: Send + Sync {
    fn retrieve<'a>(
        &'a self,
        target: &'a SubscriptionTarget,
    ) -> PortFuture<'a, Result<SubscriptionSnapshot, BillingError>>;
}

/// The lease serializes the complete provider read and commit across replicas.
/// Dropping it rolls back. Implementations must bound lock acquisition.
pub trait SubscriptionUpdate: Send {
    fn commit(
        self: Box<Self>,
        snapshot: SubscriptionSnapshot,
        now: i64,
    ) -> PortFuture<'static, Result<SubscriptionSummary, BillingError>>;
}

pub trait SubscriptionRepository: Send + Sync {
    fn begin_update(&self) -> PortFuture<'_, Result<Box<dyn SubscriptionUpdate>, BillingError>>;
    fn for_account<'a>(
        &'a self,
        account: KoruIdentityId,
        now: i64,
        livemode: bool,
        price: &'a str,
    ) -> PortFuture<'a, Result<Option<SubscriptionSummary>, BillingError>>;
}

pub async fn reconcile(
    repository: &dyn SubscriptionRepository,
    provider: &dyn SubscriptionProvider,
    target: &SubscriptionTarget,
    now: i64,
) -> Result<SubscriptionSummary, BillingError> {
    let update = repository.begin_update().await?;
    let snapshot = provider.retrieve(target).await?;
    update.commit(snapshot, now).await
}
