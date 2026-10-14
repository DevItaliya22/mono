// SPDX-License-Identifier: MPL-2.0

use super::*;

use koru_voice::VoiceLanguage;

#[test]
fn the_checked_in_smoke_fixture_loads_in_all_eight_languages() {
    // Plan 0.3's acceptance criterion, at the level a unit test can see it.
    let fixture = SmokeFixture::load(&SmokeFixture::default_root()).expect("the fixture loads");
    assert_eq!(fixture.utterances.len(), 8);
    let tags: Vec<&str> = fixture
        .utterances
        .iter()
        .map(|u| u.language.tag())
        .collect();
    assert_eq!(
        tags,
        VoiceLanguage::ALL.map(|l| l.tag()).to_vec(),
        "the fixture is loaded in VoiceLanguage::ALL order, so a picker built \
         from either list reads the same"
    );
    for utterance in &fixture.utterances {
        assert!(!utterance.reference.is_empty(), "{}", utterance.id);
        assert!(utterance.path.is_file(), "{}", utterance.path.display());
    }
}

#[test]
fn the_fixture_says_when_it_is_a_placeholder() {
    // The bit the run header repeats. When somebody records the real eight and
    // deletes the marker in the same change, this flips and the header stops
    // saying so — which is the whole point of it being a file rather than a
    // comment.
    let fixture = SmokeFixture::load(&SmokeFixture::default_root()).expect("loads");
    let marker = SmokeFixture::default_root().join("PLACEHOLDER");
    assert_eq!(fixture.is_placeholder(), marker.is_file());
}

#[test]
fn every_reference_carries_the_seven_product_names() {
    // The probe cannot report a hit on a name nobody was asked to say.
    let fixture = SmokeFixture::load(&SmokeFixture::default_root()).expect("loads");
    for utterance in &fixture.utterances {
        for name in PROPER_NOUNS {
            assert!(
                utterance.reference.contains(name),
                "{} does not ask for {name}",
                utterance.language.tag()
            );
        }
    }
}

#[test]
fn a_missing_language_is_a_refusal_rather_than_seven_of_eight() {
    // Running seven languages silently is how a language stops being tested.
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("transcripts.tsv"), "en\thello\n").expect("write");
    std::fs::write(dir.path().join("en.wav"), b"").expect("write");
    let error = SmokeFixture::load(dir.path()).expect_err("seven of eight is refused");
    assert!(error.to_string().contains("all eight languages are"));
}

#[test]
fn a_manifest_for_another_protocol_revision_is_refused() {
    // A corpus that moves under a benchmark makes two runs incomparable, and a
    // protocol that moves under a corpus does the same thing more quietly.
    let dir = tempfile::tempdir().expect("a temp dir");
    let manifest = serde_json::json!({
        "protocol_rev": crate::PROTOCOL_REV + 1,
        "converter": "ffmpeg 7.1.1",
        "sources": []
    });
    std::fs::write(
        dir.path().join("MANIFEST.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .expect("write");
    let error = Corpus::open(dir.path()).expect_err("a revision mismatch is refused");
    assert!(error.to_string().contains("refusal rather than a warning"));
}

#[test]
fn a_manifest_at_this_revision_opens_and_carries_its_pins() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let manifest = serde_json::json!({
        "protocol_rev": crate::PROTOCOL_REV,
        "converter": "ffmpeg 7.1.1",
        "sources": [{
            "source_id": "fleurs",
            "source_revision": "3f2a91c",
            "source_sha256": "8c1e",
            "converted_sha256": "44b0"
        }]
    });
    std::fs::write(
        dir.path().join("MANIFEST.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .expect("write");
    let corpus = Corpus::open(dir.path()).expect("opens");
    let pin = corpus
        .manifest
        .pin(SourceId::Fleurs)
        .expect("the fleurs pin is there");
    // **Two hashes, deliberately.** Comparable when the source matches;
    // identical when the converted tree matches too.
    assert_eq!(pin.source_sha256, "8c1e");
    assert_eq!(pin.converted_sha256, "44b0");
    // No t2s table is not an error here — it is a note the header carries.
    assert!(corpus.t2s.is_empty());
}

#[test]
fn a_t2s_table_beside_the_manifest_is_loaded() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let manifest = serde_json::json!({
        "protocol_rev": crate::PROTOCOL_REV,
        "converter": "ffmpeg 7.1.1",
        "sources": []
    });
    std::fs::write(
        dir.path().join("MANIFEST.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .expect("write");
    std::fs::write(dir.path().join(crate::normalise::T2S_FILE), "開\t开\n").expect("write");
    let corpus = Corpus::open(dir.path()).expect("opens");
    assert_eq!(corpus.t2s.len(), 1);
}

#[test]
fn the_three_language_code_mismatches_are_what_the_corpora_actually_use() {
    // A harness written from language names would get all three wrong.
    assert_eq!(fleurs_config(VoiceLanguage::Es), "es_419");
    assert_eq!(commonvoice_locale(VoiceLanguage::Es), "es");
    assert_eq!(fleurs_config(VoiceLanguage::PtBr), "pt_br");
    assert_eq!(commonvoice_locale(VoiceLanguage::PtBr), "pt");
    assert_eq!(fleurs_config(VoiceLanguage::Ar), "ar_eg");
    assert_eq!(fleurs_config(VoiceLanguage::ZhHans), "cmn_hans_cn");
    assert_eq!(commonvoice_locale(VoiceLanguage::ZhHans), "zh-CN");
}

#[test]
fn aishell_exists_for_simplified_chinese_and_for_nothing_else() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let manifest = serde_json::json!({
        "protocol_rev": crate::PROTOCOL_REV,
        "converter": "ffmpeg 7.1.1",
        "sources": []
    });
    std::fs::write(
        dir.path().join("MANIFEST.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .expect("write");
    let corpus = Corpus::open(dir.path()).expect("opens");
    assert!(
        corpus
            .directory(SourceId::Aishell1, VoiceLanguage::ZhHans)
            .is_some()
    );
    assert!(
        corpus
            .directory(SourceId::Aishell1, VoiceLanguage::En)
            .is_none()
    );
}

#[test]
fn a_repeated_utterance_id_in_a_transcript_is_an_error() {
    // Two references for one id means the tree was assembled wrong, and picking
    // one silently is how a corpus with a build mistake produces numbers nobody
    // can reproduce.
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("transcripts.tsv");
    std::fs::write(&path, "a\tone\na\ttwo\n").expect("write");
    let error = read_transcripts(&path).expect_err("a duplicate is refused");
    assert!(error.to_string().contains("repeats the utterance id a"));
}

#[test]
fn a_transcript_line_with_no_tab_names_its_line() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("transcripts.tsv");
    std::fs::write(&path, "a\tone\nbroken\n").expect("write");
    let error = read_transcripts(&path).expect_err("refused");
    assert!(error.to_string().contains(":2 has no tab"));
}

#[test]
fn an_absent_speakers_file_is_fine_because_fleurs_has_none() {
    // FLEURS carries `gender` but no speaker id, so a per-speaker analysis is
    // reportable only for Common Voice and AISHELL-1.
    let dir = tempfile::tempdir().expect("a temp dir");
    let map = read_speakers(&dir.path().join("speakers.tsv")).expect("absent is fine");
    assert!(map.is_empty());
}

#[test]
fn a_source_column_is_short_but_its_id_is_not() {
    // The table column is six characters wide; the manifest key is the real
    // name, and confusing the two would make a manifest unreadable.
    assert_eq!(SourceId::Aishell1.as_str(), "aishell1");
    assert_eq!(SourceId::Aishell1.column(), "aishel");
    assert_eq!(SourceId::CommonVoice.as_str(), "commonvoice");
    assert_eq!(SourceId::CommonVoice.column(), "cv");
}
