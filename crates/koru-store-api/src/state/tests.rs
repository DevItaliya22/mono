// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::ids::ClientId;
use koru_store_core::model::{ApiClient, Timestamp};
use koru_store_sqlite::SqliteBackend;

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

fn state(dir: &std::path::Path) -> AppState {
    let catalog = SqliteCatalog::open(&dir.join("catalog.db")).unwrap();
    let store = Store::open(catalog, SqliteBackend::new(dir)).unwrap();
    AppState::new(
        store,
        "<root>".into(),
        "internal-debug".into(),
        at().millis(),
    )
}

/// Register a client and return the token it authenticates with.
fn register(state: &AppState, name: &str, scopes: Vec<Scope>) -> (ClientId, String) {
    let token = format!("token-for-{name}");
    let client = ApiClient {
        id: ClientId::new(),
        name: name.into(),
        scopes,
        created_at: at(),
        revoked_at: None,
    };
    state
        .store
        .lock()
        .unwrap()
        .register_client(&client, &token_hash(&token))
        .unwrap();
    (client.id, token)
}

#[test]
fn a_minted_session_resolves_to_what_it_was_given() {
    // What replaced the bootstrap token. The difference that matters is not
    // visible here — it is that this credential exists only in memory and dies
    // with the process, where `store.secret` was a file that outlived it.
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let (token, _) = state.mint_session("koru", Scope::ALL.to_vec());
    let caller = state.resolve(&token).expect("a session resolves");
    for s in Scope::ALL {
        assert!(caller.allows(*s), "{s} was not granted");
    }
    assert_eq!(caller.label, "koru");
}

#[test]
fn a_session_can_be_minted_with_less_than_everything() {
    // The boundary the brief asks for: a third-party client must not
    // automatically receive unrestricted access. The mechanism is here; what is
    // missing is the policy that decides who gets what — see TECHDEBT.
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let (token, _) = state.mint_session("a-reader", vec![Scope::ReadDocuments]);
    let caller = state.resolve(&token).expect("a session resolves");
    assert!(caller.allows(Scope::ReadDocuments));
    assert!(!caller.allows(Scope::ManageWorkspaces));
}

#[test]
fn an_unknown_token_resolves_to_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    assert!(state.resolve("not the token").is_none());
    assert!(state.resolve("").is_none());
}

#[test]
fn a_token_one_character_off_is_not_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let (token, _) = state.mint_session("koru", Scope::ALL.to_vec());
    let mut near = token.clone();
    // The last character, so an implementation that compared a prefix would
    // pass every other assertion in this file and fail only here.
    near.replace_range(63..64, if token.ends_with('0') { "1" } else { "0" });
    assert!(state.resolve(&near).is_none());
}

#[test]
fn a_registered_client_resolves_to_exactly_its_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let (_, token) = register(&state, "reader", vec![Scope::ReadDocuments]);

    let caller = state.resolve(&token).expect("registered token resolves");
    assert_eq!(caller.label, "reader");
    assert!(caller.allows(Scope::ReadDocuments));
    assert!(!caller.allows(Scope::EditDocuments));
}

#[test]
fn a_revoked_client_resolves_to_nothing() {
    // Deliberately the same answer as an unknown token: a difference between
    // them is an oracle for guessing valid tokens.
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let (id, token) = register(&state, "temporary", Scope::ALL.to_vec());
    assert!(state.resolve(&token).is_some());

    state.store.lock().unwrap().revoke_client(id, at()).unwrap();
    assert!(state.resolve(&token).is_none());
}

#[test]
fn the_token_is_never_stored_only_its_hash() {
    // A leaked `catalog.db` must not be a set of working keys.
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let (_, token) = register(&state, "client", vec![Scope::ReadTasks]);

    let store = state.store.lock().unwrap();
    assert!(
        store.client_by_token(token.as_bytes()).unwrap().is_none(),
        "the token itself was written down"
    );
    assert!(
        store
            .client_by_token(&token_hash(&token))
            .unwrap()
            .is_some()
    );
}

#[test]
fn hashing_is_deterministic_and_distinguishes_tokens() {
    assert_eq!(token_hash("a"), token_hash("a"));
    assert_ne!(token_hash("a"), token_hash("b"));
    assert_eq!(token_hash("a").len(), 64);
}

#[test]
fn the_state_reports_its_version_and_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    assert_eq!(state.api_version(), koru_store_core::version::API_VERSION);
    assert!(!state.capabilities().is_empty());
    assert!(state.capabilities().contains(&"reminders"));
    assert_eq!(state.channel, "internal-debug");
}

#[test]
fn catalog_changes_are_sticky_and_coalesced_by_generation() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let mut listening = state.subscribe_catalog_changes();

    assert_eq!(*listening.borrow_and_update(), 0);
    state.notify_catalog_change();
    state.notify_catalog_change();

    assert!(listening.has_changed().unwrap());
    assert_eq!(*listening.borrow_and_update(), 2);
    assert!(!listening.has_changed().unwrap());

    let late = state.subscribe_catalog_changes();
    assert_eq!(*late.borrow(), 2, "a late subscriber sees the sticky head");
}
