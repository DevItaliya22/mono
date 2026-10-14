// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::stats::Interval;

fn header() -> Header {
    Header {
        harness: "0.0.3".into(),
        protocol_rev: crate::PROTOCOL_REV,
        run_id: "2026-09-14T09:12:03Z-3f9c1a".into(),
        stage: Stage::Decision,
        host_class: "W-desk".into(),
        host: "windows / x86_64 / 16 logical cores / AVX2".into(),
        num_threads: 2,
        corpus: vec!["fleurs       rev 3f2a91c  src sha256:8c1e  conv sha256:44b0".into()],
        converter: "ffmpeg 7.1.1".into(),
        notes: Vec::new(),
    }
}

fn empty_run() -> Run {
    Run {
        header: header(),
        latency: Vec::new(),
        accuracy: Vec::new(),
        cost: Vec::new(),
        chinese: Vec::new(),
        probe: Vec::new(),
        prefix_check: Vec::new(),
        unavailable: Vec::new(),
    }
}

#[test]
fn the_header_carries_every_field_the_protocol_lists() {
    let text = header().render();
    for expected in [
        "koru-voice-bench 0.0.3",
        "protocol BENCHMARK_PROTOCOL.md rev 1",
        "run-id",
        "stage         decision",
        "host-class    W-desk",
        "num-threads   2",
        "converter     ffmpeg 7.1.1",
    ] {
        assert!(
            text.contains(expected),
            "the header is missing {expected:?}:\n{text}"
        );
    }
}

#[test]
fn a_run_with_no_notes_says_none_rather_than_printing_nothing() {
    // An absent line reads as "nothing to say"; `none` reads as what it is.
    assert!(header().render().contains("notes         none"));
}

#[test]
fn a_note_is_in_the_header_rather_than_scrolling_past() {
    let mut h = header();
    h.notes.push("smoke fixture: PLACEHOLDER".into());
    assert!(h.render().contains("PLACEHOLDER"));
}

#[test]
fn a_cell_with_too_few_samples_prints_its_n_rather_than_a_number() {
    assert_eq!(cell(Some(1.0), 29, |v| format!("{v}")), "n/a (n=29)");
    assert_eq!(cell(Some(1.0), 30, |v| format!("{v}")), "1");
    // And a number that is absent prints as an absence rather than as zero.
    assert_eq!(cell(None, 300, |v| format!("{v}")), "—");
}

#[test]
fn a_quantile_prints_with_its_interval_or_not_at_all() {
    let q = crate::stats::quantile(
        &(1..=300).map(|i| i as f64).collect::<Vec<_>>(),
        0.95,
        crate::stats::Z_90,
    );
    let (value, ci) = quantile_cell(q, 300);
    assert!(value.contains("ms"));
    assert!(ci.starts_with('[') && ci.ends_with(']'));

    let (value, ci) = quantile_cell(q, 10);
    assert_eq!(value, "n/a (n=10)");
    assert!(ci.is_empty());
}

#[test]
fn a_screen_stage_p95_prints_with_a_greater_than_or_equal() {
    // So a screen figure can never be pasted into a decision document by
    // accident.
    let figure = CensoredFigure::Estimable {
        ms: 641.0,
        ci: (608.0, 702.0),
        unbounded_above: true,
    };
    assert!(figure.render().starts_with(">="));
    assert!(!figure.is_failure());
}

#[test]
fn an_unestimable_p95_prints_the_fraction_and_is_a_failure() {
    let figure = CensoredFigure::NotEstimable {
        at_least_ms: Some(1840.0),
        fraction: 0.118,
    };
    let text = figure.render();
    assert!(text.contains(">1840"));
    assert!(text.contains("11.8%"));
    assert!(figure.is_failure());
    assert_eq!(figure.render_ci(), "not estimable");
}

#[test]
fn a_latency_row_prints_its_censored_count_without_exception() {
    let mut run = empty_run();
    run.latency.push(LatencyRow {
        cand: 1,
        lang: "en".into(),
        n: 300,
        cens: 4,
        ttfup_p50_ms: Some(312.0),
        ttfup_p95: Some(CensoredFigure::Estimable {
            ms: 641.0,
            ci: (608.0, 702.0),
            unbounded_above: false,
        }),
        fin_p50_ms: Some(188.0),
        fin_p95_ms: Some(274.0),
        revisions_per_utterance: Some((
            1.8,
            Interval {
                low: 1.6,
                high: 2.0,
            },
        )),
        cpu_percent: Some(142.0),
        overruns: 0,
    });
    let text = run.render();
    assert!(text.contains("cens"));
    assert!(text.contains("312 ms"));
    assert!(text.contains("641 ms"));
    assert!(text.contains("[608, 702]"));
}

#[test]
fn an_empty_table_says_which_table_is_empty() {
    // An empty table reads as "nothing happened".
    let text = empty_run().render();
    assert!(text.contains("Table 1"));
    assert!(text.contains("(no rows:"));
}

#[test]
fn a_candidate_that_could_not_be_built_is_listed_with_its_reason() {
    let mut run = empty_run();
    run.unavailable
        .push("4 — Whisper large-v3-turbo q5_0: model missing (/models/x)".into());
    let text = run.render();
    assert!(text.contains("candidates not run"));
    assert!(text.contains("model missing"));
}

#[test]
fn a_wer_row_and_a_cer_row_carry_their_own_metric_name() {
    // Nothing can put two of them in one aggregate, because the metric is a
    // field of the row rather than of the table.
    let mut run = empty_run();
    run.accuracy.push(AccuracyRow {
        cand: 1,
        lang: "en".into(),
        corpus: "fleurs".into(),
        n: 300,
        excl: 38,
        metric: "WER",
        value: Some(0.064),
        ci: Some((0.057, 0.072)),
        drift: 0,
    });
    run.accuracy.push(AccuracyRow {
        cand: 1,
        lang: "ja".into(),
        corpus: "fleurs".into(),
        n: 300,
        excl: 41,
        metric: "CER",
        value: Some(0.088),
        ci: Some((0.079, 0.098)),
        drift: 2,
    });
    let text = run.render();
    assert!(text.contains("6.4%"));
    assert!(text.contains("8.8%"));
    assert!(text.contains("WER"));
    assert!(text.contains("CER"));
}

#[test]
fn the_json_and_the_tables_come_from_one_structure() {
    // Which is the only reason they cannot disagree.
    let mut run = empty_run();
    run.accuracy.push(AccuracyRow {
        cand: 2,
        lang: "de".into(),
        corpus: "cv".into(),
        n: 300,
        excl: 22,
        metric: "WER",
        value: Some(0.091),
        ci: Some((0.082, 0.101)),
        drift: 0,
    });
    let json: serde_json::Value = serde_json::from_str(&run.to_json()).expect("the JSON is JSON");
    assert_eq!(json["accuracy"][0]["lang"], "de");
    assert_eq!(json["accuracy"][0]["metric"], "WER");
    assert_eq!(json["header"]["stage"], "decision");
    assert!(run.render().contains("9.1%"));
}

#[test]
fn the_json_file_name_has_no_colon_in_it() {
    // A colon is illegal in a Windows file name, and two of the four hardware
    // classes are Windows. The run id keeps its colons everywhere it is
    // *printed*, so the header still carries the stamp the protocol specifies.
    let run = empty_run();
    assert_eq!(run.json_file_name(), "bench-2026-09-14T091203Z-3f9c1a.json");
    assert!(!run.json_file_name().contains(':'));
    assert!(run.header.render().contains("2026-09-14T09:12:03Z-3f9c1a"));
}

#[test]
fn a_run_id_is_a_utc_stamp_with_a_salt() {
    let id = run_id();
    assert!(id.len() >= 27, "{id}");
    assert!(id.contains('T') && id.contains("Z-"), "{id}");
    // Two ids from one process in one second must still differ, or two runs
    // overwrite each other's JSON.
    assert_ne!(run_id(), id);
}

#[test]
fn the_civil_date_arithmetic_is_right_at_the_epoch_and_at_a_leap_day() {
    assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
    // 2024-02-29T12:34:56Z
    assert_eq!(civil_from_unix(1_709_210_096), (2024, 2, 29, 12, 34, 56));
}

#[test]
fn the_l_check_marks_a_language_outside_the_band() {
    let mut run = empty_run();
    run.prefix_check.push(PrefixCheckRow {
        lang: "en".into(),
        l_graphemes: 8,
        mean_words: Some(2.1),
        in_band: true,
    });
    run.prefix_check.push(PrefixCheckRow {
        lang: "de".into(),
        l_graphemes: 8,
        mean_words: Some(1.1),
        in_band: false,
    });
    let text = run.render();
    assert!(text.contains("ADJUST"));
    assert!(text.contains("[1.5, 3.0]"));
}

#[test]
fn the_probe_prints_the_seven_names_and_is_labelled_as_a_probe() {
    let mut run = empty_run();
    let mut hits = std::collections::BTreeMap::new();
    for (index, name) in crate::corpus::PROPER_NOUNS.iter().enumerate() {
        hits.insert((*name).to_owned(), index % 2 == 0);
    }
    run.probe.push(ProbeRow {
        cand: 1,
        lang: "en".into(),
        hits,
    });
    let text = run.render();
    assert!(text.contains("not scored, not a gate"));
    for name in crate::corpus::PROPER_NOUNS {
        assert!(text.contains(name), "{name} is not in the probe table");
    }
    assert!(text.contains("ok"));
    assert!(text.contains("--"));
}
