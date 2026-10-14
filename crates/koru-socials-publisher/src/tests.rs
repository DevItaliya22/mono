// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_socials::{Channel, DeliveryState, Destination, Network, Post, Publication};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use uuid::Uuid;

const NOW: i64 = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Journal(Phase, Vec<String>),
    Mirror(DeliveryState, Vec<String>),
    Prepare(String),
    Send(String, String, Option<String>),
}

type Events = Rc<RefCell<Vec<Event>>>;

#[derive(Default)]
struct Store {
    post: Option<Post>,
    saves: usize,
    fail_save: Option<usize>,
    cancel_after_sending_ack: bool,
}

struct MemoryRepository {
    state: Rc<RefCell<Store>>,
    events: Events,
}

impl Repository for MemoryRepository {
    fn load(&mut self, key: Key) -> Result<Option<Post>, Fault> {
        let post = self.state.borrow().post.clone();
        assert!(post.as_ref().is_none_or(|p| p.id == key.post));
        Ok(post)
    }

    fn save(&mut self, key: Key, post: &Post, base: &Post) -> Result<(), Fault> {
        let mut state = self.state.borrow_mut();
        state.saves += 1;
        if state.fail_save == Some(state.saves) {
            return Err(Fault::Store);
        }
        assert_eq!(
            state.post.as_ref(),
            Some(base),
            "save must use the latest base"
        );
        let delivery = &post
            .destinations
            .iter()
            .find(|d| d.channel.id == key.channel)
            .unwrap()
            .delivery;
        self.events.borrow_mut().push(Event::Mirror(
            delivery.state,
            delivery.published.iter().map(|p| p.id.clone()).collect(),
        ));
        state.post = Some(post.clone());
        if state.cancel_after_sending_ack && delivery.state == DeliveryState::Publishing {
            state.post.as_mut().unwrap().cancel_requested = true;
        }
        Ok(())
    }
}

#[derive(Default)]
struct MemoryJournal {
    attempt: Option<Attempt>,
    stores: usize,
    fail_store: Option<usize>,
    events: Events,
}

impl Journal for MemoryJournal {
    fn load(&mut self, _: Key) -> Result<Option<Attempt>, Fault> {
        Ok(self.attempt.clone())
    }

    fn store(&mut self, _: Key, value: &Attempt) -> Result<(), Fault> {
        self.stores += 1;
        if self.fail_store == Some(self.stores) {
            return Err(Fault::Ledger);
        }
        self.events.borrow_mut().push(Event::Journal(
            value.phase,
            value.published.iter().map(|p| p.id.clone()).collect(),
        ));
        self.attempt = Some(value.clone());
        Ok(())
    }
}

struct MemoryProvider {
    events: Events,
    repository: Rc<RefCell<Store>>,
    prepare_error: Option<ProviderError>,
    during_prepare: Option<fn(&mut Post)>,
    responses: VecDeque<Result<Publication, ProviderError>>,
}

impl Provider for MemoryProvider {
    fn prepare(
        &mut self,
        _target: &koru_socials::PublicationTarget,
        actor: &str,
    ) -> Result<(), ProviderError> {
        self.events.borrow_mut().push(Event::Prepare(actor.into()));
        if let Some(change) = self.during_prepare.take() {
            change(self.repository.borrow_mut().post.as_mut().unwrap());
        }
        match self.prepare_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn send(
        &mut self,
        _target: &koru_socials::PublicationTarget,
        actor: &str,
        text: &str,
        reply: Option<&str>,
    ) -> Result<Publication, ProviderError> {
        self.events.borrow_mut().push(Event::Send(
            actor.into(),
            text.into(),
            reply.map(str::to_owned),
        ));
        self.responses
            .pop_front()
            .expect("unexpected provider send")
    }
}

type TestEngine = Engine<MemoryRepository, MemoryJournal, MemoryProvider>;

#[test]
fn additional_networks_publish_once_with_frozen_targets() {
    for network in [
        Network::LinkedInPerson,
        Network::LinkedInPage,
        Network::Reddit,
    ] {
        let mut f = Fixture::new();
        f.change(|p| {
            p.body = "Long text supported on this network. ".repeat(15);
            let d = &mut p.destinations[0];
            d.channel.network = network;
            d.subreddit = "r/test".into();
            d.reddit_title = "A title".into();
            d.flair_id = Some("flair-1".into());
        });
        let expected = f.store.borrow().post.as_ref().unwrap().destinations[0].target();
        f.reply("provider-id");
        f.step(NOW).unwrap();
        assert_eq!(f.attempt().target, expected);
        assert_eq!(f.attempt().phase, Phase::Published);
        assert_eq!(f.sends().len(), 1);
        f.step(NOW + 1000).unwrap();
        assert_eq!(f.sends().len(), 1);
    }
}

#[test]
fn changing_reddit_target_during_preparation_cannot_redirect_a_post() {
    let mut f = Fixture::new();
    f.change(|p| {
        p.destinations[0].channel.network = Network::Reddit;
        p.destinations[0].subreddit = "test".into();
        p.destinations[0].reddit_title = "A title".into();
    });
    f.engine.provider.during_prepare =
        Some(|p| p.destinations[0].subreddit = "another_community".into());
    f.step(NOW).unwrap();
    assert!(f.sends().is_empty());
    assert_eq!(f.attempt().phase, Phase::Failed);
    assert_eq!(
        f.attempt().target,
        koru_socials::PublicationTarget::Reddit {
            subreddit: "test".into(),
            title: "A title".into(),
            flair_id: None
        }
    );
}

#[test]
fn one_failed_destination_does_not_resend_a_confirmed_destination() {
    let f = Fixture::new();
    let x_key = f.key;
    let mut reddit_key = x_key;
    reddit_key.channel = Uuid::from_u128(88);
    f.change(|p| {
        let mut d = Destination::new(Channel::new(Network::Reddit, "Reddit".into()));
        d.channel.id = reddit_key.channel;
        d.channel.remote_actor = Some("actor-reddit".into());
        d.subreddit = "test".into();
        d.reddit_title = "Test".into();
        p.destinations.push(d);
    });
    let mut engine = Engine {
        repository: f.engine.repository,
        provider: f.engine.provider,
        journal: SqliteLedger::from_connection(rusqlite::Connection::open_in_memory().unwrap())
            .unwrap(),
        device: f.engine.device,
    };
    engine
        .provider
        .responses
        .push_back(Ok(publication("x-confirmed")));
    engine
        .provider
        .responses
        .push_back(Err(ProviderError::Rejected));
    engine.step(x_key, NOW).unwrap();
    engine.step(reddit_key, NOW).unwrap();
    f.store.borrow_mut().post.as_mut().unwrap().retry_request = Some(Uuid::new_v4());
    engine
        .provider
        .responses
        .push_back(Ok(publication("reddit-confirmed")));
    engine.step(x_key, NOW).unwrap();
    engine.step(reddit_key, NOW).unwrap();
    assert_eq!(
        f.events
            .borrow()
            .iter()
            .filter(|e| matches!(e, Event::Send(..)))
            .count(),
        3
    );
    assert_eq!(
        engine.journal.load(x_key).unwrap().unwrap().published[0].id,
        "x-confirmed"
    );
    assert_eq!(
        engine.journal.load(reddit_key).unwrap().unwrap().published[0].id,
        "reddit-confirmed"
    );
}

#[test]
fn cancellation_during_the_durable_sending_exchange_prevents_post() {
    let mut f = Fixture::new();
    f.store.borrow_mut().cancel_after_sending_ack = true;
    assert_eq!(f.engine.step(f.key, NOW), Ok(None));
    assert_eq!(
        f.engine.journal.attempt.as_ref().unwrap().phase,
        Phase::Cancelled
    );
    assert!(
        !f.events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::Send(..)))
    );
}

struct Fixture {
    engine: TestEngine,
    key: Key,
    events: Events,
    store: Rc<RefCell<Store>>,
}

impl Fixture {
    fn new() -> Self {
        let device = Uuid::from_u128(1);
        let mut channel = Channel::new(Network::X, "Test account".into());
        channel.id = Uuid::from_u128(2);
        channel.remote_actor = Some("actor-1".into());
        let mut post = Post::draft(0);
        post.id = Uuid::from_u128(3);
        post.body = "An intentional post".into();
        post.destinations.push(Destination::new(channel));
        post.execution_device = Some(device);
        post.scheduled = true;
        post.scheduled_at_ms = Some(NOW);
        let key = Key {
            workspace: Uuid::from_u128(4),
            post: post.id,
            channel: post.destinations[0].channel.id,
        };
        let store = Rc::new(RefCell::new(Store {
            post: Some(post),
            ..Store::default()
        }));
        let events = Events::default();
        Self {
            engine: Engine {
                repository: MemoryRepository {
                    state: store.clone(),
                    events: events.clone(),
                },
                journal: MemoryJournal {
                    events: events.clone(),
                    ..MemoryJournal::default()
                },
                provider: MemoryProvider {
                    events: events.clone(),
                    repository: store.clone(),
                    prepare_error: None,
                    during_prepare: None,
                    responses: VecDeque::new(),
                },
                device,
            },
            key,
            events,
            store,
        }
    }

    fn change(&self, change: impl FnOnce(&mut Post)) {
        change(self.store.borrow_mut().post.as_mut().unwrap());
    }

    fn step(&mut self, now: i64) -> Result<Option<i64>, Fault> {
        self.engine.step(self.key, now)
    }

    fn reply(&mut self, id: &str) {
        self.engine
            .provider
            .responses
            .push_back(Ok(publication(id)));
    }

    fn sends(&self) -> Vec<(String, String, Option<String>)> {
        self.events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                Event::Send(actor, text, reply) => {
                    Some((actor.clone(), text.clone(), reply.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn attempt(&self) -> &Attempt {
        self.engine.journal.attempt.as_ref().unwrap()
    }

    fn seed(&mut self, phase: Phase, parts: &[&str], published: &[&str]) {
        self.engine.journal.attempt = Some(Attempt {
            target: koru_socials::PublicationTarget::X,
            id: Uuid::from_u128(5),
            device: self.engine.device,
            actor: "actor-1".into(),
            parts: parts.iter().map(|p| (*p).into()).collect(),
            published: published.iter().map(|id| publication(id)).collect(),
            phase,
            next_at_ms: None,
            failures: 0,
            request: None,
            error: None,
        });
    }
}

fn publication(id: &str) -> Publication {
    Publication {
        id: id.into(),
        url: Some(format!("https://x.com/i/status/{id}")),
        published_at_ms: 0,
    }
}

#[test]
fn future_post_waits_and_due_post_sends_only_after_both_durable_intents() {
    let mut f = Fixture::new();
    assert_eq!(f.step(NOW - 1), Ok(Some(NOW)));
    assert!(f.events.borrow().is_empty());
    f.reply("remote-1");
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(
        f.sends(),
        vec![("actor-1".into(), "An intentional post".into(), None)]
    );
    let events = f.events.borrow();
    let journal = events
        .iter()
        .position(|e| *e == Event::Journal(Phase::Sending, vec![]))
        .unwrap();
    let mirror = events
        .iter()
        .position(|e| *e == Event::Mirror(DeliveryState::Publishing, vec![]))
        .unwrap();
    let send = events
        .iter()
        .position(|e| matches!(e, Event::Send(..)))
        .unwrap();
    assert!(journal < mirror && mirror < send);
    let confirmed = events
        .iter()
        .position(|e| *e == Event::Journal(Phase::Published, vec!["remote-1".into()]))
        .unwrap();
    let mirrored = events
        .iter()
        .position(|e| *e == Event::Mirror(DeliveryState::Published, vec!["remote-1".into()]))
        .unwrap();
    assert!(send < confirmed && confirmed < mirrored);
    assert_eq!(f.attempt().published[0].published_at_ms, NOW);
}

#[test]
fn another_device_and_legacy_missing_owner_do_not_prepare_or_send() {
    for owner in [Some(Uuid::from_u128(99)), None] {
        let mut f = Fixture::new();
        f.change(|p| p.execution_device = owner);
        assert_eq!(f.step(NOW), Ok(None));
        assert!(f.events.borrow().is_empty());
    }
}

#[test]
fn token_preparation_failure_never_sends_or_marks_an_uncertain_post() {
    for error in [ProviderError::NeedsConnection, ProviderError::Unconfirmed] {
        let mut f = Fixture::new();
        f.engine.provider.prepare_error = Some(error);
        assert_eq!(f.step(NOW), Ok(None));
        assert_eq!(f.attempt().phase, Phase::NeedsConnection);
        assert!(f.sends().is_empty());
        assert!(
            !f.events
                .borrow()
                .iter()
                .any(|e| matches!(e, Event::Journal(Phase::Sending, _)))
        );
    }
}

#[test]
fn failed_durable_intent_prevents_send() {
    for failing_write in [1, 2] {
        let mut f = Fixture::new();
        f.engine.journal.fail_store = Some(failing_write);
        assert_eq!(f.step(NOW), Err(Fault::Ledger));
        assert!(f.sends().is_empty());
    }
    let mut f = Fixture::new();
    f.store.borrow_mut().fail_save = Some(2);
    assert_eq!(f.step(NOW), Err(Fault::Store));
    assert!(f.sends().is_empty());
    assert_eq!(f.attempt().phase, Phase::Sending);
    assert_eq!(f.step(NOW + 1), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Unconfirmed);
    assert!(f.sends().is_empty());
}

#[test]
fn lost_send_response_is_unconfirmed_and_even_explicit_retry_does_not_resend() {
    let mut f = Fixture::new();
    f.engine
        .provider
        .responses
        .push_back(Err(ProviderError::Unconfirmed));
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Unconfirmed);
    assert_eq!(f.step(NOW + 100_000), Ok(None));
    f.change(|p| p.retry_request = Some(Uuid::from_u128(10)));
    assert_eq!(f.step(NOW + 200_000), Ok(None));
    assert_eq!(f.sends().len(), 1);
    assert!(f.attempt().published.is_empty());
}

#[test]
fn confirmed_part_survives_store_mirror_failure_and_is_not_sent_again() {
    let mut f = Fixture::new();
    f.reply("remote-1");
    f.store.borrow_mut().fail_save = Some(3);
    assert_eq!(f.step(NOW), Err(Fault::Store));
    assert_eq!(f.attempt().phase, Phase::Published);
    assert_eq!(f.attempt().published[0].id, "remote-1");
    assert_eq!(
        f.store.borrow().post.as_ref().unwrap().destinations[0]
            .delivery
            .state,
        DeliveryState::Publishing
    );
    assert_eq!(f.step(NOW + 1), Ok(None));
    assert_eq!(f.sends().len(), 1);
    assert_eq!(
        f.store.borrow().post.as_ref().unwrap().destinations[0]
            .delivery
            .published[0]
            .id,
        "remote-1"
    );
}

#[test]
fn crash_after_sending_was_durable_preserves_confirmed_prefix_without_resend() {
    let mut f = Fixture::new();
    f.seed(Phase::Sending, &["first", "second"], &["first-id"]);
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Unconfirmed);
    assert_eq!(f.attempt().published[0].id, "first-id");
    assert!(f.sends().is_empty());
    assert!(
        !f.events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::Prepare(_)))
    );
}

#[test]
fn thread_resumes_from_confirmed_part_with_frozen_body_and_reply_chain() {
    let mut f = Fixture::new();
    f.seed(
        Phase::Ready,
        &["first frozen", "second frozen", "third frozen"],
        &["first-id"],
    );
    f.change(|p| p.body = "later source edit".into());
    f.reply("second-id");
    f.reply("third-id");
    assert_eq!(f.step(NOW), Ok(Some(NOW + 1)));
    assert_eq!(f.sends().len(), 1, "one remote mutation per step");
    assert_eq!(f.step(NOW + 1), Ok(None));
    assert_eq!(
        f.sends(),
        vec![
            (
                "actor-1".into(),
                "second frozen".into(),
                Some("first-id".into())
            ),
            (
                "actor-1".into(),
                "third frozen".into(),
                Some("second-id".into())
            ),
        ]
    );
    assert_eq!(
        f.attempt()
            .published
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        vec!["first-id", "second-id", "third-id"]
    );
    let events = f.events.borrow();
    let second_saved = events.iter().position(|e| matches!(e, Event::Journal(_, ids) if ids.last().map(String::as_str) == Some("second-id"))).unwrap();
    let third_send = events
        .iter()
        .position(|e| matches!(e, Event::Send(_, text, _) if text == "third frozen"))
        .unwrap();
    assert!(second_saved < third_send);
}

#[test]
fn cancellation_during_preparation_prevents_remote_mutation() {
    for cancel in [
        (|p: &mut Post| p.scheduled = false) as fn(&mut Post),
        (|p: &mut Post| p.deleted = true) as fn(&mut Post),
    ] {
        let mut f = Fixture::new();
        f.engine.provider.during_prepare = Some(cancel);
        assert_eq!(f.step(NOW), Ok(None));
        assert_eq!(f.attempt().phase, Phase::Cancelled);
        assert!(f.sends().is_empty());
        assert_eq!(f.step(NOW + 1), Ok(None));
        assert!(f.sends().is_empty());
    }
}

#[test]
fn changed_actor_during_preparation_prevents_sending_to_the_previous_account() {
    let mut f = Fixture::new();
    f.engine.provider.during_prepare = Some(|post| {
        post.destinations[0].channel.remote_actor = Some("actor-2".into());
    });
    assert_eq!(f.step(NOW), Ok(None));
    assert!(f.sends().is_empty());
    assert!(
        !f.events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::Journal(Phase::Sending, _)))
    );
}

#[test]
fn changed_network_during_preparation_prevents_sending_to_x() {
    let mut f = Fixture::new();
    f.engine.provider.during_prepare = Some(|post| {
        post.destinations[0].channel.network = Network::Reddit;
    });
    assert_eq!(f.step(NOW), Ok(None));
    assert!(f.sends().is_empty());
    assert!(
        !f.events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::Journal(Phase::Sending, _)))
    );
}

#[test]
fn rate_limit_deadline_prevents_early_flush_even_after_an_explicit_retry() {
    let mut f = Fixture::new();
    f.engine
        .provider
        .responses
        .push_back(Err(ProviderError::Retryable {
            retry_after_ms: 90_000,
        }));
    assert_eq!(f.step(NOW), Ok(Some(NOW + 90_000)));
    f.change(|p| p.retry_request = Some(Uuid::from_u128(10)));
    let event_count = f.events.borrow().len();
    assert_eq!(f.step(NOW + 89_999), Ok(Some(NOW + 90_000)));
    assert_eq!(
        f.events.borrow().len(),
        event_count,
        "waiting must not prepare or write again"
    );
    f.reply("remote-1");
    assert_eq!(f.step(NOW + 90_000), Ok(None));
    assert_eq!(f.sends().len(), 2);
    assert_eq!(f.sends()[0], f.sends()[1]);
}

#[test]
fn retry_after_longer_than_one_hour_is_not_shortened() {
    let mut f = Fixture::new();
    f.engine
        .provider
        .responses
        .push_back(Err(ProviderError::Retryable {
            retry_after_ms: 7_200_000,
        }));
    let at = f.step(NOW).unwrap().unwrap();
    assert!(
        at >= NOW + 7_200_000,
        "provider Retry-After is a minimum deadline"
    );
    assert_eq!(f.step(NOW + 3_600_000), Ok(Some(at)));
    assert_eq!(f.sends().len(), 1);
}

#[test]
fn terminal_rejection_requires_new_retry_nonce_and_preserves_frozen_content() {
    let mut f = Fixture::new();
    f.engine
        .provider
        .responses
        .push_back(Err(ProviderError::Rejected));
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Failed);
    f.change(|p| {
        p.updated_at_ms += 1;
        p.body = "edited after failure".into();
    });
    assert_eq!(f.step(NOW + 1), Ok(None));
    assert_eq!(f.sends().len(), 1);
    let retry = Uuid::from_u128(10);
    f.change(|p| p.retry_request = Some(retry));
    f.engine
        .provider
        .responses
        .push_back(Err(ProviderError::Rejected));
    assert_eq!(f.step(NOW + 2), Ok(None));
    assert_eq!(f.attempt().request, Some(retry));
    assert_eq!(f.sends().len(), 2);
    assert_eq!(f.sends()[0], f.sends()[1]);
    assert_eq!(f.step(NOW + 3), Ok(None));
    assert_eq!(f.sends().len(), 2, "the same nonce is consumed only once");
}

#[test]
fn clearing_retry_nonce_does_not_authorize_another_send() {
    let mut f = Fixture::new();
    let retry = Uuid::from_u128(10);
    f.change(|post| post.retry_request = Some(retry));
    f.engine
        .provider
        .responses
        .push_back(Err(ProviderError::Rejected));
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(f.attempt().request, Some(retry));
    f.change(|post| post.retry_request = None);
    assert_eq!(f.step(NOW + 1), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Failed);
    assert_eq!(f.attempt().request, Some(retry));
    assert_eq!(f.sends().len(), 1);
}

#[test]
fn cancellation_before_due_is_acknowledged_without_preparing_credentials() {
    let mut f = Fixture::new();
    f.change(|post| post.cancel_requested = true);
    assert_eq!(f.step(NOW - 1), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Cancelled);
    assert_eq!(
        f.store.borrow().post.as_ref().unwrap().destinations[0]
            .delivery
            .state,
        DeliveryState::Cancelled
    );
    assert!(
        !f.events
            .borrow()
            .iter()
            .any(|event| matches!(event, Event::Prepare(_) | Event::Send(..)))
    );
    let events = f.events.borrow();
    let durable = events
        .iter()
        .position(|event| matches!(event, Event::Journal(Phase::Cancelled, _)))
        .unwrap();
    let visible = events
        .iter()
        .position(|event| matches!(event, Event::Mirror(DeliveryState::Cancelled, _)))
        .unwrap();
    assert!(
        durable < visible,
        "acknowledgement follows the durable cancellation"
    );
    drop(events);
    assert_eq!(f.step(NOW + 1), Ok(None));
    assert!(f.sends().is_empty());
}

#[test]
fn cancellation_requested_during_preparation_prevents_send() {
    let mut f = Fixture::new();
    f.engine.provider.during_prepare = Some(|post| post.cancel_requested = true);
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Cancelled);
    assert!(f.sends().is_empty());
}

#[test]
fn lost_ledger_recovers_frozen_evidence_as_unconfirmed_and_never_resends() {
    for confirmed in [false, true] {
        let mut f = Fixture::new();
        let frozen = vec!["first frozen".to_owned(), "second frozen".to_owned()];
        let evidence = if confirmed {
            vec![publication("remote-1")]
        } else {
            vec![]
        };
        f.change(|post| {
            post.body = "body is not the frozen request".into();
            let delivery = &mut post.destinations[0].delivery;
            delivery.content = Some(frozen.clone());
            delivery.published = evidence.clone();
            delivery.state = DeliveryState::Publishing;
        });
        assert_eq!(f.step(NOW), Ok(None));
        assert_eq!(f.attempt().phase, Phase::Unconfirmed);
        assert_eq!(f.attempt().parts, frozen);
        assert_eq!(f.attempt().published, evidence);
        let post = f.store.borrow().post.clone().unwrap();
        assert_eq!(
            post.destinations[0].delivery.state,
            DeliveryState::Unconfirmed
        );
        assert_eq!(post.destinations[0].delivery.content, Some(frozen));
        assert_eq!(post.destinations[0].delivery.published, evidence);
        let events = f.events.borrow();
        let durable = events
            .iter()
            .position(|event| matches!(event, Event::Journal(Phase::Unconfirmed, _)))
            .unwrap();
        let visible = events
            .iter()
            .position(|event| matches!(event, Event::Mirror(DeliveryState::Unconfirmed, _)))
            .unwrap();
        assert!(durable < visible);
        drop(events);
        f.change(|post| post.retry_request = Some(Uuid::from_u128(10)));
        assert_eq!(f.step(NOW + 1), Ok(None));
        assert!(
            !f.events
                .borrow()
                .iter()
                .any(|event| matches!(event, Event::Prepare(_) | Event::Send(..)))
        );
    }
}

#[test]
fn lost_ledger_preserves_confirmed_ids_even_without_a_frozen_payload() {
    let mut f = Fixture::new();
    let evidence = vec![publication("remote-1")];
    f.change(|post| {
        let delivery = &mut post.destinations[0].delivery;
        delivery.published = evidence.clone();
        delivery.state = DeliveryState::Failed;
    });
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Unconfirmed);
    assert_eq!(f.attempt().published, evidence);
    assert_eq!(
        f.store.borrow().post.as_ref().unwrap().destinations[0]
            .delivery
            .published,
        evidence
    );
    assert!(
        !f.events
            .borrow()
            .iter()
            .any(|event| matches!(event, Event::Prepare(_) | Event::Send(..)))
    );
}

#[test]
fn lost_ledger_preserves_completed_delivery_without_downgrading_or_sending() {
    let mut f = Fixture::new();
    let frozen = vec!["already published".to_owned()];
    let evidence = vec![publication("remote-1")];
    f.change(|post| {
        let delivery = &mut post.destinations[0].delivery;
        delivery.content = Some(frozen.clone());
        delivery.published = evidence.clone();
        delivery.state = DeliveryState::Published;
    });
    assert_eq!(f.step(NOW), Ok(None));
    assert_eq!(f.attempt().phase, Phase::Published);
    assert_eq!(f.attempt().parts, frozen);
    assert_eq!(f.attempt().published, evidence);
    assert_eq!(
        f.store.borrow().post.as_ref().unwrap().destinations[0]
            .delivery
            .state,
        DeliveryState::Published
    );
    assert!(
        !f.events
            .borrow()
            .iter()
            .any(|event| matches!(event, Event::Prepare(_) | Event::Send(..)))
    );
}
