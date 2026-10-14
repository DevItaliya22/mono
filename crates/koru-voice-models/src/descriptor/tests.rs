// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::fixtures;
use koru_voice::Requested;

/// **The signed bytes, spelled out.**
///
/// The one test in this crate that is allowed to be a golden string, because
/// the golden string *is* the specification: a signing tool written in another
/// language has this paragraph and nothing else to work from. Changing it means
/// bumping [`DESCRIPTOR_SCHEMA_VERSION`] and re-signing every catalog entry,
/// which is a decision rather than an edit — so the test exists to make the
/// edit impossible to make by accident.
#[test]
fn the_canonical_form_is_exactly_this() {
    let bytes = fixtures::descriptor().canonical_bytes().unwrap();
    let expected = concat!(
        "koru-model-descriptor/1\n",
        "id\tparakeet-tdt-v3\n",
        "version\t1.2.0\n",
        "engine\tsherpa\n",
        "tier\tcompact\n",
        "languages\ten,zh-Hans\n",
        "size\t3\n",
        "digest\tba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n",
        "working_set_bytes\t1073741824\n",
        "licence\tApache-2.0\n",
        "attribution\tk2-fsa\n",
    );
    assert_eq!(String::from_utf8(bytes).unwrap(), expected);
}

/// Every field is in there. Stated as a per-field assertion rather than trusted
/// to the golden string above, because the failure mode ADR-250 is about is a
/// field quietly *leaving* the envelope, and a reviewer comparing two long
/// strings would not see it.
#[test]
fn every_behaviour_bearing_field_is_inside_the_envelope() {
    let base = fixtures::descriptor();
    let mutations: Vec<(&str, ModelDescriptor)> = vec![
        (
            "id",
            ModelDescriptor {
                id: koru_voice::ModelId::parse("other-model").unwrap(),
                ..base.clone()
            },
        ),
        (
            "version",
            ModelDescriptor {
                version: ModelVersion::parse("1.2.1").unwrap(),
                ..base.clone()
            },
        ),
        (
            "engine",
            ModelDescriptor {
                engine: koru_voice::EngineId::parse("whisper").unwrap(),
                ..base.clone()
            },
        ),
        (
            "tier",
            ModelDescriptor {
                tier: ModelTier::HighAccuracy,
                ..base.clone()
            },
        ),
        (
            "languages",
            ModelDescriptor {
                languages: vec![koru_voice::VoiceLanguage::En],
                ..base.clone()
            },
        ),
        (
            "size",
            ModelDescriptor {
                size: base.size + 1,
                ..base.clone()
            },
        ),
        (
            "digest",
            ModelDescriptor {
                digest: Sha256Digest::parse(
                    "0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap(),
                ..base.clone()
            },
        ),
        (
            "working_set_bytes",
            ModelDescriptor {
                working_set_bytes: base.working_set_bytes * 2,
                ..base.clone()
            },
        ),
        (
            "licence",
            ModelDescriptor {
                licence: LicenceRef::parse("GPL-3.0-only").unwrap(),
                ..base.clone()
            },
        ),
        (
            "attribution",
            ModelDescriptor {
                attribution: "somebody else".into(),
                ..base.clone()
            },
        ),
    ];
    let original = base.canonical_bytes().unwrap();
    for (field, changed) in mutations {
        assert_ne!(
            changed.canonical_bytes().unwrap(),
            original,
            "changing {field} did not change the signed bytes"
        );
    }
}

/// The one free-text field cannot forge a line.
///
/// `attribution` is last in the canonical form, which already limits the damage
/// — but relying on field order for a security property is how a reordering
/// becomes a vulnerability. The escape is what holds, and this is the test that
/// says so: an attribution that spells out a whole `licence` line must not
/// produce the same bytes as a descriptor that actually declares that licence.
#[test]
fn attribution_cannot_smuggle_a_field_boundary() {
    let honest = ModelDescriptor {
        attribution: "k2-fsa".into(),
        licence: LicenceRef::parse("GPL-3.0-only").unwrap(),
        ..fixtures::descriptor()
    };
    let forged = ModelDescriptor {
        attribution: "k2-fsa\nlicence\tGPL-3.0-only".into(),
        ..fixtures::descriptor()
    };
    assert_ne!(
        forged.canonical_bytes().unwrap(),
        honest.canonical_bytes().unwrap()
    );
    let text = String::from_utf8(forged.canonical_bytes().unwrap()).unwrap();
    assert!(
        text.ends_with("attribution\tk2-fsa\\nlicence\\tGPL-3.0-only\n"),
        "{text}"
    );
    // One trailing newline, so the escaped payload cannot have added a line.
    assert_eq!(text.lines().count(), 11);
}

/// Backslash is escaped first, or the escapes escape each other and two
/// different attributions canonicalise the same.
#[test]
fn the_escape_is_injective() {
    let a = ModelDescriptor {
        attribution: "a\\nb".into(),
        ..fixtures::descriptor()
    };
    let b = ModelDescriptor {
        attribution: "a\nb".into(),
        ..fixtures::descriptor()
    };
    assert_ne!(a.canonical_bytes().unwrap(), b.canonical_bytes().unwrap());
}

/// A shape this build does not understand is refused *before* it is
/// canonicalised — otherwise the failure arrives as a signature failure and an
/// old build looks like an attack.
#[test]
fn an_unknown_schema_is_refused_rather_than_canonicalised() {
    let future = ModelDescriptor {
        schema_version: DESCRIPTOR_SCHEMA_VERSION + 1,
        ..fixtures::descriptor()
    };
    assert_eq!(
        future.canonical_bytes().unwrap_err().code,
        ModelErrorCode::SchemaUnsupported
    );
    assert_eq!(
        future.validate().unwrap_err().code,
        ModelErrorCode::SchemaUnsupported
    );
}

#[test]
fn validate_refuses_what_no_single_field_can_see() {
    let cases: Vec<(&str, ModelDescriptor)> = vec![
        (
            "no languages",
            ModelDescriptor {
                languages: Vec::new(),
                ..fixtures::descriptor()
            },
        ),
        (
            "a duplicated language",
            ModelDescriptor {
                languages: vec![koru_voice::VoiceLanguage::En, koru_voice::VoiceLanguage::En],
                ..fixtures::descriptor()
            },
        ),
        (
            "a zero-byte archive",
            ModelDescriptor {
                size: 0,
                ..fixtures::descriptor()
            },
        ),
        (
            "a zero working set",
            ModelDescriptor {
                working_set_bytes: 0,
                ..fixtures::descriptor()
            },
        ),
        (
            "no attribution",
            ModelDescriptor {
                attribution: "   ".into(),
                ..fixtures::descriptor()
            },
        ),
        (
            "a control character",
            ModelDescriptor {
                attribution: "k2\u{0}fsa".into(),
                ..fixtures::descriptor()
            },
        ),
    ];
    for (what, descriptor) in cases {
        assert_eq!(
            descriptor.validate().unwrap_err().code,
            ModelErrorCode::DescriptorInvalid,
            "{what} was accepted"
        );
    }
    fixtures::descriptor().validate().unwrap();
}

/// The version shares the id's charset by *asking* for it, so the two can never
/// drift. What matters at this boundary is that a catalog cannot name a
/// directory it should not: the id, the version and the digest are all path
/// components.
#[test]
fn a_version_is_a_path_component() {
    ModelVersion::parse("1.2.0").unwrap();
    ModelVersion::parse("2026-03-int8").unwrap();
    for bad in ["..", ".hidden", "1.2.0/../etc", "C:\\models", "Ünicode", ""] {
        let error = ModelVersion::parse(bad).unwrap_err();
        assert_eq!(error.code, ModelErrorCode::DescriptorInvalid, "{bad:?}");
        assert!(
            error.message.starts_with("a model version"),
            "{bad:?} reported as {}",
            error.message
        );
    }
}

/// One set of bytes has exactly one directory name, so an uppercase digest is
/// refused rather than folded. Windows and macOS would treat the two spellings
/// as one directory and Linux as two — a corruption that only reproduces on
/// somebody else's machine.
#[test]
fn a_digest_is_sixty_four_lowercase_hex_characters() {
    Sha256Digest::parse(fixtures::ABC_SHA256).unwrap();
    for bad in [
        fixtures::ABC_SHA256.to_uppercase(),
        fixtures::ABC_SHA256[..63].to_string(),
        format!("{}0", fixtures::ABC_SHA256),
        "zz7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string(),
        String::new(),
    ] {
        assert_eq!(
            Sha256Digest::parse(bad.clone()).unwrap_err().code,
            ModelErrorCode::DescriptorInvalid,
            "{bad:?}"
        );
    }
}

/// Length first, then the hash — and the two failures are different codes,
/// because a length that disagrees names a truncated download while a digest
/// that disagrees names substituted bytes.
#[test]
fn an_archive_is_checked_by_length_and_then_by_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.bin");
    std::fs::write(&path, fixtures::archive_bytes()).unwrap();
    fixtures::descriptor().verify_archive(&path).unwrap();

    let longer = ModelDescriptor {
        size: 4,
        ..fixtures::descriptor()
    };
    assert_eq!(
        longer.verify_archive(&path).unwrap_err().code,
        ModelErrorCode::SizeMismatch
    );

    let elsewhere = ModelDescriptor {
        digest: Sha256Digest::parse(
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap(),
        ..fixtures::descriptor()
    };
    assert_eq!(
        elsewhere.verify_archive(&path).unwrap_err().code,
        ModelErrorCode::DigestMismatch
    );

    let missing = dir.path().join("absent.bin");
    assert_eq!(
        fixtures::descriptor()
            .verify_archive(&missing)
            .unwrap_err()
            .code,
        ModelErrorCode::Io
    );
}

/// `<id>/<version>/<digest>` — cache identity is the digest.
#[test]
fn the_installed_path_carries_the_digest() {
    let descriptor = fixtures::descriptor();
    assert_eq!(
        descriptor.path_components(),
        ["parakeet-tdt-v3", "1.2.0", fixtures::ABC_SHA256]
    );
}

/// `Auto` is answered by any model: whether a *language* can be detected is the
/// engine's property, not the weights'.
#[test]
fn auto_is_covered_by_every_model_and_a_fixed_language_is_not() {
    let descriptor = fixtures::descriptor();
    assert!(descriptor.covers(Requested::Auto));
    assert!(descriptor.covers(koru_voice::VoiceLanguage::En.into()));
    assert!(descriptor.covers(koru_voice::VoiceLanguage::ZhHans.into()));
    assert!(!descriptor.covers(koru_voice::VoiceLanguage::Ja.into()));
}

/// The newtypes refuse a bad value on the way *in*, so a catalog cannot
/// deserialize a descriptor the constructors would have rejected.
#[test]
fn serde_cannot_smuggle_a_value_the_constructor_refuses() {
    let json = serde_json::to_string(&fixtures::descriptor()).unwrap();
    let back: ModelDescriptor = serde_json::from_str(&json).unwrap();
    assert_eq!(back, fixtures::descriptor());

    let tampered = json.replace(fixtures::ABC_SHA256, "not-a-digest");
    assert!(serde_json::from_str::<ModelDescriptor>(&tampered).is_err());
    let traversal = json.replace("\"1.2.0\"", "\"../../etc\"");
    assert!(serde_json::from_str::<ModelDescriptor>(&traversal).is_err());
}
