// SPDX-License-Identifier: MPL-2.0

// The case-table tests are `cfg(windows)` (they compile registry chords
// into virtual-key codes); their helpers stay for them.
#![cfg_attr(not(windows), allow(dead_code))]
use super::*;

fn args(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_owned()).collect()
}

fn complete() -> Vec<String> {
    args(&[
        "--root",
        "C:/koru-roots/a11y",
        "--pid",
        "4242",
        "--window",
        "0x1234",
        "--locator",
        "koru:w*/v0/s:window#close",
    ])
}

#[test]
fn a_complete_command_line_reads_back_exactly_what_was_typed() {
    let options = parse(complete()).expect("a complete command line");
    assert_eq!(options.root, PathBuf::from("C:/koru-roots/a11y"));
    assert_eq!(options.process_id, 4242);
    assert_eq!(options.window_handle, 0x1234);
    assert_eq!(options.locator, "koru:w*/v0/s:window#close");
    assert_eq!(options.timeout, Duration::from_millis(20_000));
    assert!(!options.absent);
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn every_documented_flag_parses() {
    // The table and the parser are the same list, and this is what keeps them
    // that way: a flag added to `FLAGS` for the usage text and forgotten in the
    // match arm would reach `unreachable!` here rather than a user.
    for flag in &FLAGS {
        let mut line = if matches!(
            flag.name,
            "--ui-language" | "--locale" | "--hour-cycle" | "--time-zone"
        ) {
            complete_prepare()
        } else if matches!(
            flag.name,
            "--fixture" | "--repeat" | "--report" | "--snapshot-content"
        ) {
            complete_manifest()
        } else if matches!(
            flag.name,
            "--subject" | "--document" | "--entry" | "--reminder-date" | "--reminder-time"
        ) {
            complete_case()
        } else if matches!(flag.name, "--clip" | "--actions" | "--stills") {
            complete_sample()
        } else if flag.name == "--result" {
            complete_record()
        } else {
            complete()
        };
        if let Some(index) = line.iter().position(|token| token == flag.name) {
            line.drain(index..index + if flag.argument.is_some() { 2 } else { 1 });
        }
        line.push(flag.name.to_owned());
        if let Some(argument) = flag.argument {
            line.push(sample_for(flag.name, argument));
        }
        parse(line)
            .unwrap_or_else(|error| panic!("{} is documented and not parsed: {error}", flag.name));
    }
}

fn sample_for(flag: &str, argument: &str) -> String {
    match flag {
        "--fixture" => "fixture.json".into(),
        "--report" => "report.json".into(),
        "--repeat" => "10".into(),
        "--ui-language" => "en".into(),
        "--locale" => "en-US".into(),
        "--hour-cycle" => "h12".into(),
        "--time-zone" => "Eastern Standard Time".into(),
        "--subject" | "--entry" => "koru:w*/v*/n:00000000000000000000000000000001#text".to_owned(),
        "--document" => "koru:w*/v*/n:00000000000000000000000000000002#root".to_owned(),
        "--reminder-date" => "04/12/2099".to_owned(),
        "--reminder-time" => "3:30 PM".to_owned(),
        "--root" => "C:/koru-roots/a11y".to_owned(),
        "--pid" => "4242".to_owned(),
        "--window" => "4660".to_owned(),
        "--locator" => "koru:w1/v0/s:window#close".to_owned(),
        "--timeout" => "500".to_owned(),
        "--clip" => "clip.mp4".into(),
        "--actions" => "actions.json".into(),
        "--stills" => "stills".into(),
        "--result" => "result.json".into(),
        other => panic!("{other} takes {argument} and this table does not know a sample for it"),
    }
}

#[test]
fn the_usage_prints_every_flag_in_the_table() {
    let usage = usage();
    for flag in &FLAGS {
        assert!(
            usage.contains(flag.name),
            "{} is missing from the usage",
            flag.name
        );
    }
    assert!(usage.contains("test <document-edit|task-reminder>"));
    assert!(usage.contains("record <document-edit|task-reminder>"));
    assert!(usage.contains("sample-record document-edit"));
}

#[test]
fn an_argument_that_is_not_understood_is_not_run() {
    let mut line = complete();
    line.push("--verbose".to_owned());
    let error = parse(line).expect_err("an unknown flag");
    assert!(error.message.contains("--verbose"));
}

#[test]
fn a_near_miss_is_suggested_and_a_wild_guess_is_not() {
    let error = parse(args(&["--roo", "x"])).expect_err("a typo");
    assert_eq!(error.suggestion, Some("--root"));
    assert!(error.to_string().contains("did you mean --root?"));

    let error = parse(args(&["--wildly-unrelated-argument"])).expect_err("not a flag");
    assert_eq!(
        error.suggestion, None,
        "a suggestion that fires on anything is one nobody trusts"
    );
}

#[test]
fn each_of_the_three_that_choose_an_instance_is_required() {
    for missing in ["--root", "--pid", "--window"] {
        let line: Vec<String> = complete()
            .chunks(2)
            .filter(|pair| pair[0] != missing)
            .flat_map(<[String]>::to_vec)
            .collect();
        let error = parse(line).expect_err("an incomplete command line");
        assert!(error.message.contains(missing), "{error}");
        assert!(
            error
                .message
                .contains("could choose the session you are working in"),
            "the refusal has to say why there is no default: {error}"
        );
    }
}

#[test]
fn a_flag_whose_argument_is_missing_says_which_one() {
    let error = parse(args(&["--root"])).expect_err("a flag with no value");
    assert!(error.message.contains("--root takes <dir>"), "{error}");
}

#[test]
fn a_locator_is_checked_before_anything_is_launched() {
    // The alternative is a run that starts, attaches, waits out its whole
    // timeout and then reports "not found" for a locator that could never have
    // matched anything.
    let mut line = complete();
    line[7] = "koru:w1/v1/s:*#root".to_owned();
    let error = parse(line).expect_err("a wildcard object");
    assert!(error.message.contains("is not a locator"), "{error}");
    assert!(
        error.message.contains("never for what the control is"),
        "{error}"
    );
}

#[test]
fn a_handle_may_be_decimal_or_hex_and_may_not_be_zero() {
    let mut hex = complete();
    hex[5] = "0x1234".to_owned();
    assert_eq!(parse(hex).expect("hex").window_handle, 4660);

    let mut decimal = complete();
    decimal[5] = "4660".to_owned();
    assert_eq!(parse(decimal).expect("decimal").window_handle, 4660);

    // A null handle is what an uninitialised variable and a failed lookup both
    // produce, and `ElementFromHandle(NULL)` is the *desktop* — the one element
    // whose subtree is every application on the machine.
    let mut null = complete();
    null[5] = "0".to_owned();
    assert!(parse(null).is_err(), "a null handle must be refused");
}

#[test]
fn a_pid_of_zero_is_refused() {
    // Process 0 is the System Idle Process, and `Some(0)` is what a failed
    // `GetWindowThreadProcessId` leaves in an out parameter.
    let mut line = complete();
    line[3] = "0".to_owned();
    assert!(parse(line).is_err());
}
fn complete_case() -> Vec<String> {
    args(&[
        "test",
        "document-edit",
        "--report",
        "new-report.json",
        "--root",
        "C:/koru-roots/a11y",
        "--pid",
        "4242",
        "--window",
        "1234",
        "--subject",
        "koru:w*/v*/n:00000000000000000000000000000001#text",
        "--document",
        "koru:w*/v*/n:00000000000000000000000000000002#root",
        "--entry",
        "koru:w*/v*/n:00000000000000000000000000000003#text",
    ])
}
#[test]
fn test_mode_cannot_silently_fall_back_to_inspection() {
    let mut line = complete_case();
    line[1] = "typo".into();
    assert!(parse(line).is_err());
    let mut line = complete_case();
    line.extend(args(&["--locator", "koru:w*/v*/s:body#root"]));
    assert!(parse(line).is_err());
}
#[test]
fn model_fixture_identity_and_reopen_view_wildcard_are_required() {
    let mut line = complete_case();
    line.extend(args(&[
        "--subject",
        "koru:w*/v1/n:00000000000000000000000000000001#text",
    ]));
    assert!(parse(line).is_err());
    let mut line = complete_case();
    line.extend(args(&["--subject", "koru:w*/v*/s:body#root"]));
    assert!(parse(line).is_err());
}

fn complete_manifest() -> Vec<String> {
    args(&[
        "test",
        "document-edit",
        "--root",
        "D:/isolated/m3",
        "--pid",
        "42",
        "--window",
        "1234",
        "--fixture",
        "fixture.json",
        "--report",
        "report.json",
    ])
}
fn complete_prepare() -> Vec<String> {
    args(&[
        "prepare",
        "document-edit",
        "--root",
        "D:/isolated/m3",
        "--pid",
        "42",
        "--window",
        "1234",
        "--fixture",
        "fixture.json",
        "--snapshot-content",
        "--ui-language",
        "en",
        "--locale",
        "en-US",
        "--hour-cycle",
        "h12",
        "--time-zone",
        "Eastern Standard Time",
    ])
}
#[test]
fn prepare_requires_explicit_content_and_declared_environment() {
    let options = parse(complete_prepare()).unwrap();
    assert!(options.prepare && options.snapshot_content);
    assert_eq!(options.environment.unwrap().ui_language, "en");
    let mut line = complete_prepare();
    line.retain(|token| token != "--snapshot-content");
    assert!(parse(line).is_err());
    let mut line = complete_prepare();
    let index = line
        .iter()
        .position(|token| token == "--ui-language")
        .unwrap();
    line[index + 1] = "de".into();
    assert!(parse(line).is_err());
}
#[test]
fn repetition_is_bounded_and_cannot_override_the_manifest() {
    for count in ["0", "11", "garbage"] {
        let mut line = complete_manifest();
        line.extend(args(&["--repeat", count]));
        assert!(parse(line).is_err());
    }
    let mut line = complete_manifest();
    line.extend(args(&["--repeat", "10"]));
    assert_eq!(parse(line).unwrap().repeat, 10);
    let mut line = complete_manifest();
    line.extend(args(&[
        "--subject",
        "koru:w*/v*/n:00000000000000000000000000000001#text",
    ]));
    assert!(parse(line).is_err());
    let mut line = complete_manifest();
    line.extend(args(&["--reminder-time", "3:30 PM"]));
    assert!(parse(line).is_err());
}
// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn every_test_requires_a_report_before_runtime_can_deliver_input() {
    let mut line = complete_case();
    let index = line.iter().position(|token| token == "--report").unwrap();
    line.drain(index..index + 2);
    assert!(parse(line).unwrap_err().message.contains("--report"));
}

fn complete_sample() -> Vec<String> {
    args(&[
        "sample-record",
        "document-edit",
        "--root",
        "D:/isolated/m3",
        "--pid",
        "42",
        "--window",
        "1234",
        "--fixture",
        "fixture.json",
        "--clip",
        "clip.mp4",
        "--actions",
        "actions.json",
        "--stills",
        "stills",
    ])
}

#[test]
fn sample_record_requires_the_named_window_and_does_not_discover_one() {
    let options = parse(complete_sample()).unwrap();
    assert!(options.sample_record);
    assert_eq!(options.case.as_deref(), Some("document-edit"));
    assert_eq!(options.window_handle, 1234);
    assert!(options.clip.is_some());
    for missing in ["--root", "--pid", "--window"] {
        let mut line = complete_sample();
        if let Some(index) = line.iter().position(|token| token == missing) {
            line.drain(index..index + 2);
        }
        let error = parse(line).expect_err(missing);
        assert!(error.message.contains(missing), "{error}");
        assert!(
            error
                .message
                .contains("could choose the session you are working in"),
            "{error}"
        );
    }
}

fn complete_record() -> Vec<String> {
    let mut line = complete_sample();
    line[0] = "record".into();
    line.extend(args(&["--result", "result.json"]));
    line
}

#[test]
fn record_without_a_case_names_the_verb() {
    let error = parse(args(&["record"])).expect_err("record needs a case");
    assert!(
        error
            .message
            .contains("record <document-edit|task-reminder>"),
        "{error}"
    );
}

#[test]
fn record_accepts_both_compiled_cases_and_requires_the_named_window() {
    let options = parse(complete_record()).unwrap();
    assert!(options.record);
    assert!(!options.sample_record);
    assert_eq!(options.case.as_deref(), Some("document-edit"));
    assert!(options.result_out.is_some());
    let mut reminder = complete_record();
    reminder[1] = "task-reminder".into();
    let options = parse(reminder).unwrap();
    assert_eq!(options.case.as_deref(), Some("task-reminder"));
    for missing in ["--root", "--pid", "--window", "--result"] {
        let mut line = complete_record();
        if let Some(index) = line.iter().position(|token| token == missing) {
            line.drain(index..index + 2);
        }
        assert!(parse(line).is_err(), "{missing}");
    }
}

#[test]
fn sample_record_still_records_document_edit_only() {
    let mut line = complete_sample();
    line[1] = "task-reminder".into();
    assert!(
        parse(line)
            .unwrap_err()
            .message
            .contains("document-edit only")
    );
}

#[test]
fn record_does_not_take_a_test_report_or_repeat() {
    let mut line = complete_record();
    line.extend(args(&["--report", "report.json"]));
    assert!(parse(line).unwrap_err().message.contains("--report"));
    let mut line = complete_record();
    line.extend(args(&["--repeat", "2"]));
    assert!(parse(line).unwrap_err().message.contains("repetition"));
}
