// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::engine::ScriptedEngineHost;
use crate::fixtures;
use koru_voice::VoiceLanguage;

fn engine() -> [koru_voice::EngineId; 1] {
    [ScriptedEngineHost::engine()]
}

#[test]
fn a_request_that_names_no_model_chooses_by_language_tier_and_engine() {
    let store = MemoryModelStore::new(vec![fixtures::installed(
        fixtures::MODEL,
        fixtures::VERSION,
    )]);
    let chosen = resolve(
        &store,
        None,
        Requested::Fixed(VoiceLanguage::En),
        ModelTier::Compact,
        &engine(),
    )
    .unwrap();
    assert_eq!(chosen.id().as_str(), fixtures::MODEL);

    // A language the fixture does not cover finds nothing, and the code is the
    // one that is fixed by downloading something.
    let refused = resolve(
        &store,
        None,
        Requested::Fixed(VoiceLanguage::Ja),
        ModelTier::Compact,
        &engine(),
    )
    .unwrap_err();
    assert_eq!(refused.code, ErrorCode::ModelMissing);

    // And so does the tier a person has not installed.
    assert!(
        resolve(
            &store,
            None,
            Requested::Fixed(VoiceLanguage::En),
            ModelTier::HighAccuracy,
            &engine(),
        )
        .is_err()
    );
}

#[test]
fn two_compiled_engines_choose_the_model_for_the_requested_tier() {
    let compact = fixtures::installed("compact", "1.0.0");
    let mut high = fixtures::installed("high", "1.0.0");
    high.descriptor.engine = fixtures::other_engine();
    high.descriptor.tier = ModelTier::HighAccuracy;
    let store = MemoryModelStore::new(vec![compact, high]);
    let engines = [ScriptedEngineHost::engine(), fixtures::other_engine()];

    let compact = resolve(&store, None, Requested::Auto, ModelTier::Compact, &engines).unwrap();
    assert_eq!(compact.id().as_str(), "compact");
    let high = resolve(
        &store,
        None,
        Requested::Auto,
        ModelTier::HighAccuracy,
        &engines,
    )
    .unwrap();
    assert_eq!(high.id().as_str(), "high");
    assert_eq!(high.descriptor.engine, fixtures::other_engine());
}

/// **Only models for this build's engine are considered**, in both paths.
/// Offering a model for another runtime would turn the signed `engine` field's
/// check into a failure at load time instead of a choice that was never made.
#[test]
fn a_model_for_another_engine_is_never_chosen() {
    let mut other = fixtures::installed("other", "1.0.0");
    other.descriptor.engine = fixtures::other_engine();
    let store = MemoryModelStore::new(vec![other]);
    assert!(
        resolve(
            &store,
            None,
            Requested::Fixed(VoiceLanguage::En),
            ModelTier::Compact,
            &engine()
        )
        .is_err()
    );
    let named = ModelId::parse("other").unwrap();
    assert!(
        resolve(
            &store,
            Some(&named),
            Requested::Fixed(VoiceLanguage::En),
            ModelTier::Compact,
            &engine()
        )
        .is_err()
    );
}

/// A named model wins over the language choice — a bench or a test pinning one
/// gets exactly that one.
#[test]
fn a_named_model_is_taken_at_its_newest_installed_build() {
    let store = MemoryModelStore::new(vec![
        fixtures::installed(fixtures::MODEL, "1.0.0"),
        fixtures::installed(fixtures::MODEL, "1.2.0"),
        fixtures::installed("another", "9.9.9"),
    ]);
    let named = ModelId::parse(fixtures::MODEL).unwrap();
    let chosen = resolve(
        &store,
        Some(&named),
        Requested::Fixed(VoiceLanguage::En),
        ModelTier::Compact,
        &engine(),
    )
    .unwrap();
    assert_eq!(chosen.version().as_str(), "1.2.0");

    let missing = ModelId::parse("nobody-installed-this").unwrap();
    assert_eq!(
        resolve(
            &store,
            Some(&missing),
            Requested::Fixed(VoiceLanguage::En),
            ModelTier::Compact,
            &engine()
        )
        .unwrap_err()
        .code,
        ErrorCode::ModelMissing
    );
}

/// **A lease is released by dropping it, and there is deliberately no
/// `release()`** — so a thread that panics releases its lease, and so does one
/// that returns early down a path nobody wrote a cleanup for.
#[test]
fn a_lease_is_released_by_dropping_it() {
    let store = MemoryModelStore::new(vec![fixtures::installed(
        fixtures::MODEL,
        fixtures::VERSION,
    )]);
    let id = ModelId::parse(fixtures::MODEL).unwrap();
    let version = ModelVersion::parse(fixtures::VERSION).unwrap();
    assert_eq!(store.leases(&id, &version), 0);
    {
        let _lease = store.lease(&id, &version).unwrap();
        assert_eq!(store.leases(&id, &version), 1);
        // And removing it is refused while anything holds it — including, in
        // the real store, a process on another channel.
        assert!(store.remove(&id, &version).is_err());
    }
    assert_eq!(store.leases(&id, &version), 0);
    assert!(store.remove(&id, &version).is_ok());
}

/// `find` answers a question and `lease` is a demand, so only the second one
/// has a right to fail.
#[test]
fn find_answers_none_rather_than_failing() {
    let store = MemoryModelStore::new(Vec::new());
    let id = ModelId::parse(fixtures::MODEL).unwrap();
    let version = ModelVersion::parse(fixtures::VERSION).unwrap();
    assert!(store.find(&id, &version).unwrap().is_none());
    assert!(store.lease(&id, &version).is_err());
}

/// Every path the store answers with lives under its root, which is the
/// invariant that keeps `KORU_VOICE_MODELS` from silently writing outside the
/// profile it was pointed away from.
#[test]
fn every_installed_path_is_under_the_stores_root() {
    let store = MemoryModelStore::new(vec![fixtures::installed(
        fixtures::MODEL,
        fixtures::VERSION,
    )]);
    for model in store.installed().unwrap() {
        assert!(
            model.root.starts_with(store.root()),
            "{} is not under {}",
            model.root.display(),
            store.root().display()
        );
    }
}
