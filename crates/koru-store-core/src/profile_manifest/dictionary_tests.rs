// SPDX-License-Identifier: MPL-2.0

use super::*;

fn manifest() -> ProfileManifest {
    let profile = Profile {
        id: ProfileId::new(),
        name: "Steven".into(),
        purpose: None,
        avatar: None,
        icon: None,
        color: None,
        account: None,
        locale: Locale::parse("en-US").expect("a valid locale"),
        created_at: Timestamp::from_millis(0),
        kind: ProfileKind::default(),
    };
    ProfileManifest::create(&profile).expect("a fresh manifest")
}

use crate::text::Locale;

#[test]
fn words_round_trip_per_language_and_removal_deletes() {
    let m = manifest();
    m.add_dictionary_word("en-US", "Loro").unwrap();
    m.add_dictionary_word("en-US", "iroh").unwrap();
    m.add_dictionary_word("de", "Koru").unwrap();
    // Idempotent: a second add stores nothing new.
    m.add_dictionary_word("en-US", "Loro").unwrap();

    let words = m.dictionary();
    assert_eq!(words["en-US"], vec!["Loro", "iroh"]);
    assert_eq!(words["de"], vec!["Koru"]);

    assert!(m.remove_dictionary_word("en-US", "Loro").unwrap());
    assert!(!m.remove_dictionary_word("en-US", "Loro").unwrap());
    assert!(!m.remove_dictionary_word("fr", "rien").unwrap());
    assert_eq!(m.dictionary()["en-US"], vec!["iroh"]);
}

/// Two of this person's devices each add their first word for one language
/// before they meet — both words must survive the merge, which is what
/// `ensure_mergeable_map` is for.
#[test]
fn two_devices_first_words_merge_rather_than_one_winning() {
    let a = manifest();
    let snapshot = a.export_snapshot().unwrap();
    let b = ProfileManifest::from_snapshot(&snapshot).unwrap();
    b.set_peer_id(2).unwrap();

    a.add_dictionary_word("en-US", "Loro").unwrap();
    b.add_dictionary_word("en-US", "iroh").unwrap();

    let from_b = b.export_updates_since(&loro::VersionVector::new()).unwrap();
    a.import_frames(&[from_b]).unwrap();
    assert_eq!(a.dictionary()["en-US"], vec!["Loro", "iroh"]);
}

#[test]
fn malformed_languages_and_words_are_refused() {
    let m = manifest();
    for language in ["", "x", "-en", "en-", "en US", "日本語", &"e".repeat(40)] {
        assert!(
            m.add_dictionary_word(language, "fine").is_err(),
            "{language:?}"
        );
    }
    for word in [
        "",
        "two words",
        "tab\tin",
        "ctrl\u{1}char",
        &"w".repeat(101),
    ] {
        assert!(m.add_dictionary_word("en-US", word).is_err(), "{word:?}");
    }
    // A hundred characters of any language is a word.
    m.add_dictionary_word("en-US", &"ü".repeat(100)).unwrap();
}

/// The container is additive: a manifest written before it existed reads an
/// empty dictionary, not an error, and creating one bumps no schema.
#[test]
fn an_older_manifest_reads_an_empty_dictionary() {
    let m = manifest();
    assert!(m.dictionary().is_empty());
}
