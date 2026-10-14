// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::fixtures::{SAMPLES, UNICODE_FILENAME};

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

// ------------------------------------------------------------------ time

#[test]
fn a_timestamp_is_a_number_not_a_rendering() {
    let t = Timestamp::from_millis(1_770_000_000_000);
    assert_eq!(t.millis(), 1_770_000_000_000);
    assert_eq!(t.to_string(), "1770000000000");
    // A filename stamp carries no calendar and no locale, and sorts lexically
    // the way it sorts chronologically.
    assert!(t.for_filename().chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn filename_stamps_sort_in_time_order() {
    let a = Timestamp::from_millis(1_770_000_000_000).for_filename();
    let b = Timestamp::from_millis(1_770_000_000_001).for_filename();
    assert!(a < b, "{a} !< {b}");
}

#[test]
fn now_is_after_the_epoch() {
    assert!(Timestamp::now().millis() > 1_600_000_000_000);
}

// ------------------------------------------------------------------ names

#[test]
fn a_name_in_every_supported_language_is_accepted() {
    for s in SAMPLES {
        let p = Profile::new(s.title, s.locale(), at())
            .unwrap_or_else(|e| panic!("{} rejected: {e}", s.tag));
        assert_eq!(p.name, s.title);
        assert_eq!(p.locale.as_str(), s.tag);
    }
}

#[test]
fn a_name_is_trimmed_but_never_otherwise_rewritten() {
    let w = Workspace::new(ProfileId::new(), "  季度计划  ", at()).unwrap();
    assert_eq!(w.name, "季度计划");
    // Not normalised, not case-folded, not stripped of controls: it is theirs.
    let odd = "Gru\u{0308}ße \u{202B}RTL\u{202C}";
    assert_eq!(
        Workspace::new(ProfileId::new(), odd, at()).unwrap().name,
        odd
    );
}

#[test]
fn an_empty_or_whitespace_name_is_rejected() {
    assert_eq!(
        Profile::new("", Locale::default(), at())
            .unwrap_err()
            .code(),
        Code::InvalidRequest
    );
    assert!(Profile::new("   ", Locale::default(), at()).is_err());
}

#[test]
fn the_name_limit_counts_characters_not_bytes() {
    // The invariant: a limit in bytes is a different product in each language.
    // 200 Japanese characters is 600 bytes and must be accepted.
    let long_ja: String = std::iter::repeat_n('計', 200).collect();
    assert!(long_ja.len() > 200, "the fixture is not multibyte");
    assert!(Workspace::new(ProfileId::new(), &long_ja, at()).is_ok());

    let too_long: String = std::iter::repeat_n('計', 201).collect();
    assert!(Workspace::new(ProfileId::new(), &too_long, at()).is_err());

    // And the same limit in English, so neither language gets a private rule.
    let long_en: String = std::iter::repeat_n('a', 200).collect();
    assert!(Workspace::new(ProfileId::new(), &long_en, at()).is_ok());
}

// ------------------------------------------------------------------ profile

#[test]
fn a_profile_needs_no_account() {
    let p = Profile::new("Work", Locale::parse("de").unwrap(), at()).unwrap();
    assert_eq!(p.account, None, "creating a profile must be fully offline");
}

#[test]
fn a_profile_round_trips_through_json() {
    let p = Profile::new("Trabajo", Locale::parse("es").unwrap(), at()).unwrap();
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<Profile>(&json).unwrap(), p);
    // Absent optional fields are omitted rather than serialised as null.
    assert!(!json.contains("account"), "{json}");
}

/// Onboarding knows what a profile is for and what colour it wears, and both
/// have to survive the trip. Neither is interpreted here — `purpose` is the
/// person's own word and `avatar` is a token this crate never parses.
#[test]
fn a_purpose_and_an_avatar_are_carried_and_not_read() {
    let p = Profile::new("Steven", Locale::parse("en").unwrap(), at())
        .unwrap()
        .with_purpose(Some("personal"))
        .with_avatar(Some("color:purple"));
    assert_eq!(p.purpose.as_deref(), Some("personal"));
    assert_eq!(p.avatar.as_deref(), Some("color:purple"));

    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<Profile>(&json).unwrap(), p);

    // A token this build has never heard of is stored, not refused: a store
    // that validated the vocabulary would have to ship before any client could
    // adopt a new kind of avatar.
    let future = Profile::new("Steven", Locale::default(), at())
        .unwrap()
        .with_avatar(Some("blob:b3:deadbeef"));
    assert_eq!(future.avatar.as_deref(), Some("blob:b3:deadbeef"));
}

/// An untouched field must not become `Some("")` — a third state between "they
/// chose this" and "they chose nothing", which nothing downstream branches on.
#[test]
fn an_empty_purpose_or_avatar_is_absent_rather_than_blank() {
    let p = Profile::new("Steven", Locale::default(), at())
        .unwrap()
        .with_purpose(Some("   "))
        .with_avatar(Some(""));
    assert_eq!(p.purpose, None);
    assert_eq!(p.avatar, None);

    let json = serde_json::to_string(&p).unwrap();
    assert!(!json.contains("purpose"), "{json}");
    assert!(!json.contains("avatar"), "{json}");
}

// ------------------------------------------------------------------ enums are tokens

#[test]
fn every_enum_value_is_a_stable_ascii_token() {
    let tokens: Vec<&str> = DocumentKind::parse("manifest")
        .into_iter()
        .map(|k| k.as_str())
        .chain(
            LineageKind::parse("restore")
                .into_iter()
                .map(|k| k.as_str()),
        )
        .chain(Scope::ALL.iter().map(|s| s.as_str()))
        .collect();
    for t in tokens {
        assert!(
            t.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{t} is not a stable token"
        );
    }
}

#[test]
fn enum_tokens_round_trip() {
    for k in [DocumentKind::Manifest, DocumentKind::Document] {
        assert_eq!(DocumentKind::parse(k.as_str()), Some(k));
    }
    for k in [LineageKind::Restore, LineageKind::Duplicate] {
        assert_eq!(LineageKind::parse(k.as_str()), Some(k));
    }
    for s in Scope::ALL {
        assert_eq!(Scope::parse(s.as_str()), Some(*s));
    }
}

#[test]
fn an_unknown_token_is_none_not_a_default() {
    // Defaulting an unrecognised value is how an older build silently
    // misreads a newer one.
    assert_eq!(DocumentKind::parse("canvas"), None);
    assert_eq!(LineageKind::parse("fork"), None);
    assert_eq!(Scope::parse("admin"), None);
    // And never a translated label.
    assert_eq!(LineageKind::parse("restaurar"), None);
    assert_eq!(DocumentKind::parse("マニフェスト"), None);
}

#[test]
fn serde_uses_the_same_tokens_as_as_str() {
    assert_eq!(
        serde_json::to_string(&DocumentKind::Manifest).unwrap(),
        "\"manifest\""
    );
    assert_eq!(
        serde_json::to_string(&Scope::ManageWorkspaces).unwrap(),
        "\"manage_workspaces\""
    );
    assert_eq!(
        serde_json::to_string(&LineageKind::Duplicate).unwrap(),
        "\"duplicate\""
    );
}

// ------------------------------------------------------------------ blobs

#[test]
fn a_blob_hash_carries_its_algorithm() {
    let h = BlobHash::from_digest(BlobHash::BLAKE3, &"AB".repeat(32)).unwrap();
    assert_eq!(h.algorithm(), "b3");
    assert_eq!(h.digest(), "ab".repeat(32), "digests are stored lowercase");
    assert_eq!(h.as_str(), format!("b3:{}", "ab".repeat(32)));
}

#[test]
fn a_blob_hash_round_trips_through_parse() {
    let s = format!("b3:{}", "0f".repeat(32));
    assert_eq!(BlobHash::parse(&s).unwrap().as_str(), s);
}

#[test]
fn a_hash_without_an_algorithm_is_rejected() {
    // A bare digest cannot be migrated: adopting a second algorithm would
    // orphan everything already written.
    assert!(BlobHash::parse(&"ab".repeat(32)).is_err());
    assert!(BlobHash::parse("b3:not-hex").is_err());
    assert!(BlobHash::parse("").is_err());
}

#[test]
fn the_on_disk_name_is_the_hash_never_the_filename() {
    let h = BlobHash::from_digest("b3", &"c3".repeat(32)).unwrap();
    let name = h.file_name();
    assert_eq!(name, format!("b3-{}", "c3".repeat(32)));
    // No colon: it is illegal in a Windows path.
    assert!(!name.contains(':'), "{name}");
    assert!(name.is_ascii(), "{name}");
}

#[test]
fn a_unicode_filename_is_metadata_and_survives_intact() {
    let blob = Blob {
        hash: BlobHash::from_digest("b3", &"11".repeat(32)).unwrap(),
        size: 1024,
        mime: "application/pdf".into(),
        filename: Some(UNICODE_FILENAME.to_string()),
        created_at: at(),
        attributes: Default::default(),
    };
    let back: Blob = serde_json::from_str(&serde_json::to_string(&blob).unwrap()).unwrap();
    assert_eq!(back.filename.as_deref(), Some(UNICODE_FILENAME));
    // Identity is content, and the filename plays no part in it.
    assert_eq!(back.hash, blob.hash);
    assert!(!back.hash.file_name().contains(' '));
}

#[test]
fn blob_attributes_are_additive_on_the_wire() {
    // A peer running an older build serializes no `attributes` key at all;
    // it must decode as empty rather than fail the frame.
    let without = serde_json::json!({
        "hash": format!("b3:{}", "22".repeat(32)),
        "size": 7,
        "mime": "image/png",
        "created_at": 1,
    });
    let back: Blob = serde_json::from_value(without).unwrap();
    assert!(back.attributes.is_empty());

    // And a blob written *with* attributes keeps them across the round trip.
    let mut blob = Blob {
        hash: BlobHash::from_digest("b3", &"33".repeat(32)).unwrap(),
        size: 7,
        mime: "image/png".into(),
        filename: None,
        created_at: at(),
        attributes: Default::default(),
    };
    blob.attributes.insert(
        crate::model::blob_attr::ORIGIN_DEVICE.to_string(),
        BlobAttribute::Text("5f9c0d7e-…".into()),
    );
    let back: Blob = serde_json::from_str(&serde_json::to_string(&blob).unwrap()).unwrap();
    assert_eq!(
        back.text_attribute(crate::model::blob_attr::ORIGIN_DEVICE),
        Some("5f9c0d7e-…")
    );
}

// ------------------------------------------------------------------ api clients

#[test]
fn a_revoked_client_allows_nothing() {
    let mut c = ApiClient {
        id: ClientId::new(),
        name: "test".into(),
        scopes: Scope::ALL.to_vec(),
        created_at: at(),
        revoked_at: None,
    };
    assert!(c.allows(Scope::EditDocuments));
    c.revoked_at = Some(at());
    assert!(!c.active());
    for s in Scope::ALL {
        assert!(!c.allows(*s), "{s} survived revocation");
    }
}

#[test]
fn a_client_is_denied_a_scope_it_was_not_given() {
    let c = ApiClient {
        id: ClientId::new(),
        name: "reader".into(),
        scopes: vec![Scope::ReadDocuments],
        created_at: at(),
        revoked_at: None,
    };
    assert!(c.allows(Scope::ReadDocuments));
    assert!(!c.allows(Scope::EditDocuments));
    assert!(!c.allows(Scope::ManageWorkspaces));
}

// --------------------------------------------------------------- profile kind

/// The tokens are storage. Renaming one orphans every row already written with
/// it, exactly as renaming a `NodeMeta.colour` value would.
#[test]
fn every_profile_kind_has_one_stable_token_and_parses_back_to_itself() {
    let mut seen = std::collections::HashSet::new();
    for kind in ProfileKind::ALL {
        assert!(seen.insert(kind.as_str()), "{kind} shares a token");
        assert_eq!(ProfileKind::parse(kind.as_str()), *kind);
    }
    assert_eq!(ProfileKind::Normal.as_str(), "normal");
    assert_eq!(ProfileKind::Holder.as_str(), "holder");
}

/// **A token this build does not know is a persona, and so is an absent one.**
///
/// The conservative reading: a persona is the kind that is safe to show and
/// safe to leave alone. Defaulting the other way would let one unrecognised
/// value hide somebody's profile from every switcher at once.
#[test]
fn an_unrecognised_kind_reads_as_a_persona() {
    assert_eq!(ProfileKind::parse("archivist"), ProfileKind::Normal);
    assert_eq!(ProfileKind::parse(""), ProfileKind::Normal);
    assert_eq!(ProfileKind::default(), ProfileKind::Normal);
}

/// A persona made the ordinary way is a persona, and only the holder
/// constructor makes a holder — there is no builder that flips a made profile
/// into one, because nothing should ever want to.
#[test]
fn only_the_holder_constructor_makes_a_holder() {
    let persona = Profile::new("Work", Locale::parse("en").unwrap(), at()).unwrap();
    assert!(!persona.is_holder());
    assert_eq!(persona.kind, ProfileKind::Normal);

    let holder = Profile::holder(Locale::parse("en").unwrap(), at()).unwrap();
    assert!(holder.is_holder());
    assert_eq!(holder.name, crate::model::HOLDER_NAME);
    assert_ne!(holder.id, persona.id);
}

/// **The stored name is not the shown name**, and this pins the direction of
/// that rule: the marker is the identity, and the name is untranslated ASCII a
/// raw catalog dump can be read with. A surface that branched on the name would
/// break in seven of the eight locales.
#[test]
fn the_holders_identity_is_its_kind_and_never_its_name() {
    let mut impostor = Profile::new(
        crate::model::HOLDER_NAME,
        Locale::parse("en").unwrap(),
        at(),
    )
    .unwrap();
    assert!(!impostor.is_holder(), "a name is not an identity");

    impostor.name = "Whatever".into();
    let holder = Profile::holder(Locale::parse("en").unwrap(), at()).unwrap();
    assert!(holder.is_holder());
}

/// The field is additive over the wire: a body written before it existed still
/// parses, and reads as a persona.
#[test]
fn a_profile_body_without_a_kind_still_parses_as_a_persona() {
    let body = serde_json::json!({
        "id": ProfileId::new().to_storage(),
        "name": "Work",
        "locale": "en",
        "created_at": at().millis(),
    });
    let parsed: Profile = serde_json::from_value(body).unwrap();
    assert!(!parsed.is_holder());
}
