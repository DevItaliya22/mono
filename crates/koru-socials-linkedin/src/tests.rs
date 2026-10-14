// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_oauth_credentials::SessionCredentialStore;
use koru_socials_auth::credential_lock::LockGuard;
use std::{collections::VecDeque, sync::Mutex};
const PERSON: &str = "urn:li:person:abc_123";
const PAGE: &str = "urn:li:organization:1234";
fn context() -> CredentialContext {
    CredentialContext::new(
        "internal-debug",
        Path::new("synthetic-root"),
        koru_device::DeviceId::parse("30000000-0000-4000-8000-000000000003").unwrap(),
    )
    .unwrap()
}
fn response(status: u16, body: &str) -> Response {
    Response {
        status,
        body: Zeroizing::new(body.as_bytes().to_vec()),
        retry_after: None,
        id: None,
    }
}
fn identity() -> Response {
    response(200, r#"{"sub":"abc_123","name":"Test Person"}"#)
}
fn receipt() -> Response {
    let mut r = response(201, "");
    r.id = Some("urn:li:share:123456".into());
    r
}
fn role(state: &str, role: &str, member: &str, actor: &str) -> serde_json::Value {
    serde_json::json!({"state":state,"role":role,"roleAssignee":member,"organization":actor})
}
fn roles(state: &str) -> Response {
    response(
        200,
        &serde_json::json!({"elements":[role(state,"ADMINISTRATOR",PERSON,PAGE)]}).to_string(),
    )
}
struct FakeTransport {
    responses: VecDeque<Result<Response, ()>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    locked: Option<Arc<AtomicBool>>,
}
impl Transport for FakeTransport {
    fn request(&mut self, request: Request<'_>) -> Result<Response, ()> {
        if let Some(locked) = &self.locked {
            assert!(locked.load(Ordering::Relaxed));
        }
        let label = match request {
            Request::Identity { .. } => "identity",
            Request::Roles { .. } => "roles",
            Request::Post { .. } => "post",
        };
        self.events.lock().unwrap().push(label);
        self.responses.pop_front().expect("unexpected request")
    }
}
struct Guard(Arc<AtomicBool>);
impl LockGuard for Guard {}
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}
struct Locks(Arc<AtomicBool>);
impl CredentialLocks for Locks {
    fn acquire(&self, _: &CredentialEntry) -> Result<Box<dyn LockGuard>, LockError> {
        assert!(!self.0.swap(true, Ordering::Relaxed));
        Ok(Box::new(Guard(self.0.clone())))
    }
}
fn record(actor: &str, expiry: i64) -> SecretString {
    SecretString::new(
        serde_json::to_string(&Record {
            version: 1,
            actor: actor.into(),
            member: PERSON.into(),
            access_token: "test-access".into(),
            expires_ms: expiry,
        })
        .unwrap(),
    )
    .unwrap()
}
type Fixture = (
    Publisher,
    Arc<SessionCredentialStore>,
    Arc<Mutex<Vec<&'static str>>>,
);
fn fixture(actor: &str, responses: Vec<Result<Response, ()>>) -> Fixture {
    let context = context();
    let vault = Arc::new(SessionCredentialStore::default());
    vault
        .store(
            &actor_entry(&context, actor).unwrap(),
            record(actor, now_ms() + 3_600_000),
        )
        .unwrap();
    let locked = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    (
        Publisher {
            context,
            transport: Box::new(FakeTransport {
                responses: responses.into(),
                events: events.clone(),
                locked: Some(locked.clone()),
            }),
            vault: vault.clone(),
            locks: Box::new(Locks(locked)),
            prepared: None,
        },
        vault,
        events,
    )
}
#[test]
fn native_pkce_has_no_client_secret_and_matches_callback() {
    let url = authorize_url("client", "state", "challenge", false).unwrap();
    assert_eq!(url.path(), "/oauth/native-pkce/authorization");
    let fields: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(fields["redirect_uri"], CALLBACK_URL);
    assert_eq!(fields["code_challenge_method"], "S256");
    assert_eq!(fields["scope"], PERSON_SCOPES);
    let form = token_form("client", "a+b/c", "v");
    let pairs: std::collections::HashMap<_, _> = url::form_urlencoded::parse(form.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(pairs["code"], "a+b/c");
    assert_eq!(pairs["redirect_uri"], CALLBACK_URL);
    assert!(!pairs.contains_key("client_secret"));
    assert!(!PAGE_SCOPES.contains("rw_organization_admin"));
}
#[test]
fn native_token_requires_finite_lifetime_and_rejects_reduced_scopes() {
    assert!(parse_token(br#"{"access_token":"a","expires_in":7200}"#, 1000, false).is_ok());
    for body in [
        r#"{"access_token":"a","expires_in":60}"#,
        r#"{"access_token":"a","expires_in":18446744073709551615}"#,
        r#"{"access_token":"a","expires_in":7200,"scope":"openid profile"}"#,
    ] {
        assert!(parse_token(body.as_bytes(), 1000, false).is_err());
    }
}
#[test]
fn plaintext_escapes_little_grammar_without_changing_unicode() {
    let payload = transport::post_payload(PERSON, "Hello @[Name](urn) #tag *文字* \\_|{}<>~\nأهلا");
    assert_eq!(payload["author"], PERSON);
    assert_eq!(
        payload["commentary"],
        "Hello \\@\\[Name\\]\\(urn\\) \\#tag \\*文字\\* \\\\\\_\\|\\{\\}\\<\\>\\~\nأهلا"
    );
    assert_eq!(payload["visibility"], "PUBLIC");
    assert_eq!(payload["distribution"]["feedDistribution"], "MAIN_FEED");
}
#[test]
fn page_requires_approved_organic_role_of_exact_member_and_organization() {
    assert!(approved_role(
        &role("APPROVED", "ADMINISTRATOR", PERSON, PAGE),
        PERSON,
        PAGE
    ));
    assert!(approved_role(
        &role("APPROVED", "CONTENT_ADMINISTRATOR", PERSON, PAGE),
        PERSON,
        PAGE
    ));
    for state in ["REQUESTED", "REVOKED", "REJECTED"] {
        assert!(!approved_role(
            &role(state, "ADMINISTRATOR", PERSON, PAGE),
            PERSON,
            PAGE
        ));
    }
    for role_name in [
        "ANALYST",
        "DIRECT_SPONSORED_CONTENT_POSTER",
        "RECRUITING_POSTER",
    ] {
        assert!(!approved_role(
            &role("APPROVED", role_name, PERSON, PAGE),
            PERSON,
            PAGE
        ));
    }
    assert!(!approved_role(
        &role("APPROVED", "ADMINISTRATOR", "urn:li:person:other", PAGE),
        PERSON,
        PAGE
    ));
    assert!(!approved_role(
        &role("APPROVED", "ADMINISTRATOR", PERSON, "urn:li:organization:2"),
        PERSON,
        PAGE
    ));
}
#[test]
fn member_publication_checks_identity_and_consumes_grant_once() {
    let (mut p, _, events) = fixture(PERSON, vec![Ok(identity()), Ok(receipt())]);
    p.prepare(PERSON).unwrap();
    let posted = p.send(PERSON, "Hello").unwrap();
    assert_eq!(
        posted.url,
        "https://www.linkedin.com/feed/update/urn:li:share:123456/"
    );
    assert_eq!(p.send(PERSON, "Hello"), Err(PublishError::NeedsConnection));
    assert_eq!(*events.lock().unwrap(), vec!["identity", "post"]);
}
#[test]
fn identity_mismatch_never_arms_publication() {
    let (mut p, _, events) = fixture(
        PERSON,
        vec![Ok(response(200, r#"{"sub":"different","name":"Other"}"#))],
    );
    assert_eq!(p.prepare(PERSON), Err(PublishError::NeedsConnection));
    assert_eq!(p.send(PERSON, "Hello"), Err(PublishError::NeedsConnection));
    assert_eq!(*events.lock().unwrap(), vec!["identity"]);
}
#[test]
fn page_roles_are_checked_before_each_part() {
    let (mut p, _, events) = fixture(
        PAGE,
        vec![
            Ok(identity()),
            Ok(roles("APPROVED")),
            Ok(receipt()),
            Ok(identity()),
            Ok(roles("REVOKED")),
        ],
    );
    p.prepare(PAGE).unwrap();
    p.send(PAGE, "one").unwrap();
    assert_eq!(p.prepare(PAGE), Err(PublishError::NeedsConnection));
    assert_eq!(p.send(PAGE, "two"), Err(PublishError::NeedsConnection));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["identity", "roles", "post", "identity", "roles"]
    );
}
#[test]
fn expired_token_never_reaches_network() {
    let (mut p, vault, events) = fixture(PERSON, vec![]);
    vault
        .store(
            &actor_entry(&p.context, PERSON).unwrap(),
            record(PERSON, now_ms() - 1),
        )
        .unwrap();
    assert_eq!(p.prepare(PERSON), Err(PublishError::NeedsConnection));
    assert!(events.lock().unwrap().is_empty());
}
#[test]
fn disconnect_and_reconnect_invalidate_prepared_grant() {
    for reconnect in [false, true] {
        let (mut p, vault, events) = fixture(PERSON, vec![Ok(identity())]);
        p.prepare(PERSON).unwrap();
        let entry = actor_entry(&p.context, PERSON).unwrap();
        if reconnect {
            vault
                .store(&entry, record(PERSON, now_ms() + 7_200_000))
                .unwrap();
        } else {
            vault.clear(&entry).unwrap();
        }
        assert_eq!(p.send(PERSON, "Hi"), Err(PublishError::NeedsConnection));
        assert_eq!(*events.lock().unwrap(), vec!["identity"]);
    }
}
#[test]
fn expired_prepared_permission_never_sends() {
    let (mut p, _, events) = fixture(PERSON, vec![Ok(identity())]);
    p.prepare(PERSON).unwrap();
    p.prepared.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    assert_eq!(p.send(PERSON, "Hi"), Err(PublishError::NeedsConnection));
    assert_eq!(*events.lock().unwrap(), vec!["identity"]);
}
#[test]
fn post_failures_preserve_uncertainty_and_do_not_retry_implicitly() {
    for (status, expected) in [
        (401, PublishError::NeedsConnection),
        (403, PublishError::Rejected),
        (429, retry()),
        (500, PublishError::Unconfirmed),
        (408, PublishError::Unconfirmed),
        (200, PublishError::Unconfirmed),
        (201, PublishError::Unconfirmed),
    ] {
        let (mut p, _, events) = fixture(PERSON, vec![Ok(identity()), Ok(response(status, ""))]);
        p.prepare(PERSON).unwrap();
        assert_eq!(p.send(PERSON, "Hi"), Err(expected));
        assert_eq!(p.send(PERSON, "Hi"), Err(PublishError::NeedsConnection));
        assert_eq!(*events.lock().unwrap(), vec!["identity", "post"]);
    }
    let (mut p, _, _) = fixture(PERSON, vec![Ok(identity()), Err(())]);
    p.prepare(PERSON).unwrap();
    assert_eq!(p.send(PERSON, "Hi"), Err(PublishError::Unconfirmed));
}
#[test]
fn rate_limit_is_not_truncated_and_read_failures_are_safe() {
    assert_eq!(
        transport::retry_delay(Some("99999999999999999999999999999")),
        u64::MAX
    );
    let mut r = response(429, "");
    r.retry_after = Some("7200".into());
    assert_eq!(
        classify(&r, true),
        Err(PublishError::Retryable {
            retry_after_ms: 7_200_000
        })
    );
    assert_eq!(classify(&response(503, ""), false), Err(retry()));
    for id in [
        None,
        Some(""),
        Some("https://evil.test/x"),
        Some("urn:li:share:123?evil"),
        Some("urn:li:organization:123"),
    ] {
        assert_eq!(parse_posted(id), Err(PublishError::Unconfirmed));
    }
}
#[test]
fn cancelled_or_unapproved_connection_never_stores_credential() {
    let context = context();
    let vault = SessionCredentialStore::default();
    let cancelled = AtomicBool::new(true);
    let mut transport = FakeTransport {
        responses: VecDeque::new(),
        events: Default::default(),
        locked: None,
    };
    assert_eq!(
        finish_connection(
            &context,
            (SecretString::new("a").unwrap(), now_ms() + 3_600_000),
            None,
            &vault,
            &cancelled,
            &mut transport
        ),
        Err(Error::Cancelled)
    );
    cancelled.store(false, Ordering::Relaxed);
    transport.responses = vec![Ok(identity()), Ok(roles("REQUESTED"))].into();
    assert_eq!(
        finish_connection(
            &context,
            (SecretString::new("a").unwrap(), now_ms() + 3_600_000),
            Some("1234"),
            &vault,
            &cancelled,
            &mut transport
        ),
        Err(Error::IdentityRejected)
    );
    assert!(
        vault
            .load(&actor_entry(&context, PAGE).unwrap())
            .unwrap()
            .is_none()
    );
}
#[test]
fn verified_page_connection_stores_member_separately_from_page() {
    let context = context();
    let vault = SessionCredentialStore::default();
    let mut transport = FakeTransport {
        responses: vec![Ok(identity()), Ok(roles("APPROVED"))].into(),
        events: Default::default(),
        locked: None,
    };
    let account = finish_connection(
        &context,
        (SecretString::new("a").unwrap(), now_ms() + 3_600_000),
        Some("1234"),
        &vault,
        &AtomicBool::new(false),
        &mut transport,
    )
    .unwrap();
    assert_eq!(account.id, PAGE);
    assert_eq!(account.display_name, "1234");
    let secret = vault
        .load(&actor_entry(&context, PAGE).unwrap())
        .unwrap()
        .unwrap();
    let r = Record::decode(&secret).unwrap();
    assert_eq!(r.member, PERSON);
    assert_eq!(r.actor, PAGE);
}

#[test]
fn role_pagination_is_bounded_and_does_not_follow_remote_urls() {
    struct Pages {
        requested: Vec<usize>,
        found_on: Option<usize>,
    }
    impl Transport for Pages {
        fn request(&mut self, request: Request<'_>) -> Result<Response, ()> {
            let Request::Roles { start, .. } = request else {
                panic!("only ACL reads allowed");
            };
            self.requested.push(start);
            let actor = if self.found_on == Some(start) {
                PAGE
            } else {
                "urn:li:organization:999"
            };
            Ok(response(
                200,
                &serde_json::json!({
                    "elements": [role("APPROVED", "ADMINISTRATOR", PERSON, actor)],
                    "paging": {"links": [{"rel": "next", "href": "https://untrusted.test/collect"}]}
                })
                .to_string(),
            ))
        }
    }
    let mut pages = Pages {
        requested: vec![],
        found_on: Some(2),
    };
    verify_page(&mut pages, "access", PERSON, PAGE, None).unwrap();
    assert_eq!(pages.requested, vec![0, 1, 2]);
    let mut pages = Pages {
        requested: vec![],
        found_on: None,
    };
    assert_eq!(
        verify_page(&mut pages, "access", PERSON, PAGE, None),
        Err(PublishError::NeedsConnection)
    );
    assert_eq!(pages.requested, vec![0, 1, 2, 3, 4]);
    let cancelled = AtomicBool::new(true);
    pages.requested.clear();
    assert_eq!(
        verify_page(&mut pages, "access", PERSON, PAGE, Some(&cancelled)),
        Err(PublishError::Unavailable)
    );
    assert!(pages.requested.is_empty());
}

#[test]
fn invalid_actor_and_changed_actor_never_publish() {
    for actor in [
        "urn:li:organization:0",
        "urn:li:organization:123/other",
        "urn:li:person:",
        "https://linkedin.com/user",
    ] {
        assert!(actor_entry(&context(), actor).is_err());
    }
    let (mut p, _, events) = fixture(PERSON, vec![Ok(identity())]);
    p.prepare(PERSON).unwrap();
    assert_eq!(
        p.send(PAGE, "wrong author"),
        Err(PublishError::NeedsConnection)
    );
    assert_eq!(*events.lock().unwrap(), vec!["identity"]);
}
