// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::cli::Options;

fn smoke() -> Options {
    Options {
        stage: Stage::Smoke,
        ..Options::default()
    }
}

#[test]
fn the_smoke_run_covers_all_eight_languages_and_reports_every_candidate_as_unavailable() {
    // **Plan 0.3's acceptance criterion.** The harness runs the smoke fixture in
    // all eight languages and, with no model present, reports *model missing*
    // and produces a report rather than an error.
    let run = execute(&smoke()).expect("the smoke run finishes");
    assert_eq!(
        run.unavailable.len(),
        crate::candidate::CANDIDATES.len(),
        "with nothing provisioned, every candidate should say why: {:?}",
        run.unavailable
    );
    for line in &run.unavailable {
        assert!(
            line.contains("model missing") || line.contains("runtime not built"),
            "{line}"
        );
    }
    // Every language was reached: the fixture is loaded before any candidate is
    // opened, so a failure to load one is a failure of the run.
    let fixture = SmokeFixture::load(&SmokeFixture::default_root()).expect("loads");
    assert_eq!(fixture.utterances.len(), 8);
}

#[test]
fn the_smoke_header_says_the_fixture_is_a_placeholder_while_it_is_one() {
    let run = execute(&smoke()).expect("finishes");
    let placeholder = SmokeFixture::default_root().join("PLACEHOLDER").is_file();
    let says = run.header.notes.iter().any(|n| n.contains("PLACEHOLDER"));
    assert_eq!(says, placeholder);
    // And it always says that nothing here reaches a decision table.
    assert!(
        run.header
            .notes
            .iter()
            .any(|n| n.contains("no number here appears in a decision table"))
    );
}

#[test]
fn the_smoke_header_carries_no_corpus_pins_because_there_is_no_corpus() {
    let run = execute(&smoke()).expect("finishes");
    assert!(run.header.corpus.is_empty());
    assert!(run.render().contains("corpus        none (smoke fixture)"));
}

#[test]
fn a_run_id_and_a_host_line_are_always_present() {
    // A run whose header is incomplete is a run whose numbers cannot be
    // compared, and that is true of a smoke run too — it is what says which
    // machine reported the harness working.
    let run = execute(&smoke()).expect("finishes");
    assert!(!run.header.run_id.is_empty());
    assert!(!run.header.host.is_empty());
    assert_eq!(run.header.protocol_rev, crate::PROTOCOL_REV);
}

#[test]
fn a_decision_run_over_a_corpus_with_no_t2s_table_refuses() {
    // A zh-Hans figure produced without the table scores a model's Traditional
    // output as wrong, which is a real difference from the protocol's number
    // and must not be invisible. The screen tolerates it with a note; the
    // decision run does not.
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

    let options = Options {
        stage: Stage::Decision,
        corpus: Some(dir.path().to_path_buf()),
        ..Options::default()
    };
    let error = execute(&options).expect_err("a decision run refuses");
    assert!(error.contains("opencc-t2s.tsv"), "{error}");

    // The same corpus at the screen stage runs and says so in the header.
    let screen = Options {
        stage: Stage::Screen,
        ..options
    };
    let run = execute(&screen).expect("the screen runs");
    assert!(
        run.header
            .notes
            .iter()
            .any(|n| n.contains("NOT the protocol's"))
    );
}

#[test]
fn a_request_never_asks_for_automatic_language_detection() {
    // The corpus knows the language, and the gate measures transcription rather
    // than detection; asking for `Auto` would make every row depend on a second
    // capability that only one candidate has.
    for language in VoiceLanguage::ALL {
        let request = request_for(language, 1);
        assert_eq!(request.language, Requested::Fixed(language));
        assert!(request.generation >= 1);
    }
}

#[test]
fn a_generation_is_never_zero() {
    // `Session` refuses a generation that is not strictly greater than the last
    // it ran, so a zero would be refused by the first thing that checked.
    assert_eq!(request_for(VoiceLanguage::En, 0).generation, 1);
}

#[test]
fn a_hash_is_shortened_for_the_header_and_not_truncated_to_nothing() {
    assert_eq!(short("8c1e0f77aabb"), "8c1e");
    assert_eq!(short("ab"), "ab");
    assert_eq!(short(""), "");
}

/// One measured utterance, built by hand so the folding functions can be tested
/// without a recogniser.
fn measured(
    language: VoiceLanguage,
    source: SourceId,
    speaker: Option<&str>,
    edits: usize,
    units: usize,
    rtf: Option<f64>,
) -> Measurement {
    Measurement {
        language,
        source,
        speaker: speaker.map(str::to_owned),
        ttfup: Some(Duration::from_millis(300)),
        censored_at: Duration::from_millis(900),
        final_latency: Some(Duration::from_millis(200)),
        revisions: 1,
        partials: 3,
        prefix_words: Some(2),
        error: ErrorCount {
            edits,
            reference_units: units,
        },
        drift: false,
        overruns: 0,
        rtf,
        cpu_percent: Some(140.0),
        probe_source: None,
    }
}

#[test]
fn a_cost_row_leaves_the_warm_load_absent_rather_than_inventing_it() {
    // The protocol's cold and warm loads are both properties of the page cache
    // *across process boundaries*. The second load of a model in one process is
    // not a warm load, it is the same mapped file — so a number here produced
    // by a loop would be a plausible wrong answer.
    let measurements = vec![
        measured(VoiceLanguage::En, SourceId::Fleurs, None, 1, 10, Some(0.21)),
        measured(VoiceLanguage::En, SourceId::Fleurs, None, 0, 10, Some(0.34)),
    ];
    let candidate = crate::candidate::Candidate::by_number(1).unwrap();
    let row = cost_row(
        candidate,
        &measurements,
        &Options::default(),
        Duration::from_millis(4180),
    );
    assert_eq!(row.load_first_ms, Some(4180.0));
    assert_eq!(row.load_warm_p50_ms, None);
    assert_eq!(row.load_warm_p95_ms, None);
    assert!(row.rtf_p50.is_some());
    // No `--models`, so there is nothing to size, and it is absent rather than
    // zero: `model_on_disk` is what M4 compares against and a zero would pass.
    assert_eq!(row.model_on_disk_bytes, None);
    assert_eq!(row.thr, 2);
}

#[test]
fn a_chinese_table_is_never_pooled_and_carries_a_per_speaker_spread() {
    // AISHELL-1 is clean read speech and Common Voice is crowd-sourced on
    // unknown microphones; a pooled CER describes neither, and which of the two
    // a candidate is bad at is exactly the decision.
    let measurements = vec![
        // Common Voice: two speakers, one of whom is far worse than the other.
        measured(
            VoiceLanguage::ZhHans,
            SourceId::CommonVoice,
            Some("a"),
            1,
            20,
            None,
        ),
        measured(
            VoiceLanguage::ZhHans,
            SourceId::CommonVoice,
            Some("b"),
            10,
            20,
            None,
        ),
        // AISHELL-1: one speaker.
        measured(
            VoiceLanguage::ZhHans,
            SourceId::Aishell1,
            Some("S0001"),
            1,
            50,
            None,
        ),
        // And an English row, which must not reach this table at all.
        measured(
            VoiceLanguage::En,
            SourceId::CommonVoice,
            Some("c"),
            5,
            10,
            None,
        ),
    ];
    let candidate = crate::candidate::Candidate::by_number(1).unwrap();
    let rows = chinese_rows(candidate, &measurements);
    let sources: Vec<&str> = rows.iter().map(|r| r.source.as_str()).collect();
    assert_eq!(sources, ["cv", "aishel"]);

    let cv = &rows[0];
    assert_eq!(cv.speakers, 2);
    // The spread is the point of the table: a max far above the median is the
    // failure mode a mean would hide.
    assert_eq!(cv.speaker_cer_min, Some(0.05));
    assert_eq!(cv.speaker_cer_max, Some(0.5));
    // And the corpus figure is total edits over total units, not the mean of
    // those two rates.
    let pooled = cv.cer.expect("a rate");
    assert!((pooled - 11.0 / 40.0).abs() < 1e-12, "{pooled}");
}

#[test]
fn a_source_with_no_speaker_ids_reports_no_spread_rather_than_a_zero() {
    // FLEURS carries `gender` and no speaker id at all.
    let measurements = vec![measured(
        VoiceLanguage::ZhHans,
        SourceId::Fleurs,
        None,
        2,
        20,
        None,
    )];
    let candidate = crate::candidate::Candidate::by_number(1).unwrap();
    let rows = chinese_rows(candidate, &measurements);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].speakers, 0);
    assert_eq!(rows[0].speaker_cer_median, None);
}

#[test]
fn a_median_of_nothing_is_absent_and_an_even_count_averages_the_middle_two() {
    assert_eq!(median(&[]), None);
    assert_eq!(median(&[1.0]), Some(1.0));
    assert_eq!(median(&[1.0, 3.0]), Some(2.0));
    assert_eq!(median(&[1.0, 2.0, 3.0]), Some(2.0));
}

#[test]
fn a_directory_size_walks_into_subdirectories_and_is_absent_when_it_cannot() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("a"), b"1234567890").expect("write");
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
    std::fs::write(dir.path().join("sub").join("b"), b"12345").expect("write");
    assert_eq!(directory_size(dir.path()), Some(15));
    assert_eq!(directory_size(&dir.path().join("no-such-dir")), None);
}

#[test]
fn the_smoke_run_prints_no_cost_or_chinese_rows_because_nothing_was_loaded() {
    // Both tables come from a candidate that opened, and none did.
    let run = execute(&smoke()).expect("finishes");
    assert!(run.cost.is_empty());
    assert!(run.chinese.is_empty());
    assert!(run.render().contains("(no rows: no candidate was loaded)"));
}
