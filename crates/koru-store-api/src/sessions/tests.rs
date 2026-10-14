// SPDX-License-Identifier: MPL-2.0

use super::*;

const NOW: i64 = 1_700_000_000_000;

#[test]
fn a_minted_session_resolves_to_what_it_was_given() {
    let sessions = Sessions::new();
    let (token, expires) = sessions.mint("koru", vec![Scope::ReadDocuments], NOW);
    assert_eq!(expires, NOW + DEFAULT_SESSION_TTL_MILLIS);
    let caller = sessions.resolve(&token, NOW).expect("live");
    assert_eq!(caller.label, "koru");
    assert!(caller.allows(Scope::ReadDocuments));
    assert!(!caller.allows(Scope::ManageWorkspaces));
}

#[test]
fn two_clients_never_get_the_same_token() {
    // The rendezvous mints per connection. If two connections shared a token,
    // revoking one would revoke both and the whole point would be lost.
    let sessions = Sessions::new();
    let (a, _) = sessions.mint("koru", Scope::ALL.to_vec(), NOW);
    let (b, _) = sessions.mint("koru", Scope::ALL.to_vec(), NOW);
    assert_ne!(a, b);
    assert!(sessions.resolve(&a, NOW).is_some());
    assert!(sessions.resolve(&b, NOW).is_some());
}

#[test]
fn an_expired_session_is_indistinguishable_from_an_unknown_one() {
    // Both `None`. "That was valid until Tuesday" confirms the token was real.
    let sessions = Sessions::new();
    let (token, expires) = sessions.mint("koru", Scope::ALL.to_vec(), NOW);
    assert!(sessions.resolve(&token, expires - 1).is_some());
    assert!(sessions.resolve(&token, expires).is_none());
    assert!(
        sessions
            .resolve("a-token-that-never-existed", NOW)
            .is_none()
    );
}

#[test]
fn minting_sweeps_what_has_expired() {
    // The only thing that grows the map is minting, so it is the only moment
    // that can need pruning. Nothing walks it on a timer.
    let sessions = Sessions::new();
    let (old, _) = sessions.mint("koru", Scope::ALL.to_vec(), NOW);
    let later = NOW + DEFAULT_SESSION_TTL_MILLIS + 1;
    assert_eq!(sessions.count(NOW), 1);
    sessions.mint("koru", Scope::ALL.to_vec(), later);
    assert_eq!(sessions.count(later), 1, "the expired one should be gone");
    assert!(sessions.resolve(&old, later).is_none());
}

#[test]
fn the_token_itself_is_never_held() {
    // What is stored is a hash, so a memory dump yields hashes and not keys.
    // Asserted through behaviour: a token whose hash is not present cannot
    // resolve, and the map is keyed by nothing else.
    let sessions = Sessions::new();
    let (token, _) = sessions.mint("koru", Scope::ALL.to_vec(), NOW);
    let live = sessions.live.lock().unwrap();
    let key = live.keys().next().unwrap();
    assert_ne!(key.as_slice(), token.as_bytes());
    assert_eq!(key, &crate::state::token_hash(&token));
}

#[test]
fn a_token_is_long_enough_to_be_worth_having() {
    // Two v4 UUIDs: 244 bits of randomness. One would be 122, which is still a
    // lot and is not what was chosen — the comment beside `mint` says so, and
    // this is what stops a tidy-up from halving it silently.
    let sessions = Sessions::new();
    let (token, _) = sessions.mint("koru", Scope::ALL.to_vec(), NOW);
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()), "{token}");
}

#[test]
fn session_lifetimes_accept_the_compact_store_policy_values() {
    assert_eq!(SessionTtl::parse("5m").unwrap().millis(), 5 * 60 * 1_000);
    assert_eq!(SessionTtl::parse("10m").unwrap().millis(), 10 * 60 * 1_000);
    assert_eq!(SessionTtl::parse("1h").unwrap().millis(), 60 * 60 * 1_000);
    assert_eq!(SessionTtl::parse("1s").unwrap().millis(), 1_000);
}

#[test]
fn an_invalid_session_lifetime_is_not_a_silent_new_policy() {
    for value in ["", "0m", "25h", "1d", "5µ"] {
        assert!(SessionTtl::parse(value).is_err(), "{value}");
    }
}

#[test]
fn a_configured_lifetime_is_what_the_store_mints() {
    let sessions = Sessions::with_ttl(SessionTtl::parse("5m").unwrap());
    let (_, expires) = sessions.mint("koru", Scope::ALL.to_vec(), NOW);
    assert_eq!(expires, NOW + 5 * 60 * 1_000);
}
