// SPDX-License-Identifier: MPL-2.0

use super::*;

fn parse_args(args: &[&str]) -> Result<Command, Rejected> {
    let mut all = vec!["koru-relay".to_string()];
    all.extend(args.iter().map(|s| s.to_string()));
    parse(all)
}

#[test]
fn local_serves_the_developer_relay() {
    assert_eq!(
        parse_args(&["--local"]).unwrap(),
        Command::Serve(Mode::Local)
    );
}

#[test]
fn the_config_path_can_be_given_either_way() {
    for args in [
        &["--config", "/etc/koru-relay.toml"][..],
        &["--config=/etc/koru-relay.toml"][..],
    ] {
        assert_eq!(
            parse_args(args).unwrap(),
            Command::Serve(Mode::Config(PathBuf::from("/etc/koru-relay.toml")))
        );
    }
}

#[test]
fn a_path_with_spaces_and_non_latin_characters_is_kept_whole() {
    // A path is bytes the operating system gave us, not something to validate.
    let path = r"C:\Users\سامي\koru relay.toml";
    assert_eq!(
        parse_args(&["--config", path]).unwrap(),
        Command::Serve(Mode::Config(PathBuf::from(path)))
    );
}

/// There is no safe default: a bare `koru-relay` on a production host must
/// not quietly start a plain-HTTP relay, and a developer told to pass
/// `--local` learns what the flag means.
#[test]
fn no_mode_is_refused() {
    assert_eq!(parse_args(&[]).unwrap_err(), Rejected::NoMode);
}

#[test]
fn local_and_config_do_not_combine() {
    for args in [
        &["--local", "--config", "x.toml"][..],
        &["--config", "x.toml", "--local"][..],
    ] {
        assert!(matches!(
            parse_args(args).unwrap_err(),
            Rejected::Combined { .. }
        ));
    }
}

#[test]
fn a_repeated_mode_flag_is_refused() {
    let error = parse_args(&["--local", "--local"]).unwrap_err();
    assert_eq!(
        error,
        Rejected::Combined {
            first: "--local".to_string(),
            second: "--local".to_string(),
        }
    );
    assert!(error.message().contains("twice"), "{}", error.message());
}

#[test]
fn an_unknown_flag_suggests_its_neighbour() {
    let error = parse_args(&["--locl"]).unwrap_err();
    assert_eq!(
        error,
        Rejected::Unknown {
            arg: "--locl".to_string(),
            did_you_mean: Some("--local"),
        }
    );
}

#[test]
fn a_distant_unknown_gets_no_suggestion() {
    let error = parse_args(&["--frobnicate-relay"]).unwrap_err();
    assert_eq!(
        error,
        Rejected::Unknown {
            arg: "--frobnicate-relay".to_string(),
            did_you_mean: None,
        }
    );
}

/// `probe` is the one subcommand; any other bare word is as unknown as a
/// bad flag.
#[test]
fn a_bare_word_is_refused() {
    assert!(matches!(
        parse_args(&["serve"]).unwrap_err(),
        Rejected::Unknown { .. }
    ));
}

#[test]
fn a_config_without_a_path_is_refused() {
    assert_eq!(
        parse_args(&["--config"]).unwrap_err(),
        Rejected::MissingValue {
            flag: "--config".to_string(),
        }
    );
}

#[test]
fn a_value_on_a_flag_that_takes_none_is_refused() {
    assert_eq!(
        parse_args(&["--local=1"]).unwrap_err(),
        Rejected::Unknown {
            arg: "--local=1".to_string(),
            did_you_mean: Some("--local"),
        }
    );
}

#[test]
fn help_and_version_win_wherever_they_appear() {
    // `koru-relay --config typo.toml --help` must explain, not fail.
    assert_eq!(parse_args(&["--help"]).unwrap(), Command::Help);
    assert_eq!(parse_args(&["-h"]).unwrap(), Command::Help);
    assert_eq!(
        parse_args(&["--config", "x.toml", "--help"]).unwrap(),
        Command::Help
    );
    assert_eq!(parse_args(&["--version"]).unwrap(), Command::Version);
    assert_eq!(parse_args(&["-V"]).unwrap(), Command::Version);
    assert_eq!(parse_args(&["--local", "-V"]).unwrap(), Command::Version);
}

// ------------------------------------------------------------- probe

fn a_url() -> RelayUrl {
    "http://127.0.0.1:3340".parse().expect("valid relay url")
}

#[test]
fn probe_parses_a_url_with_the_library_default_timeout() {
    assert_eq!(
        parse_args(&["probe", "http://127.0.0.1:3340"]).unwrap(),
        Command::Probe {
            url: a_url(),
            timeout: DEFAULT_PROBE_TIMEOUT,
        }
    );
}

#[test]
fn the_probe_timeout_can_be_given_either_way_and_either_side_of_the_url() {
    for args in [
        &["probe", "http://127.0.0.1:3340", "--timeout", "5"][..],
        &["probe", "http://127.0.0.1:3340", "--timeout=5"][..],
        &["probe", "--timeout", "5", "http://127.0.0.1:3340"][..],
    ] {
        assert_eq!(
            parse_args(args).unwrap(),
            Command::Probe {
                url: a_url(),
                timeout: Duration::from_secs(5),
            }
        );
    }
}

/// `probe` with no URL is refused with usage — there is nothing safe to
/// probe by default.
#[test]
fn probe_without_a_url_is_refused() {
    assert_eq!(parse_args(&["probe"]).unwrap_err(), Rejected::NoUrl);
    assert_eq!(
        parse_args(&["probe", "--timeout", "5"]).unwrap_err(),
        Rejected::NoUrl
    );
}

/// A bad URL is a usage error caught before any endpoint exists — never a
/// probe failure, which the monitors would page on.
#[test]
fn a_probe_url_that_does_not_parse_is_refused() {
    // A bare IP:port is here too: `127.` cannot start a URL scheme, so it
    // fails URL parsing outright rather than the scheme check below.
    for arg in ["not a url", "127.0.0.1:3340"] {
        let error = parse_args(&["probe", arg]).unwrap_err();
        assert!(
            matches!(&error, Rejected::InvalidUrl { url, .. } if url == arg),
            "{arg}: {error:?}"
        );
    }
}

/// A bare `host:port` parses as a URL whose *scheme* is the host; refusing
/// non-http(s) schemes turns that trap into a corrected usage error.
#[test]
fn a_probe_url_without_an_http_scheme_is_refused() {
    for arg in ["localhost:3340", "ftp://relay.example"] {
        let error = parse_args(&["probe", arg]).unwrap_err();
        assert!(
            matches!(&error, Rejected::InvalidUrl { url, .. } if url == arg),
            "{arg}: {error:?}"
        );
        assert!(error.message().contains("http"), "{}", error.message());
    }
}

#[test]
fn a_second_probe_url_is_refused() {
    let error = parse_args(&["probe", "http://a.example", "http://b.example"]).unwrap_err();
    assert!(
        matches!(&error, Rejected::ExtraUrl { second, .. } if second == "http://b.example"),
        "{error:?}"
    );
}

#[test]
fn probe_timeouts_that_mean_nothing_are_refused() {
    // Zero would fail every probe before it began; the rest are not whole
    // seconds at all.
    for value in ["0", "abc", "1.5", "-3", ""] {
        assert_eq!(
            parse_args(&[
                "probe",
                "http://127.0.0.1:3340",
                &format!("--timeout={value}")
            ])
            .unwrap_err(),
            Rejected::InvalidTimeout {
                value: value.to_string(),
            },
            "--timeout={value}"
        );
    }
}

#[test]
fn a_probe_timeout_without_a_value_is_refused() {
    assert_eq!(
        parse_args(&["probe", "http://127.0.0.1:3340", "--timeout"]).unwrap_err(),
        Rejected::MissingValue {
            flag: "--timeout".to_string(),
        }
    );
}

#[test]
fn a_repeated_probe_timeout_is_refused() {
    let error = parse_args(&[
        "probe",
        "http://127.0.0.1:3340",
        "--timeout=5",
        "--timeout=6",
    ])
    .unwrap_err();
    assert_eq!(
        error,
        Rejected::Combined {
            first: "--timeout".to_string(),
            second: "--timeout".to_string(),
        }
    );
    assert!(error.message().contains("twice"), "{}", error.message());
}

#[test]
fn an_unknown_probe_flag_suggests_its_neighbour() {
    assert_eq!(
        parse_args(&["probe", "http://127.0.0.1:3340", "--timeuot=5"]).unwrap_err(),
        Rejected::Unknown {
            arg: "--timeuot".to_string(),
            did_you_mean: Some("--timeout"),
        }
    );
    // Serve flags are not probe flags.
    assert!(matches!(
        parse_args(&["probe", "http://127.0.0.1:3340", "--local"]).unwrap_err(),
        Rejected::Unknown {
            did_you_mean: None,
            ..
        }
    ));
}

#[test]
fn help_and_version_win_inside_probe_too() {
    // Even a `probe` with no URL yet must explain rather than fail.
    assert_eq!(parse_args(&["probe", "--help"]).unwrap(), Command::Help);
    assert_eq!(
        parse_args(&["probe", "http://127.0.0.1:3340", "-V"]).unwrap(),
        Command::Version
    );
}

/// `probe` anywhere but first is recognised and redirected, not treated as
/// an arbitrary unknown word.
#[test]
fn probe_after_a_mode_flag_is_refused() {
    assert_eq!(
        parse_args(&["--local", "probe"]).unwrap_err(),
        Rejected::ProbeNotFirst
    );
}

#[test]
fn a_near_miss_of_probe_is_suggested() {
    assert_eq!(
        parse_args(&["prob"]).unwrap_err(),
        Rejected::Unknown {
            arg: "prob".to_string(),
            did_you_mean: Some("probe"),
        }
    );
}

/// The drift test: the table is the only table, so everything it documents
/// must parse and everything it accepts must be documented (the short
/// spellings are listed beside the long ones in `help()`).
#[test]
fn every_documented_flag_parses() {
    for flag in FLAGS {
        let args: Vec<&str> = if flag.takes_value {
            vec![flag.name, "value"]
        } else {
            vec![flag.name]
        };
        let result = parse_args(&args);
        assert!(
            !matches!(result, Err(Rejected::Unknown { .. })),
            "documented flag {} did not parse: {result:?}",
            flag.name
        );
        assert!(
            help().contains(flag.args),
            "accepted flag {} is not documented",
            flag.name
        );
        if let Some(short) = flag.short {
            let result = parse_args(&[short]);
            assert!(
                !matches!(result, Err(Rejected::Unknown { .. })),
                "documented short {short} did not parse: {result:?}"
            );
            assert!(
                help().contains(short),
                "accepted short {short} is not documented"
            );
        }
    }
}

/// The probe half of the drift test: every documented probe flag parses in a
/// probe command line, and everything the probe parser accepts is documented.
#[test]
fn every_documented_probe_flag_parses() {
    for flag in PROBE_FLAGS {
        let args: Vec<&str> = if flag.takes_value {
            vec!["probe", "http://127.0.0.1:3340", flag.name, "1"]
        } else {
            vec!["probe", "http://127.0.0.1:3340", flag.name]
        };
        let result = parse_args(&args);
        assert!(
            !matches!(result, Err(Rejected::Unknown { .. })),
            "documented probe flag {} did not parse: {result:?}",
            flag.name
        );
        assert!(
            help().contains(flag.args),
            "accepted probe flag {} is not documented",
            flag.name
        );
    }
    // The usage line documents the subcommand itself.
    assert!(help().contains("probe <url>"), "probe is not documented");
}

/// The help prints the library's default timeout rather than retyping it, so
/// the documented number is the shipped one.
#[test]
fn the_help_shows_the_probe_default_timeout() {
    let expected = format!("default {}s", DEFAULT_PROBE_TIMEOUT.as_secs());
    assert!(help().contains(&expected), "help lost {expected:?}");
}

/// The help embeds the config example, so the schema a reader is shown is the
/// one the parser accepts — `config::tests` proves the example itself parses.
#[test]
fn the_help_shows_the_config_example() {
    let help = help();
    for line in crate::config::EXAMPLE.lines() {
        assert!(help.contains(line), "help lost the example line {line:?}");
    }
}
