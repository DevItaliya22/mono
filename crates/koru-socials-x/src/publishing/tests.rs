// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::credential_lock::LockGuard;
use koru_oauth_credentials::SessionCredentialStore;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

const ACTOR: &str = "123456789";
type Events = Arc<Mutex<Vec<&'static str>>>;

fn response(status: u16, body: &str) -> Response {
    Response {
        status,
        body: Zeroizing::new(body.as_bytes().to_vec()),
        retry_after: None,
        reset: None,
    }
}
fn refreshed() -> Response {
    response(
        200,
        r#"{"access_token":"new-access","refresh_token":"new-refresh","token_type":"bearer","expires_in":7200,"scope":"tweet.read users.read tweet.write offline.access"}"#,
    )
}
fn identity() -> Response {
    response(
        200,
        r#"{"data":{"id":"123456789","username":"test_user","name":"Test User"}}"#,
    )
}

struct FakeTransport {
    responses: VecDeque<Result<Response, ()>>,
    events: Events,
    locked: Arc<AtomicBool>,
}
impl Transport for FakeTransport {
    fn request(&mut self, request: Request<'_>) -> Result<Response, ()> {
        assert!(
            self.locked.load(Ordering::Relaxed),
            "lock must cover every provider effect"
        );
        let event = match request {
            Request::Refresh { client_id, refresh } => {
                assert_eq!(client_id, "client");
                assert!(!refresh.is_empty());
                "refresh"
            }
            Request::Identity { access } => {
                assert_eq!(access, "new-access");
                "identity"
            }
            Request::Post { access, .. } => {
                assert_eq!(access, "new-access");
                "post"
            }
        };
        self.events.lock().unwrap().push(event);
        self.responses
            .pop_front()
            .expect("an extra remote request must fail the test")
    }
}
struct FakeGuard(Arc<AtomicBool>);
impl LockGuard for FakeGuard {}
impl Drop for FakeGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}
struct FakeLocks {
    locked: Arc<AtomicBool>,
    busy: bool,
}
impl CredentialLocks for FakeLocks {
    fn acquire(&self, _: &CredentialEntry) -> Result<Box<dyn LockGuard>, LockError> {
        if self.busy {
            return Err(LockError::Busy);
        }
        assert!(!self.locked.swap(true, Ordering::Relaxed));
        Ok(Box::new(FakeGuard(self.locked.clone())))
    }
}
struct FakeVault {
    inner: SessionCredentialStore,
    events: Events,
    locked: Arc<AtomicBool>,
    refuse: AtomicBool,
}
impl CredentialStore for FakeVault {
    fn load(
        &self,
        entry: &CredentialEntry,
    ) -> Result<Option<SecretString>, koru_oauth_credentials::Error> {
        assert!(self.locked.load(Ordering::Relaxed));
        self.events.lock().unwrap().push("load");
        self.inner.load(entry)
    }
    fn store(
        &self,
        entry: &CredentialEntry,
        secret: SecretString,
    ) -> Result<(), koru_oauth_credentials::Error> {
        assert!(self.locked.load(Ordering::Relaxed));
        self.events.lock().unwrap().push("store");
        if self.refuse.load(Ordering::Relaxed) {
            return Err(koru_oauth_credentials::Error::Unavailable);
        }
        self.inner.store(entry, secret)
    }
    fn clear(&self, _: &CredentialEntry) -> Result<(), koru_oauth_credentials::Error> {
        panic!("publisher never erases credentials")
    }
}
fn fixture(responses: Vec<Result<Response, ()>>) -> (Publisher, Arc<FakeVault>, Events) {
    let context = CredentialContext::new(
        "internal-debug",
        Path::new("synthetic-root"),
        koru_device::DeviceId::parse("30000000-0000-4000-8000-000000000003").unwrap(),
    )
    .unwrap();
    let locked = Arc::new(AtomicBool::new(false));
    let events = Events::default();
    let vault = Arc::new(FakeVault {
        inner: SessionCredentialStore::default(),
        events: events.clone(),
        locked: locked.clone(),
        refuse: AtomicBool::new(false),
    });
    vault
        .inner
        .store(
            &actor_entry(&context, ACTOR).unwrap(),
            SecretString::new(
                r#"{"version":1,"client_id":"client","refresh_token":"old-refresh"}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let publisher = Publisher {
        context,
        transport: Box::new(FakeTransport {
            responses: responses.into(),
            events: events.clone(),
            locked: locked.clone(),
        }),
        vault: vault.clone(),
        locks: Box::new(FakeLocks {
            locked,
            busy: false,
        }),
        prepared: None,
    };
    (publisher, vault, events)
}

#[test]
fn rotation_is_durable_before_identity_and_one_send_consumes_its_grant() {
    let (mut publisher, vault, events) = fixture(vec![
        Ok(refreshed()),
        Ok(identity()),
        Ok(response(201, r#"{"data":{"id":"987654321"}}"#)),
    ]);
    publisher.prepare(ACTOR).unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        ["load", "refresh", "store", "identity"]
    );
    let record = vault
        .inner
        .load(&actor_entry(&publisher.context, ACTOR).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        RefreshRecord::decode(&record).unwrap().refresh_token,
        "new-refresh"
    );
    assert!(!record.expose().contains("new-access"));
    let posted = publisher.send(ACTOR, "hello", None).unwrap();
    assert_eq!(
        posted,
        Posted {
            id: "987654321".into(),
            url: "https://x.com/i/status/987654321".into()
        }
    );
    assert_eq!(
        publisher.send(ACTOR, "hello", None),
        Err(PublishError::NeedsConnection)
    );
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| **event == "post")
            .count(),
        1
    );
}

#[test]
fn identity_failure_or_mismatch_preserves_rotation_but_never_arms_send() {
    for answer in [
        Err(()),
        Ok(response(503, "")),
        Ok(response(
            200,
            r#"{"data":{"id":"555","username":"other","name":"Other"}}"#,
        )),
    ] {
        let (mut publisher, vault, events) = fixture(vec![Ok(refreshed()), answer]);
        assert!(publisher.prepare(ACTOR).is_err());
        assert_eq!(
            publisher.send(ACTOR, "hello", None),
            Err(PublishError::NeedsConnection)
        );
        let stored = vault
            .inner
            .load(&actor_entry(&publisher.context, ACTOR).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            RefreshRecord::decode(&stored).unwrap().refresh_token,
            "new-refresh"
        );
        assert!(!events.lock().unwrap().contains(&"post"));
    }
}

#[test]
fn vault_refusal_stops_before_identity_or_send() {
    let (mut publisher, vault, events) = fixture(vec![Ok(refreshed())]);
    vault.refuse.store(true, Ordering::Relaxed);
    assert_eq!(publisher.prepare(ACTOR), Err(PublishError::Unavailable));
    assert_eq!(
        publisher.send(ACTOR, "hello", None),
        Err(PublishError::NeedsConnection)
    );
    assert_eq!(*events.lock().unwrap(), ["load", "refresh", "store"]);
}

#[test]
fn revoked_changed_or_expired_prepared_grant_never_reaches_post() {
    for mode in ["removed", "changed", "expired", "wrong_actor"] {
        let (mut publisher, vault, events) = fixture(vec![Ok(refreshed()), Ok(identity())]);
        publisher.prepare(ACTOR).unwrap();
        let entry = actor_entry(&publisher.context, ACTOR).unwrap();
        match mode {
            "removed" => vault.inner.clear(&entry).unwrap(),
            "changed" => vault
                .inner
                .store(&entry, SecretString::new("different-generation").unwrap())
                .unwrap(),
            "expired" => publisher.prepared.as_mut().unwrap().expires = Instant::now(),
            _ => {}
        }
        let actor = if mode == "wrong_actor" { "999" } else { ACTOR };
        assert_eq!(
            publisher.send(actor, "hello", None),
            Err(PublishError::NeedsConnection)
        );
        assert!(!events.lock().unwrap().contains(&"post"));
    }
}

#[test]
fn uncertain_results_never_trigger_a_second_post() {
    for answer in [
        Err(()),
        Ok(response(500, "")),
        Ok(response(503, "")),
        Ok(response(408, "")),
        Ok(response(201, "bad-json")),
        Ok(response(201, r#"{"data":{"id":"1","id":"2"}}"#)),
    ] {
        let (mut publisher, _, events) = fixture(vec![Ok(refreshed()), Ok(identity()), answer]);
        publisher.prepare(ACTOR).unwrap();
        assert_eq!(
            publisher.send(ACTOR, "hello", None),
            Err(PublishError::Unconfirmed)
        );
        assert_eq!(
            publisher.send(ACTOR, "hello", None),
            Err(PublishError::NeedsConnection)
        );
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| **event == "post")
                .count(),
            1
        );
    }
}

#[test]
fn definite_refusals_have_safe_recovery_categories() {
    for (status, expected) in [
        (400, PublishError::Rejected),
        (401, PublishError::NeedsConnection),
        (403, PublishError::Rejected),
        (
            429,
            PublishError::Retryable {
                retry_after_ms: DEFAULT_RETRY_MS,
            },
        ),
    ] {
        let (mut publisher, _, _) = fixture(vec![
            Ok(refreshed()),
            Ok(identity()),
            Ok(response(status, "")),
        ]);
        publisher.prepare(ACTOR).unwrap();
        assert_eq!(publisher.send(ACTOR, "hello", None), Err(expected));
    }
    assert_eq!(classify_prepare(&response(500, ""), true), Err(retry()));
    assert_eq!(
        classify_prepare(&response(400, ""), true),
        Err(PublishError::NeedsConnection)
    );
}

#[test]
fn lock_contention_has_a_short_safe_retry_and_no_remote_effects() {
    let (mut publisher, _, events) = fixture(vec![]);
    publisher.locks = Box::new(FakeLocks {
        locked: Arc::new(AtomicBool::new(false)),
        busy: true,
    });
    assert_eq!(
        publisher.prepare(ACTOR),
        Err(PublishError::Retryable {
            retry_after_ms: 1000
        })
    );
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn request_payloads_encode_refresh_and_thread_reply_without_a_secret_parameter() {
    let form = refresh_form("client", "a+b&=refresh");
    let fields: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(form.as_bytes()).collect();
    assert_eq!(fields["grant_type"], "refresh_token");
    assert_eq!(fields["refresh_token"], "a+b&=refresh");
    assert!(!fields.contains_key("client_secret"));
    let reply: serde_json::Value =
        serde_json::from_str(&post_body("hello\nworld", Some("12345")).unwrap()).unwrap();
    assert_eq!(reply["text"], "hello\nworld");
    assert_eq!(reply["reply"]["in_reply_to_tweet_id"], "12345");
    let plain: serde_json::Value =
        serde_json::from_str(&post_body("hello", None).unwrap()).unwrap();
    assert!(plain.get("reply").is_none());
}

#[test]
fn retry_headers_preserve_provider_minimum_and_choose_the_later_reset() {
    let mut value = response(429, "");
    value.retry_after = Some("2".into());
    value.reset = Some("105".into());
    assert_eq!(retry_delay(&value, 100_000), 5000);
    value.retry_after = Some("172800".into());
    assert_eq!(retry_delay(&value, 100_000), 172_800_000);
    value.retry_after = Some(u64::MAX.to_string());
    assert_eq!(retry_delay(&value, 100_000), u64::MAX);
    value.retry_after = Some("9".repeat(100));
    assert_eq!(retry_delay(&value, 100_000), u64::MAX);
    value.retry_after = None;
    value.reset = Some("172900".into());
    assert_eq!(retry_delay(&value, 100_000), 172_800_000);
    value.reset = Some("105".into());
    value.retry_after = Some("Thu, 01 Jan 1970 00:02:00 GMT".into());
    assert_eq!(retry_delay(&value, 100_000), 20_000);
    value.retry_after = Some("untrusted-header".into());
    value.reset = None;
    assert_eq!(retry_delay(&value, 100_000), DEFAULT_RETRY_MS);
}

#[test]
fn unavailable_send_is_always_before_the_post_request() {
    struct UnavailableLocks;
    impl CredentialLocks for UnavailableLocks {
        fn acquire(&self, _: &CredentialEntry) -> Result<Box<dyn LockGuard>, LockError> {
            Err(LockError::Unavailable)
        }
    }
    struct UnavailableVault;
    impl CredentialStore for UnavailableVault {
        fn load(
            &self,
            _: &CredentialEntry,
        ) -> Result<Option<SecretString>, koru_oauth_credentials::Error> {
            Err(koru_oauth_credentials::Error::Unavailable)
        }
        fn store(
            &self,
            _: &CredentialEntry,
            _: SecretString,
        ) -> Result<(), koru_oauth_credentials::Error> {
            panic!("send never writes credentials")
        }
        fn clear(&self, _: &CredentialEntry) -> Result<(), koru_oauth_credentials::Error> {
            panic!("send never clears credentials")
        }
    }
    for unavailable_lock in [false, true] {
        let (mut publisher, _, events) = fixture(vec![Ok(refreshed()), Ok(identity())]);
        publisher.prepare(ACTOR).unwrap();
        if unavailable_lock {
            publisher.locks = Box::new(UnavailableLocks);
        } else {
            publisher.vault = Arc::new(UnavailableVault);
        }
        assert_eq!(
            publisher.send(ACTOR, "hello", None),
            Err(PublishError::Unavailable)
        );
        assert!(!events.lock().unwrap().contains(&"post"));
    }
}

#[test]
fn refresh_may_omit_unchanged_token_but_not_reduce_granted_scopes() {
    assert!(
        parse_refresh(br#"{"access_token":"a","token_type":"bearer","expires_in":7200}"#).is_ok()
    );
    for bytes in [
        br#"{"access_token":"a","token_type":"bearer","expires_in":0}"#.as_slice(),
        br#"{"access_token":"a","token_type":"bearer","expires_in":7200,"scope":"users.read"}"#
            .as_slice(),
        br#"{"access_token":"a","access_token":"b","token_type":"bearer","expires_in":7200}"#
            .as_slice(),
    ] {
        assert!(parse_refresh(bytes).is_err());
    }
}

#[test]
fn production_constructor_never_reads_ambient_state_under_tests() {
    assert!(matches!(
        Publisher::new(None),
        Err(PublishError::Unavailable)
    ));
}
