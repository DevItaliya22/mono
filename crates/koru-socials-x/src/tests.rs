// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_oauth_credentials::SessionCredentialStore;
use std::sync::Arc;

fn context() -> CredentialContext {
    CredentialContext::new(
        "internal-debug",
        Path::new("synthetic-root"),
        koru_device::DeviceId::parse("30000000-0000-4000-8000-000000000003").unwrap(),
    )
    .unwrap()
}
fn tokens() -> Tokens {
    parse_tokens(br#"{"access_token":"access-secret","refresh_token":"refresh-secret","token_type":"bearer","scope":"tweet.read users.read tweet.write offline.access"}"#).unwrap()
}
fn actor() -> ConnectedAccount {
    ConnectedAccount {
        id: "123456789".into(),
        username: "koru_test".into(),
        display_name: "Test account".into(),
    }
}

#[test]
fn authorization_uses_exact_callback_s256_and_no_secret() {
    assert_eq!(CALLBACK_URL, "http://localhost:48179/oauth/x/callback");
    let url = authorize_url("public_client", "random-state", "challenge").unwrap();
    assert_eq!(url.origin().ascii_serialization(), "https://x.com");
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(pairs["redirect_uri"], CALLBACK_URL);
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert_eq!(pairs["scope"], SCOPES);
    assert!(!pairs.contains_key("client_secret"));
    let form = token_form("client", "code+&=/", "verifier");
    let pairs: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(form.as_bytes()).collect();
    assert_eq!(pairs["code"], "code+&=/");
    assert_eq!(pairs["client_id"], "client");
    assert_eq!(pairs["redirect_uri"], CALLBACK_URL);
    assert_eq!(pairs["code_verifier"], "verifier");
    assert!(!pairs.contains_key("client_secret"));
}

#[test]
fn callback_refuses_denial_missing_or_duplicate_codes() {
    assert_eq!(
        callback_code([("code", "one"), ("code", "two")].into_iter()),
        Err(Error::InvalidCallback)
    );
    assert_eq!(
        callback_code([("error", "access_denied"), ("code", "one")].into_iter()),
        Err(Error::Denied)
    );
    assert_eq!(
        callback_code([("code", "")].into_iter()),
        Err(Error::InvalidCallback)
    );
    assert_eq!(
        callback_code([("state", "s")].into_iter()),
        Err(Error::InvalidCallback)
    );
}

#[test]
fn malformed_or_ambiguous_provider_responses_are_rejected() {
    for bytes in [
        br#"{"data":{"id":"1","id":"2","username":"name","name":"Name"}}"#.as_slice(),
        br#"{"data":{"id":"../actor","username":"name","name":"Name"}}"#.as_slice(),
        br#"{"data":{"id":"1","username":"name@host","name":"Name"}}"#.as_slice(),
        br#"{"data":{"id":"1","username":"name","name":""}}"#.as_slice(),
    ] {
        assert!(parse_actor(bytes).is_err());
    }
    assert_eq!(
        parse_actor(br#"{"data":{"id":"123456789","username":"koru_test","name":"Test account"}}"#)
            .unwrap(),
        actor()
    );
    assert!(parse_tokens(br#"{"access_token":"one","access_token":"two","refresh_token":"r","token_type":"bearer","scope":"tweet.read users.read tweet.write offline.access"}"#).is_err());
    assert!(matches!(parse_tokens(br#"{"access_token":"one","refresh_token":"r","token_type":"bearer","scope":"tweet.read users.read offline.access"}"#), Err(Error::MissingScopes)));
}

#[test]
fn verified_identity_precedes_vault_save_and_access_token_is_never_durable() {
    let context = context();
    let vault = SessionCredentialStore::default();
    let account = finish_connection(
        &context,
        "client",
        tokens(),
        &vault,
        &AtomicBool::new(false),
        |access| {
            assert_eq!(access.expose(), "access-secret");
            assert!(
                vault
                    .load(&actor_entry(&context, "123456789").unwrap())
                    .unwrap()
                    .is_none()
            );
            Ok(actor())
        },
    )
    .unwrap();
    assert_eq!(account, actor());
    let stored = vault
        .load(&actor_entry(&context, &account.id).unwrap())
        .unwrap()
        .unwrap();
    assert!(!stored.expose().contains("access-secret"));
    assert_eq!(
        RefreshRecord::decode(&stored).unwrap().refresh_token,
        "refresh-secret"
    );
    assert!(
        vault
            .load(&actor_entry(&context, "987654321").unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn failure_and_cancellation_never_claim_a_connected_account() {
    struct RefusingVault;
    impl CredentialStore for RefusingVault {
        fn load(
            &self,
            _: &CredentialEntry,
        ) -> Result<Option<SecretString>, koru_oauth_credentials::Error> {
            unreachable!()
        }
        fn store(
            &self,
            _: &CredentialEntry,
            _: SecretString,
        ) -> Result<(), koru_oauth_credentials::Error> {
            Err(koru_oauth_credentials::Error::Unavailable)
        }
        fn clear(&self, _: &CredentialEntry) -> Result<(), koru_oauth_credentials::Error> {
            unreachable!()
        }
    }
    assert_eq!(
        finish_connection(
            &context(),
            "client",
            tokens(),
            &RefusingVault,
            &AtomicBool::new(false),
            |_| Ok(actor())
        ),
        Err(Error::VaultUnavailable)
    );
    let vault = SessionCredentialStore::default();
    assert_eq!(
        finish_connection(
            &context(),
            "client",
            tokens(),
            &vault,
            &AtomicBool::new(false),
            |_| Err(Error::IdentityRejected)
        ),
        Err(Error::IdentityRejected)
    );
    assert_eq!(
        finish_connection(
            &context(),
            "client",
            tokens(),
            &vault,
            &AtomicBool::new(true),
            |_| panic!("cancelled before identity request")
        ),
        Err(Error::Cancelled)
    );
    assert!(
        vault
            .load(&actor_entry(&context(), "123456789").unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn cancellation_during_vault_save_preserves_the_credential_without_claiming_connection() {
    struct CancellingVault<'a> {
        inner: SessionCredentialStore,
        cancelled: &'a AtomicBool,
    }
    impl CredentialStore for CancellingVault<'_> {
        fn load(
            &self,
            entry: &CredentialEntry,
        ) -> Result<Option<SecretString>, koru_oauth_credentials::Error> {
            self.inner.load(entry)
        }
        fn store(
            &self,
            entry: &CredentialEntry,
            secret: SecretString,
        ) -> Result<(), koru_oauth_credentials::Error> {
            self.inner.store(entry, secret)?;
            self.cancelled.store(true, Ordering::Relaxed);
            Ok(())
        }
        fn clear(&self, _: &CredentialEntry) -> Result<(), koru_oauth_credentials::Error> {
            panic!("cancellation must not delete a possibly rotated credential")
        }
    }
    let cancelled = AtomicBool::new(false);
    let vault = CancellingVault {
        inner: SessionCredentialStore::default(),
        cancelled: &cancelled,
    };
    let context = context();
    let result = finish_connection(&context, "client", tokens(), &vault, &cancelled, |_| {
        Ok(actor())
    });
    assert_eq!(result, Err(Error::Cancelled));
    let stored = vault
        .load(&actor_entry(&context, "123456789").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        RefreshRecord::decode(&stored).unwrap().refresh_token,
        "refresh-secret"
    );
}

#[test]
fn exported_production_entries_cannot_touch_an_ambient_agent_or_vault_in_tests() {
    assert_eq!(has_credentials(None, "123456789"), Err(Error::Disabled));
    assert_eq!(disconnect(None, "123456789"), Err(Error::Disabled));
    assert_eq!(
        connect(ConnectOptions {
            root: None,
            client_id: "client".into(),
            cancelled: Arc::new(AtomicBool::new(false)),
            open_browser: |_| panic!("must not open a browser"),
            success_html: String::new(),
            error_html: String::new()
        }),
        Err(Error::Disabled)
    );
}
