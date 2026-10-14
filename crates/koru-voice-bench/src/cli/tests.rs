// SPDX-License-Identifier: MPL-2.0

use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn every_documented_flag_parses() {
    // The rule `koru_workspace_host`'s own `FLAGS` table follows: the parser and the
    // help are one list, and this is what fails if they drift.
    for flag in &FLAGS {
        // `--smoke` **last**, so the corpus requirement does not fire and what
        // is under test is the flag rather than the fallback rule beside it.
        // Last rather than first because `--stage` would otherwise put the
        // stage back and the requirement would fire after all.
        let mut tokens = vec![flag.name.to_owned()];
        if flag.value.is_some() {
            tokens.push(sample_value(flag.name).to_owned());
        }
        tokens.extend(args(&["--smoke"]));
        let parsed = parse(tokens);
        assert!(parsed.is_ok(), "{} does not parse: {parsed:?}", flag.name);
    }
}

/// A value each flag will accept, so the loop above is a real parse rather than
/// a shape check.
fn sample_value(name: &str) -> &'static str {
    match name {
        "--stage" => "decision",
        "--threads" => "2",
        "--candidate" => "1",
        _ => ".",
    }
}

#[test]
fn every_flag_in_the_help_is_in_the_parser_and_the_other_way() {
    let help = usage();
    for flag in &FLAGS {
        assert!(
            help.contains(flag.name),
            "{} is not in the usage",
            flag.name
        );
    }
}

#[test]
fn an_unknown_argument_is_refused_rather_than_ignored() {
    // Ignoring one sounds harmless and is the same failure as guessing a
    // directory.
    let error = parse(args(&["--corpora", "."])).expect_err("refused");
    assert!(error.message.contains("--corpora"));
}

#[test]
fn a_near_miss_gets_a_suggestion() {
    let error = parse(args(&["--corpu", "."])).expect_err("refused");
    assert_eq!(error.suggestion, Some("--corpus"));
    assert!(error.to_string().contains("did you mean --corpus"));
}

#[test]
fn something_that_is_not_near_any_flag_gets_no_suggestion() {
    // A suggestion that fires on anything is one nobody trusts.
    let error = parse(args(&["--completely-different-thing"])).expect_err("refused");
    assert_eq!(error.suggestion, None);
}

#[test]
fn a_flag_with_no_value_is_refused() {
    let error = parse(args(&["--corpus"])).expect_err("refused");
    assert!(error.message.contains("needs a value"));
}

#[test]
fn smoke_needs_no_corpus_and_every_other_stage_does() {
    // There is deliberately no default and no search path.
    let smoke = parse(args(&["--smoke"])).expect("smoke needs nothing");
    assert_eq!(smoke.stage, Stage::Smoke);
    assert!(smoke.corpus.is_none());

    // The screen stage without a corpus refuses — unless the environment
    // happens to name one, which is the documented fallback and the only one.
    if std::env::var_os("KORU_VOICE_BENCH_CORPUS").is_none() {
        let error = parse(args(&["--stage", "screen"])).expect_err("refused");
        assert!(error.message.contains("no corpus"));
        assert!(error.message.contains("no default and no search path"));
    }
}

#[test]
fn the_default_thread_count_is_the_gate_value() {
    // "RTF without a thread count is not a number", and 2 is the decision: a
    // benchmark that gave the recogniser the whole machine would measure a
    // situation that never occurs.
    assert_eq!(Options::default().num_threads, 2);
    let parsed = parse(args(&["--smoke", "--threads", "4"])).expect("parses");
    assert_eq!(parsed.num_threads, 4);
}

#[test]
fn a_thread_count_that_is_not_a_positive_number_is_refused() {
    assert!(parse(args(&["--smoke", "--threads", "0"])).is_err());
    assert!(parse(args(&["--smoke", "--threads", "-1"])).is_err());
    assert!(parse(args(&["--smoke", "--threads", "many"])).is_err());
}

#[test]
fn a_candidate_that_does_not_exist_is_refused_by_number() {
    let error = parse(args(&["--smoke", "--candidate", "9"])).expect_err("refused");
    assert!(error.message.contains("there is no candidate 9"));
    let parsed =
        parse(args(&["--smoke", "--candidate", "1", "--candidate", "3"])).expect("repeatable");
    assert_eq!(parsed.candidates, vec![1, 3]);
}

#[test]
fn stage_smoke_cannot_be_reached_through_stage() {
    // `--smoke` is its own switch, so nothing that says `--stage decision` can
    // turn into a fixture run through a typo.
    let error = parse(args(&["--stage", "smoke"])).expect_err("refused");
    assert!(error.message.contains("it is screen or decision"));
}

#[test]
fn an_unstated_class_prints_as_unstated_rather_than_as_a_plausible_name() {
    // A class is a claim about the machine and the header has to carry one.
    assert_eq!(Options::default().class, "unstated");
}

#[test]
fn the_sample_sizes_are_the_protocols() {
    use koru_voice::VoiceLanguage;
    assert_eq!(Stage::Screen.sample_size(VoiceLanguage::En), 100);
    assert_eq!(Stage::Decision.sample_size(VoiceLanguage::En), 300);
    // Simplified Chinese is the stated accuracy risk and gets double the
    // sample.
    assert_eq!(Stage::Decision.sample_size(VoiceLanguage::ZhHans), 600);
}

#[test]
fn the_environment_variables_are_named_where_a_doc_generator_could_find_them() {
    assert_eq!(
        ENV_VARS,
        ["KORU_VOICE_BENCH_CORPUS", "KORU_VOICE_BENCH_MODELS"]
    );
    for name in ENV_VARS {
        assert!(usage().contains(name), "{name} is not in the usage");
    }
}
