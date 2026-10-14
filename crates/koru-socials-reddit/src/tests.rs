// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_oauth_credentials::SessionCredentialStore;

pub(super) fn context() -> CredentialContext {
    CredentialContext::new(
        "dev",
        Path::new("D:/koru/inert-reddit-test"),
        koru_device::DeviceId::parse("a6258102-2534-4fab-bd4d-40717b066f29").unwrap(),
    )
    .unwrap()
}
pub(super) fn token_json(refresh: Option<&str>) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({ "access_token": "access-private", "refresh_token": refresh,
        "token_type": "bearer", "expires_in": 3600, "scope": "identity submit" }),
    )
    .unwrap()
}

#[test]
fn oauth_is_permanent_and_encodes_code_without_a_client_secret() {
    let url = authorize_url("public-client", "state&=?").unwrap();
    let fields: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(fields["duration"], "permanent");
    assert_eq!(fields["redirect_uri"], CALLBACK_URL);
    assert_eq!(fields["scope"], "identity submit");
    assert_eq!(fields["state"], "state&=?");
    let form = token_form("code+&=");
    let fields: std::collections::HashMap<_, _> = url::form_urlencoded::parse(form.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(fields["code"], "code+&=");
    assert!(!fields.contains_key("client_secret"));
    assert_eq!(
        callback_code([("code", "one"), ("code", "two")].into_iter()),
        Err(Error::InvalidCallback)
    );
    assert_eq!(
        callback_code([("error", "access_denied")].into_iter()),
        Err(Error::Denied)
    );
}

#[test]
fn tokens_require_identity_submit_live_bearer_and_initial_refresh() {
    assert!(parse_tokens(&token_json(Some("refresh-private")), true).is_ok());
    assert!(parse_tokens(&token_json(None), false).is_ok());
    assert_eq!(
        parse_tokens(&token_json(None), true).err(),
        Some(Error::InvalidResponse)
    );
    for (field, value, expected) in [
        ("scope", serde_json::json!("identity"), Error::MissingScopes),
        ("scope", serde_json::json!("submit"), Error::MissingScopes),
        ("expires_in", serde_json::json!(0), Error::InvalidResponse),
        ("expires_in", serde_json::json!(30), Error::InvalidResponse),
        (
            "expires_in",
            serde_json::json!(u64::MAX),
            Error::InvalidResponse,
        ),
        (
            "token_type",
            serde_json::json!("basic"),
            Error::InvalidResponse,
        ),
        (
            "access_token",
            serde_json::json!(""),
            Error::InvalidResponse,
        ),
        (
            "refresh_token",
            serde_json::json!(""),
            Error::InvalidResponse,
        ),
    ] {
        let mut value_json: serde_json::Value =
            serde_json::from_slice(&token_json(Some("refresh"))).unwrap();
        value_json[field] = value;
        assert_eq!(
            parse_tokens(&serde_json::to_vec(&value_json).unwrap(), true).err(),
            Some(expected),
            "{field}"
        );
    }
}

#[test]
fn identity_uses_stable_base36_id_and_stores_only_refresh() {
    let vault = SessionCredentialStore::default();
    let ctx = context();
    let actor = finish_connection(
        &ctx,
        "client",
        parse_tokens(&token_json(Some("refresh-private")), true).unwrap(),
        &vault,
        &AtomicBool::new(false),
        |access| {
            assert_eq!(access.expose(), "access-private");
            parse_actor(br#"{"id":"a1b2","name":"Example_User"}"#)
        },
    )
    .unwrap();
    assert_eq!(actor.id, "a1b2");
    assert_eq!(actor.username, "Example_User");
    let stored = vault
        .load(&actor_entry(&ctx, "a1b2").unwrap())
        .unwrap()
        .unwrap();
    assert!(!stored.expose().contains("access-private"));
    assert_eq!(
        RefreshRecord::decode(&stored).unwrap().refresh_token,
        "refresh-private"
    );
    assert_ne!(
        actor_entry(&ctx, "a1b2").unwrap(),
        actor_entry(&ctx, "b1c2").unwrap()
    );
    assert!(parse_actor(br#"{"id":"t2_a1b2","name":"Example_User"}"#).is_err());
    assert!(parse_actor(br#"{"id":"a1b2","name":"bad/user"}"#).is_err());
}

#[test]
fn cancellation_and_identity_failure_do_not_store_a_grant() {
    let vault = SessionCredentialStore::default();
    let ctx = context();
    let cancelled = AtomicBool::new(false);
    let result = finish_connection(
        &ctx,
        "client",
        parse_tokens(&token_json(Some("refresh")), true).unwrap(),
        &vault,
        &cancelled,
        |_| {
            cancelled.store(true, Ordering::Relaxed);
            parse_actor(br#"{"id":"a1b2","name":"Example_User"}"#)
        },
    );
    assert_eq!(result, Err(Error::Cancelled));
    assert!(
        vault
            .load(&actor_entry(&ctx, "a1b2").unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn production_entry_points_are_inert_under_tests() {
    assert_eq!(has_credentials(None, "a1b2"), Err(Error::Disabled));
    assert_eq!(disconnect(None, "a1b2"), Err(Error::Disabled));
    assert!(matches!(
        Publisher::new(None),
        Err(PublishError::Unavailable)
    ));
    assert_eq!(
        connect(ConnectOptions {
            root: None,
            client_id: "client".into(),
            cancelled: std::sync::Arc::new(AtomicBool::new(false)),
            open_browser: |_| panic!("must not open browser"),
            success_html: String::new(),
            error_html: String::new(),
        }),
        Err(Error::Disabled)
    );
}
