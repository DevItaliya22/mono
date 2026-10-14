// SPDX-License-Identifier: MPL-2.0

use super::*;

fn parse_args(args: &[&str]) -> Result<Invocation, Rejected> {
    let mut all = vec!["koru-voice-worker".to_string()];
    all.extend(args.iter().map(|arg| arg.to_string()));
    parse(all)
}

#[test]
fn no_arguments_serves_on_the_channels_own_root() {
    let invocation = parse_args(&[]).unwrap();
    assert_eq!(invocation.command, Command::Serve);
    assert!(invocation.root.is_none());
    assert!(!invocation.supervised);
    assert!(!invocation.console);
}

#[test]
fn the_three_verbs_parse() {
    assert_eq!(parse_args(&["serve"]).unwrap().command, Command::Serve);
    assert_eq!(parse_args(&["status"]).unwrap().command, Command::Status);
    assert_eq!(parse_args(&["stop"]).unwrap().command, Command::Stop);
}

#[test]
fn root_takes_a_value_either_way_round() {
    let separate = parse_args(&["--root", "C:/tmp/one"]).unwrap();
    let inline = parse_args(&["--root=C:/tmp/one"]).unwrap();
    assert_eq!(separate.root, Some(PathBuf::from("C:/tmp/one")));
    assert_eq!(inline.root, separate.root);
}

/// **A mistyped `--root` must not be ignored.** Silently serving the channel's
/// own root instead of the named one would make a suite that meant to run in
/// isolation stop the worker a person was dictating through.
#[test]
fn an_unknown_argument_is_refused_and_the_nearest_flag_is_suggested() {
    let rejected = parse_args(&["--rooot", "C:/tmp"]).unwrap_err();
    assert_eq!(rejected.arg, "--rooot");
    assert!(
        matches!(
            rejected.why,
            Why::Unknown {
                did_you_mean: Some("--root")
            }
        ),
        "{rejected:?}"
    );
    assert!(rejected.message().contains("did you mean `--root`"));
}

#[test]
fn a_flag_that_needs_a_value_and_has_none_says_so() {
    let rejected = parse_args(&["--root"]).unwrap_err();
    assert_eq!(rejected.why, Why::MissingValue);
    assert!(rejected.message().contains("needs a value"));
}

#[test]
fn an_unknown_verb_suggests_the_nearest_one() {
    let rejected = parse_args(&["statsu"]).unwrap_err();
    assert!(matches!(
        rejected.why,
        Why::Unknown {
            did_you_mean: Some("status")
        }
    ));
}

#[test]
fn two_verbs_are_refused() {
    assert!(parse_args(&["status", "stop"]).is_err());
}

/// **A bootstrap belongs to a launch, not to a question.** `--supervised` reads
/// stdin; asking for it beside `status` would make a diagnostic block on a pipe
/// nobody wrote to.
#[test]
fn supervised_is_only_valid_while_serving() {
    assert!(parse_args(&["--supervised"]).unwrap().supervised);
    assert!(parse_args(&["serve", "--supervised"]).unwrap().supervised);
    let rejected = parse_args(&["status", "--supervised"]).unwrap_err();
    assert!(matches!(rejected.why, Why::NotHere { .. }));
    assert!(rejected.message().contains("only while serving"));
}

/// `--help` and `--version` win wherever they appear, so
/// `koru-voice-worker serve --help` explains rather than starting a worker.
#[test]
fn help_and_version_win_over_a_verb() {
    assert_eq!(
        parse_args(&["serve", "--help"]).unwrap().command,
        Command::Help
    );
    assert_eq!(parse_args(&["stop", "-h"]).unwrap().command, Command::Help);
    assert_eq!(
        parse_args(&["serve", "--version"]).unwrap().command,
        Command::Version
    );
    assert_eq!(parse_args(&["-V"]).unwrap().command, Command::Version);
}

/// **The table is the only table.** A flag that parses and is not documented is
/// the same drift the table exists to prevent, in the other direction.
#[test]
fn every_documented_flag_parses_and_every_verb_is_listed() {
    let help = help();
    for flag in FLAGS {
        assert!(
            help.contains(flag.name),
            "{} is parsed and not documented",
            flag.name
        );
        let args = if flag.takes_value {
            vec![flag.name, "value"]
        } else {
            vec![flag.name]
        };
        // Every one of them either parses or is one of the two that exit early;
        // none of them is unknown.
        match parse_args(&args) {
            Ok(_) => {}
            Err(rejected) => panic!("{} is documented and not parsed: {rejected:?}", flag.name),
        }
    }
    for (verb, _) in COMMANDS {
        assert!(help.contains(verb), "{verb} is parsed and not documented");
        assert!(parse_args(&[verb]).is_ok());
    }
    // And the help names the channels, because "which build am I" is the first
    // question a wrong endpoint raises.
    assert!(help.contains(koru_channel::segment()));
}

#[test]
fn the_version_names_the_binary_rather_than_the_package() {
    assert!(version().starts_with("koru-voice-worker "));
}

/// Character distance, not byte distance: a mistyped flag in any script should
/// still find its neighbour.
#[test]
fn the_suggestion_counts_characters_rather_than_bytes() {
    assert_eq!(distance("--root", "--root"), 0);
    assert_eq!(distance("--rooot", "--root"), 1);
    assert_eq!(distance("日本", "日本語"), 1);
}
