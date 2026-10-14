// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_oauth_credentials::SessionCredentialStore;
use koru_socials_auth::credential_lock::LockGuard;
use std::{collections::VecDeque, sync::Mutex};

struct InertGuard;
impl LockGuard for InertGuard {}
struct InertLocks;
impl CredentialLocks for InertLocks {
    fn acquire(&self, _: &CredentialEntry) -> Result<Box<dyn LockGuard>, LockError> {
        Ok(Box::new(InertGuard))
    }
}
struct FakeTransport {
    results: VecDeque<Result<Response, ()>>,
    calls: Arc<Mutex<Vec<String>>>,
}
impl Transport for FakeTransport {
    fn request(&mut self, request: Request<'_>) -> Result<Response, ()> {
        let call = match request {
            Request::Refresh { client_id, refresh } => {
                assert_eq!(client_id, "client");
                assert_eq!(refresh, "original");
                "refresh".to_string()
            }
            Request::Identity { access } => {
                assert_eq!(access, "access-private");
                "identity".into()
            }
            Request::Submit {
                text,
                subreddit,
                title,
                flair_id,
                ..
            } => submit_form(text, subreddit, title, flair_id).to_string(),
        };
        self.calls.lock().unwrap().push(call);
        self.results.pop_front().expect("unexpected request")
    }
}
fn response(status: u16, body: &[u8]) -> Response {
    Response {
        status,
        body: Zeroizing::new(body.to_vec()),
        retry_after: None,
        reset: None,
    }
}
fn identity(id: &str) -> Response {
    response(
        200,
        &serde_json::to_vec(&serde_json::json!({"id":id,"name":"Test_User"})).unwrap(),
    )
}
fn token() -> Response {
    response(200, &crate::tests::token_json(Some("rotated")))
}
fn success() -> Response {
    response(200, br#"{"json":{"errors":[],"data":{"id":"abc123","name":"t3_abc123","url":"https://untrusted.invalid/"}}}"#)
}
type Fixture = (
    Publisher,
    Arc<SessionCredentialStore>,
    Arc<Mutex<Vec<String>>>,
);
fn publisher(results: Vec<Result<Response, ()>>) -> Fixture {
    let context = crate::tests::context();
    let vault = Arc::new(SessionCredentialStore::default());
    vault
        .store(
            &actor_entry(&context, "a1b2").unwrap(),
            RefreshRecord {
                version: 1,
                client_id: "client".into(),
                refresh_token: "original".into(),
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let publisher = Publisher {
        context,
        vault: vault.clone(),
        prepared: None,
        locks: Box::new(InertLocks),
        transport: Box::new(FakeTransport {
            results: results.into(),
            calls: calls.clone(),
        }),
    };
    (publisher, vault, calls)
}

#[test]
fn successful_submission_encodes_destination_metadata_and_never_uses_remote_url() {
    let (mut publisher, _, calls) =
        publisher(vec![Ok(token()), Ok(identity("a1b2")), Ok(success())]);
    publisher.prepare("a1b2").unwrap();
    let posted = publisher
        .send("a1b2", "body & +\n雪", "test_sub", "A & B", Some("abc-def"))
        .unwrap();
    assert_eq!(posted.id, "t3_abc123");
    assert_eq!(posted.url, "https://www.reddit.com/comments/abc123/");
    let calls = calls.lock().unwrap();
    let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(calls[2].as_bytes())
        .into_owned()
        .collect();
    for (key, value) in [
        ("sr", "test_sub"),
        ("title", "A & B"),
        ("text", "body & +\n雪"),
        ("flair_id", "abc-def"),
        ("kind", "self"),
        ("api_type", "json"),
        ("raw_json", "1"),
    ] {
        assert_eq!(form[key], value);
    }
    drop(calls);
    assert_eq!(
        publisher.send("a1b2", "body", "test_sub", "title", None),
        Err(PublishError::NeedsConnection)
    );
}

#[test]
fn rotation_survives_identity_failure_and_wrong_identity_never_arms_send() {
    for identity_result in [Err(()), Ok(identity("other"))] {
        let (mut publisher, vault, calls) = publisher(vec![Ok(token()), identity_result]);
        assert!(publisher.prepare("a1b2").is_err());
        let entry = actor_entry(&publisher.context, "a1b2").unwrap();
        assert_eq!(
            RefreshRecord::decode(&vault.load(&entry).unwrap().unwrap())
                .unwrap()
                .refresh_token,
            "rotated"
        );
        assert_eq!(
            publisher.send("a1b2", "body", "test_sub", "title", None),
            Err(PublishError::NeedsConnection)
        );
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}

#[test]
fn disconnect_reconnect_and_expiry_between_prepare_and_send_block_submission() {
    for action in 0..3 {
        let (mut publisher, vault, calls) = publisher(vec![Ok(token()), Ok(identity("a1b2"))]);
        publisher.prepare("a1b2").unwrap();
        let entry = actor_entry(&publisher.context, "a1b2").unwrap();
        match action {
            0 => vault.clear(&entry).unwrap(),
            1 => vault
                .store(
                    &entry,
                    RefreshRecord {
                        version: 1,
                        client_id: "client".into(),
                        refresh_token: "reconnected".into(),
                    }
                    .encode()
                    .unwrap(),
                )
                .unwrap(),
            _ => {
                publisher.prepared.as_mut().unwrap().expires =
                    Instant::now() - Duration::from_secs(1)
            }
        }
        assert_eq!(
            publisher.send("a1b2", "body", "test_sub", "title", None),
            Err(PublishError::NeedsConnection)
        );
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}

#[test]
fn uncertain_transport_consumes_one_preparation_and_cannot_resend() {
    let (mut publisher, _, calls) = publisher(vec![Ok(token()), Ok(identity("a1b2")), Err(())]);
    publisher.prepare("a1b2").unwrap();
    assert_eq!(
        publisher.send("a1b2", "body", "test_sub", "title", None),
        Err(PublishError::Unconfirmed)
    );
    assert_eq!(
        publisher.send("a1b2", "body", "test_sub", "title", None),
        Err(PublishError::NeedsConnection)
    );
    assert_eq!(calls.lock().unwrap().len(), 3);
}

#[test]
fn http_statuses_distinguish_safe_refusal_from_possible_submission() {
    for (status, expected) in [
        (401, PublishError::NeedsConnection),
        (403, PublishError::Rejected),
        (400, PublishError::Rejected),
        (408, PublishError::Unconfirmed),
        (500, PublishError::Unconfirmed),
        (302, PublishError::Unconfirmed),
        (
            429,
            PublishError::Retryable {
                retry_after_ms: 60_000,
            },
        ),
    ] {
        assert_eq!(classify_submit(&response(status, b"")), Err(expected));
    }
    assert_eq!(classify_prepare(&response(500, b""), false), Err(retry()));
    assert_eq!(
        classify_prepare(&response(400, b""), true),
        Err(PublishError::NeedsConnection)
    );
}

#[test]
fn http_200_requires_clear_confirmation_and_treats_json_errors_as_failure() {
    assert_eq!(
        parse_submitted(br#"{"json":{"errors":[["SUBREDDIT_NOTALLOWED","not allowed","sr"]]}}"#),
        Err(PublishError::Rejected)
    );
    assert_eq!(
        parse_submitted(br#"{"json":{"errors":[["RATELIMIT","wait","ratelimit"]]}}"#),
        Err(PublishError::Rejected)
    );
    for body in [
        b"not json".as_slice(),
        br#"{"json":{"errors":[]}}"#,
        br#"{"json":{"errors":[],"data":{"name":"t1_abc"}}}"#,
        br#"{"json":{"errors":[],"data":{"name":"t3_abc","id":"def"}}}"#,
        br#"{"json":{"errors":[[]]}}"#,
        br#"{"json":{"errors":[["FAIL","failed"]],"data":{"id":"abc"}}}"#,
        br#"{"json":{"errors":[],"data":{"id":"../evil"}}}"#,
    ] {
        assert_eq!(parse_submitted(body), Err(PublishError::Unconfirmed));
    }
}

#[test]
fn rate_limits_preserve_long_delays_and_relative_reddit_reset() {
    let mut limited = response(429, b"");
    limited.retry_after = Some("999999999999999999999999999999999".into());
    assert_eq!(retry_delay(&limited, 100_000), u64::MAX);
    limited.retry_after = Some("10".into());
    limited.reset = Some("121.2".into());
    assert_eq!(retry_delay(&limited, 100_000), 122_000);
    limited.retry_after = Some("Thu, 01 Jan 1970 00:05:00 GMT".into());
    assert_eq!(retry_delay(&limited, 100_000), 200_000);
}

#[test]
fn invalid_metadata_never_reaches_submit() {
    for (subreddit, title, flair) in [
        ("r/test", "title", None),
        ("test", "", None),
        ("test", "title\n", None),
        ("test", "title", Some("../bad")),
    ] {
        let (mut publisher, _, calls) = publisher(vec![Ok(token()), Ok(identity("a1b2"))]);
        publisher.prepare("a1b2").unwrap();
        assert_eq!(
            publisher.send("a1b2", "body", subreddit, title, flair),
            Err(PublishError::Rejected)
        );
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}

struct FailingSave(Arc<SessionCredentialStore>);
impl CredentialStore for FailingSave {
    fn load(
        &self,
        entry: &CredentialEntry,
    ) -> Result<Option<SecretString>, koru_oauth_credentials::Error> {
        self.0.load(entry)
    }
    fn store(
        &self,
        _: &CredentialEntry,
        _: SecretString,
    ) -> Result<(), koru_oauth_credentials::Error> {
        Err(koru_oauth_credentials::Error::Unavailable)
    }
    fn clear(&self, entry: &CredentialEntry) -> Result<(), koru_oauth_credentials::Error> {
        self.0.clear(entry)
    }
}

#[test]
fn failed_rotation_storage_and_missing_credentials_prevent_later_requests() {
    let (mut publisher, vault, calls) = publisher(vec![Ok(token())]);
    publisher.vault = Arc::new(FailingSave(vault.clone()));
    assert_eq!(publisher.prepare("a1b2"), Err(PublishError::Unavailable));
    assert_eq!(calls.lock().unwrap().as_slice(), &["refresh"]);
    assert_eq!(
        publisher.send("a1b2", "body", "test_sub", "title", None),
        Err(PublishError::NeedsConnection)
    );
    vault
        .clear(&actor_entry(&publisher.context, "a1b2").unwrap())
        .unwrap();
    assert_eq!(
        publisher.prepare("a1b2"),
        Err(PublishError::NeedsConnection)
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
}

struct BusyLocks;
impl CredentialLocks for BusyLocks {
    fn acquire(&self, _: &CredentialEntry) -> Result<Box<dyn LockGuard>, LockError> {
        Err(LockError::Busy)
    }
}

#[test]
fn busy_credential_lock_prevents_refresh_and_submit() {
    let (mut publisher, _, calls) = publisher(vec![Ok(token()), Ok(identity("a1b2"))]);
    publisher.locks = Box::new(BusyLocks);
    assert_eq!(
        publisher.prepare("a1b2"),
        Err(PublishError::Retryable {
            retry_after_ms: 1000
        })
    );
    assert!(calls.lock().unwrap().is_empty());
    publisher.locks = Box::new(InertLocks);
    publisher.prepare("a1b2").unwrap();
    publisher.locks = Box::new(BusyLocks);
    assert_eq!(
        publisher.send("a1b2", "body", "test_sub", "title", None),
        Err(PublishError::Retryable {
            retry_after_ms: 1000
        })
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
}
