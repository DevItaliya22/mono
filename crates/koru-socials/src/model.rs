// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    X,
    LinkedInPerson,
    LinkedInPage,
    Reddit,
}

impl Network {
    pub const ALL: [Self; 4] = [
        Self::X,
        Self::LinkedInPage,
        Self::LinkedInPerson,
        Self::Reddit,
    ];

    /// Product/provider proper names; surrounding prose is localized by the UI.
    pub fn brand(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::LinkedInPerson | Self::LinkedInPage => "LinkedIn",
            Self::Reddit => "Reddit",
        }
    }
}

/// Non-secret workspace channel metadata. Credentials never enter this value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub network: Network,
    pub name: String,
    pub remote_actor: Option<String>,
}

impl Channel {
    pub fn new(network: Network, name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            network,
            name,
            remote_actor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub workspace: Uuid,
    pub document: Uuid,
    pub title: String,
    pub follows: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    pub channel: Channel,
    /// None follows base content. Some("") is an intentionally empty override.
    pub override_body: Option<String>,
    pub subreddit: String,
    pub reddit_title: String,
    pub flair_id: Option<String>,
    pub flair_required: bool,
    pub delivery: Delivery,
}

/// Provider routing and metadata frozen with the first publication attempt.
/// Retrying a Reddit post must not silently change its community or title.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "network", rename_all = "snake_case")]
pub enum PublicationTarget {
    X,
    LinkedInPerson,
    LinkedInPage,
    Reddit {
        subreddit: String,
        title: String,
        flair_id: Option<String>,
    },
}
impl PublicationTarget {
    pub fn network(&self) -> Network {
        match self {
            Self::X => Network::X,
            Self::LinkedInPerson => Network::LinkedInPerson,
            Self::LinkedInPage => Network::LinkedInPage,
            Self::Reddit { .. } => Network::Reddit,
        }
    }
}

impl Destination {
    pub fn target(&self) -> PublicationTarget {
        match self.channel.network {
            Network::X => PublicationTarget::X,
            Network::LinkedInPerson => PublicationTarget::LinkedInPerson,
            Network::LinkedInPage => PublicationTarget::LinkedInPage,
            Network::Reddit => PublicationTarget::Reddit {
                subreddit: self.subreddit.trim().trim_start_matches("r/").to_owned(),
                title: self.reddit_title.clone(),
                flair_id: self.flair_id.clone(),
            },
        }
    }
    pub fn new(channel: Channel) -> Self {
        Self {
            channel,
            override_body: None,
            subreddit: String::new(),
            reddit_title: String::new(),
            flair_id: None,
            flair_required: false,
            delivery: Delivery::default(),
        }
    }

    pub fn body<'a>(&'a self, base: &'a str) -> &'a str {
        self.override_body.as_deref().unwrap_or(base)
    }

    pub fn parts(&self, base: &str) -> Vec<String> {
        let body = self.body(base);
        if self.channel.network == Network::X && self.override_body.is_none() {
            crate::split_thread(body)
        } else {
            vec![body.to_owned()]
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    #[serde(default)]
    pub target: Option<PublicationTarget>,
    pub state: DeliveryState,
    /// Confirmed parts are never automatically resent.
    pub published: Vec<Publication>,
    /// Frozen on the first attempt, shared by all retries of this destination.
    pub content: Option<Vec<String>>,
    pub attempt: Option<Uuid>,
    pub retry_at_ms: Option<i64>,
    /// A stable code, never an arbitrary provider response or credential.
    pub error_code: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    #[default]
    Pending,
    Publishing,
    Published,
    Failed,
    Unconfirmed,
    NeedsConnection,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publication {
    pub id: String,
    pub url: Option<String>,
    pub published_at_ms: i64,
}

/// Construct only provider-owned links from validated response identifiers.
/// The URL in a synced publication is display evidence, never navigation input.
pub fn publication_url(network: Network, id: &str) -> Option<String> {
    match network {
        Network::X
            if !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Some(format!("https://x.com/i/status/{id}"))
        }
        Network::Reddit => {
            let id = id.strip_prefix("t3_")?;
            (!id.is_empty()
                && id.len() <= 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
            .then(|| format!("https://www.reddit.com/comments/{id}"))
        }
        Network::LinkedInPerson | Network::LinkedInPage => {
            let suffix = id
                .strip_prefix("urn:li:share:")
                .or_else(|| id.strip_prefix("urn:li:ugcPost:"))?;
            (!suffix.is_empty() && suffix.len() <= 32 && suffix.bytes().all(|b| b.is_ascii_digit()))
                .then(|| format!("https://www.linkedin.com/feed/update/{id}/"))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Draft,
    Scheduled,
    Queued,
    Publishing,
    Published,
    Partial,
    Failed,
    Unconfirmed,
    NeedsConnection,
    Cancelled,
    Waiting,
    NeedsDevice,
    CancelRequested,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub destinations: Vec<Destination>,
    /// A draft may remember a picker value, without authorizing execution.
    pub scheduled_at_ms: Option<i64>,
    pub scheduled: bool,
    pub execution_device: Option<Uuid>,
    #[serde(default)]
    pub retry_request: Option<Uuid>,
    #[serde(default)]
    pub cancel_requested: bool,
    pub source: Option<Source>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub deleted: bool,
}

impl Post {
    pub fn draft(now_ms: i64) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: String::new(),
            body: String::new(),
            destinations: Vec::new(),
            scheduled_at_ms: None,
            scheduled: false,
            execution_device: None,
            retry_request: None,
            cancel_requested: false,
            source: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            deleted: false,
        }
    }

    pub fn status(&self, now_ms: i64) -> Status {
        if self.deleted {
            return Status::Deleted;
        }
        let states: Vec<_> = self.destinations.iter().map(|d| d.delivery.state).collect();
        if states.contains(&DeliveryState::Unconfirmed) {
            return Status::Unconfirmed;
        }
        if states.contains(&DeliveryState::Publishing) {
            return Status::Publishing;
        }
        if !states.is_empty() && states.iter().all(|s| *s == DeliveryState::Published) {
            return Status::Published;
        }
        if !states.is_empty()
            && states
                .iter()
                .all(|s| matches!(s, DeliveryState::Published | DeliveryState::Cancelled))
        {
            return Status::Cancelled;
        }
        if self.cancel_requested {
            return Status::CancelRequested;
        }
        if states.contains(&DeliveryState::Published)
            || self
                .destinations
                .iter()
                .any(|d| !d.delivery.published.is_empty())
        {
            return Status::Partial;
        }
        if states.contains(&DeliveryState::Failed) {
            return Status::Failed;
        }
        if states.contains(&DeliveryState::NeedsConnection) {
            return Status::NeedsConnection;
        }
        if states.contains(&DeliveryState::Cancelled) {
            return Status::Cancelled;
        }
        if !self.scheduled {
            return Status::Draft;
        }
        if self.execution_device.is_none() {
            return Status::NeedsDevice;
        }
        if self
            .destinations
            .iter()
            .any(|d| d.delivery.retry_at_ms.is_some_and(|at| at > now_ms))
        {
            return Status::Waiting;
        }
        if self.scheduled_at_ms.is_some_and(|at| at <= now_ms) {
            Status::Queued
        } else {
            Status::Scheduled
        }
    }

    pub fn editable(&self) -> bool {
        !self.deleted
            && !self.cancel_requested
            && self.destinations.iter().all(|d| {
                d.delivery.content.is_none()
                    && d.delivery.published.is_empty()
                    && !matches!(
                        d.delivery.state,
                        DeliveryState::Publishing | DeliveryState::Unconfirmed
                    )
            })
    }

    pub fn due_on(&self, device: Uuid, now_ms: i64) -> bool {
        !self.deleted
            && !self.cancel_requested
            && self.scheduled
            && self.execution_device == Some(device)
            && self.scheduled_at_ms.is_some_and(|at| at <= now_ms)
    }

    /// Freeze precisely one destination before issuing a remote mutation.
    /// The caller must durably persist this transition before sending.
    pub fn begin_attempt(&mut self, channel: Uuid, device: Uuid, now_ms: i64) -> bool {
        if !self.due_on(device, now_ms) {
            return false;
        }
        let Some(destination) = self
            .destinations
            .iter_mut()
            .find(|d| d.channel.id == channel)
        else {
            return false;
        };
        if matches!(
            destination.delivery.state,
            DeliveryState::Published
                | DeliveryState::Publishing
                | DeliveryState::Unconfirmed
                | DeliveryState::Cancelled
        ) || destination
            .delivery
            .retry_at_ms
            .is_some_and(|at| at > now_ms)
        {
            return false;
        }
        if destination.delivery.content.is_none() {
            destination.delivery.content = Some(destination.parts(&self.body));
        }
        destination.delivery.attempt = Some(Uuid::new_v4());
        destination.delivery.state = DeliveryState::Publishing;
        destination.delivery.error_code = None;
        true
    }

    /// An interrupted in-flight attempt may already exist remotely.
    pub fn recover_interrupted(&mut self) -> bool {
        let mut changed = false;
        for destination in &mut self.destinations {
            if destination.delivery.state == DeliveryState::Publishing {
                destination.delivery.state = DeliveryState::Unconfirmed;
                changed = true;
            }
        }
        changed
    }
}
