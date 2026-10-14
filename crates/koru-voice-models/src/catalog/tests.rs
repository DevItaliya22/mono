// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::descriptor::{DESCRIPTOR_SCHEMA_VERSION, Sha256Digest};
use crate::fixtures::{self, Signer};
use koru_voice::VoiceLanguage;

fn signed(descriptor: ModelDescriptor) -> CatalogEntry {
    fixtures::entry(&Signer::new(), descriptor)
}

/// The wire shape is flat — `id` and `url` are siblings — because that is how
/// Plan 1.1 writes an entry and how a publisher will hand-edit one. The Rust
/// struct groups by trust; the JSON does not have to.
#[test]
fn an_entry_is_flat_on_the_wire_and_grouped_in_rust() {
    let entry = signed(fixtures::descriptor());
    let json: serde_json::Value = serde_json::to_value(&entry).unwrap();
    for key in [
        "id",
        "version",
        "engine",
        "tier",
        "languages",
        "size",
        "digest",
        "working_set_bytes",
        "licence",
        "attribution",
        "signature",
        "url",
    ] {
        assert!(json.get(key).is_some(), "{key} is not a top-level field");
    }
    assert!(json.get("descriptor").is_none());
    let back: CatalogEntry = serde_json::from_value(json).unwrap();
    assert_eq!(back, entry);
}

/// `mirrors` is optional, and the preferred URL comes first.
#[test]
fn the_sources_are_the_url_then_the_mirrors_in_order() {
    let mut entry = signed(fixtures::descriptor());
    assert_eq!(entry.sources().len(), 1);
    entry.mirrors = vec![
        Url::parse("https://mirror-a.invalid/m.tar.gz").unwrap(),
        Url::parse("https://mirror-b.invalid/m.tar.gz").unwrap(),
    ];
    let sources: Vec<String> = entry.sources().iter().map(|u| u.to_string()).collect();
    assert_eq!(sources[0], entry.url.to_string());
    assert!(sources[1].contains("mirror-a"));
    assert!(sources[2].contains("mirror-b"));
}

/// A mirror can be added without re-signing, which is the whole reason it is
/// outside the envelope — so changing one must not disturb verification.
#[test]
fn adding_a_mirror_does_not_invalidate_a_signature() {
    let signer = Signer::new();
    let mut entry = fixtures::entry(&signer, fixtures::descriptor());
    entry.verify(&signer.trusted).unwrap();
    entry
        .mirrors
        .push(Url::parse("https://added-later.invalid/m.tar.gz").unwrap());
    entry.url = Url::parse("https://moved.invalid/m.tar.gz").unwrap();
    entry.verify(&signer.trusted).unwrap();
}

#[test]
fn a_catalog_refuses_plain_http_and_a_repeated_identity() {
    let mut plain = fixtures::catalog(vec![signed(fixtures::descriptor())]);
    plain.entries[0].url = Url::parse("http://models.invalid/m.tar.gz").unwrap();
    assert_eq!(
        plain.validate().unwrap_err().code,
        ModelErrorCode::CatalogInvalid
    );

    let mut mirrored = fixtures::catalog(vec![signed(fixtures::descriptor())]);
    mirrored.entries[0].mirrors = vec![Url::parse("http://mirror.invalid/m.tar.gz").unwrap()];
    assert_eq!(
        mirrored.validate().unwrap_err().code,
        ModelErrorCode::CatalogInvalid
    );

    let twice = fixtures::catalog(vec![
        signed(fixtures::descriptor()),
        signed(fixtures::descriptor()),
    ]);
    assert_eq!(
        twice.validate().unwrap_err().code,
        ModelErrorCode::CatalogInvalid
    );
}

/// A catalog document of the wrong version is refused whole, because there is
/// nothing in it a build can be sure it read correctly.
#[test]
fn a_catalog_of_an_unknown_version_is_refused_whole() {
    let mut catalog = fixtures::catalog(vec![signed(fixtures::descriptor())]);
    catalog.schema_version = CATALOG_SCHEMA_VERSION + 1;
    assert_eq!(
        catalog.validate().unwrap_err().code,
        ModelErrorCode::CatalogInvalid
    );
}

/// **A model from the future must not stop today's models being installed.**
///
/// The catalog keeps the entry — refusing the document would make every new
/// model a breaking change for every old build — and `choose` skips it.
#[test]
fn an_entry_this_build_does_not_understand_is_kept_and_skipped() {
    let future = ModelDescriptor {
        schema_version: DESCRIPTOR_SCHEMA_VERSION + 1,
        id: koru_voice::ModelId::parse("from-the-future").unwrap(),
        ..fixtures::descriptor()
    };
    // Deliberately unsigned by this build: a descriptor whose canonical form we
    // do not know is one we *cannot* sign, which is the situation itself.
    let unsignable = CatalogEntry {
        descriptor: future,
        signature: ModelSignature::parse("signed by a later build").unwrap(),
        url: Url::parse("https://models.koru.do/v2/future.tar.gz").unwrap(),
        mirrors: Vec::new(),
    };
    let catalog = fixtures::catalog(vec![unsignable, signed(fixtures::descriptor())]);
    catalog.validate().unwrap();
    assert_eq!(catalog.entries.len(), 2);
    let chosen = catalog
        .choose(VoiceLanguage::En.into(), ModelTier::Compact, None)
        .unwrap();
    assert_eq!(chosen.id().as_str(), "parakeet-tdt-v3");
}

/// **The tier is exact.** A `HighAccuracy` request that quietly settles for
/// `Compact` gives a person a worse result than they chose and no way to find
/// out.
#[test]
fn a_tier_never_falls_back() {
    let catalog = fixtures::catalog(vec![signed(fixtures::descriptor())]);
    assert!(
        catalog
            .choose(VoiceLanguage::En.into(), ModelTier::Compact, None)
            .is_some()
    );
    assert!(
        catalog
            .choose(VoiceLanguage::En.into(), ModelTier::HighAccuracy, None)
            .is_none()
    );
}

#[test]
fn choosing_filters_by_language_and_by_engine() {
    let catalog = fixtures::catalog(vec![signed(fixtures::descriptor())]);
    let sherpa = koru_voice::EngineId::parse("sherpa").unwrap();
    let whisper = koru_voice::EngineId::parse("whisper").unwrap();
    assert!(
        catalog
            .choose(VoiceLanguage::Ja.into(), ModelTier::Compact, None)
            .is_none()
    );
    assert!(
        catalog
            .choose(
                koru_voice::Requested::Auto,
                ModelTier::Compact,
                Some(&sherpa)
            )
            .is_some()
    );
    assert!(
        catalog
            .choose(
                koru_voice::Requested::Auto,
                ModelTier::Compact,
                Some(&whisper)
            )
            .is_none()
    );
}

/// Catalog order is the publisher's preference order, so the first match wins
/// — deterministic, and without inventing a comparison over `ModelVersion`,
/// which is an opaque label rather than semver.
#[test]
fn the_first_match_in_catalog_order_wins() {
    let preferred = ModelDescriptor {
        version: ModelVersion::parse("2.0.0").unwrap(),
        digest: Sha256Digest::parse(
            "1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap(),
        ..fixtures::descriptor()
    };
    let catalog = fixtures::catalog(vec![signed(preferred), signed(fixtures::descriptor())]);
    let chosen = catalog
        .choose(VoiceLanguage::En.into(), ModelTier::Compact, None)
        .unwrap();
    assert_eq!(chosen.version().as_str(), "2.0.0");
}

#[test]
fn parsing_reads_what_serializing_wrote() {
    let catalog = fixtures::catalog(vec![signed(fixtures::descriptor())]);
    let bytes = serde_json::to_vec(&catalog).unwrap();
    assert_eq!(Catalog::parse(&bytes).unwrap(), catalog);
    assert_eq!(
        Catalog::parse(b"not json").unwrap_err().code,
        ModelErrorCode::CatalogInvalid
    );
    let id = koru_voice::ModelId::parse("parakeet-tdt-v3").unwrap();
    let version = ModelVersion::parse("1.2.0").unwrap();
    assert!(catalog.find(&id, &version).is_some());
    assert!(
        catalog
            .find(&id, &ModelVersion::parse("9.9.9").unwrap())
            .is_none()
    );
}

// ------------------------------------------------------------------- source

/// A value naming a file that exists is a path; anything else is a URL. The
/// filesystem resolves the ambiguity, so nobody has to remember a prefix.
#[test]
fn a_source_is_a_path_when_the_path_is_there_and_a_url_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("catalog.json");
    std::fs::write(&file, b"{}").unwrap();

    assert_eq!(
        CatalogSource::parse(&file.display().to_string()).unwrap(),
        CatalogSource::File(file.clone())
    );
    assert_eq!(
        CatalogSource::parse(DEFAULT_CATALOG_URL).unwrap(),
        CatalogSource::Url(Url::parse(DEFAULT_CATALOG_URL).unwrap())
    );
}

/// **A catalog is never fetched over plain HTTP.** The same rule an entry's URL
/// obeys, applied one step earlier — the document that carries the signatures
/// is not less worth protecting than the archives it names.
#[test]
fn a_plain_http_catalog_is_refused() {
    let error = CatalogSource::parse("http://models.example/catalog.json").unwrap_err();
    assert_eq!(error.code, ModelErrorCode::CatalogInvalid);
}

/// A value that is neither says so, rather than being fetched hopefully.
#[test]
fn a_source_that_is_neither_is_named_rather_than_guessed() {
    let error = CatalogSource::parse("not a url and not a file").unwrap_err();
    assert_eq!(error.code, ModelErrorCode::CatalogInvalid);
}

/// The offline path end to end: a file source reads and validates. **No test
/// in this crate reaches the network**, and this is the source that keeps that
/// true of the catalog as well as of the archives.
#[test]
fn a_file_source_reads_a_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    let catalog = fixtures::catalog(vec![signed(fixtures::descriptor())]);
    std::fs::write(&path, serde_json::to_vec(&catalog).unwrap()).unwrap();

    let read = CatalogSource::File(path).fetch().unwrap();
    assert_eq!(read, catalog);
}

/// **What a consent step shows is what a grant is recorded against.** The terms
/// come off the signed descriptor, so the sentence and the record cannot
/// disagree about the licence or the size.
#[test]
fn an_entrys_terms_are_the_signed_fields() {
    let entry = signed(fixtures::descriptor());
    let terms = entry.terms();
    assert_eq!(terms.model, entry.descriptor.id.as_str());
    assert_eq!(terms.version, entry.descriptor.version.as_str());
    assert_eq!(terms.licence, entry.descriptor.licence.as_str());
    assert_eq!(terms.attribution, entry.descriptor.attribution);
    assert_eq!(terms.download_bytes, entry.descriptor.size);
    assert!(koru_voice_consent::Granted::recorded(terms.clone(), 1).covers(&terms));
}
