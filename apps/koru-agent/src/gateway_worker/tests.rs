// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The whole of `main`, read as text.
///
/// **The ordering this pins has no type that could hold it.** `dispatch` runs
/// first or a worker child half-becomes an Agent: it claims the single-instance
/// endpoint for the root, registers the global chord, puts an icon in the
/// notification area, and — on a machine-owning launch — re-executes itself
/// after registering the Windows notification identity. None of that is
/// reachable from a role, because the role never returns; the *only* thing
/// keeping it that way is where the call sits, so that is what is asserted.
const MAIN: &str = include_str!("../main.rs");

fn position(needle: &str) -> usize {
    MAIN.find(needle)
        .unwrap_or_else(|| panic!("apps/koru-agent/src/main.rs no longer contains `{needle}`"))
}

#[test]
fn dispatch_runs_before_every_piece_of_agent_machinery() {
    let dispatch = position("koru_agent_worker::dispatch(");
    for later in [
        // The strict Agent CLI, which knows no `--worker` token at all.
        "cli::parse(",
        // The Windows notification identity, which re-executes `current_exe()`
        // with the original arguments — a worker child reaching it would spawn
        // a second worker child.
        "windows_identity::ensure_registered(",
        // The log sink the parent also writes to.
        "koru_log::init_file",
        // The single-instance claim, the tray and the global chord all live
        // inside `run`.
        "koru_agent::run(",
    ] {
        assert!(
            dispatch < position(later),
            "`dispatch` must be the first statement of main, before `{later}`"
        );
    }
}

#[test]
fn the_gateway_role_is_registered_beside_the_sessions_role() {
    assert!(MAIN.contains("koru_agent::gateway_worker::role()"));
    assert!(MAIN.contains("koru_agent::session_worker::role()"));
    assert_eq!(role().name, "gateway");
}

#[test]
fn the_role_accepts_a_root() {
    assert_eq!(parse(Vec::<String>::new()).unwrap(), Options::default());
    assert_eq!(
        parse(["--root".to_owned(), r"D:\rehearsal".to_owned()]).unwrap(),
        Options {
            root: Some(PathBuf::from(r"D:\rehearsal")),
            deterministic_ai_test: false,
        }
    );
}

/// **The debug-only switch, in the build that has it.**
///
/// It is off unless it is asked for, it does not disturb `--root`, and it can
/// be named on either side of the path — the parser is a loop, not a grammar.
#[cfg(debug_assertions)]
#[test]
fn the_deterministic_ai_switch_parses_under_debug_assertions() {
    assert!(!parse(Vec::<String>::new()).unwrap().deterministic_ai_test);
    assert_eq!(
        parse(["--deterministic-ai-test".to_owned()]).unwrap(),
        Options {
            root: None,
            deterministic_ai_test: true,
        }
    );
    assert_eq!(
        parse([
            "--root".to_owned(),
            r"D:\rehearsal".to_owned(),
            "--deterministic-ai-test".to_owned(),
        ])
        .unwrap(),
        Options {
            root: Some(PathBuf::from(r"D:\rehearsal")),
            deterministic_ai_test: true,
        }
    );
    assert_eq!(
        parse([
            "--deterministic-ai-test".to_owned(),
            "--root".to_owned(),
            r"D:\rehearsal".to_owned(),
        ])
        .unwrap()
        .root,
        Some(PathBuf::from(r"D:\rehearsal"))
    );
    // And it is still a *value* that `--root` wants: the switch is not a path.
    assert_eq!(
        parse(["--root".to_owned(), "--deterministic-ai-test".to_owned()]).unwrap_err(),
        "--root needs a path"
    );
}

/// **A build without debug assertions does not have the switch at all**, and
/// says so in the ordinary words: it is an argument that is not understood, so
/// it is not run. This is the whole of what keeps a mock AI provider off a
/// production path.
#[cfg(not(debug_assertions))]
#[test]
fn the_deterministic_ai_switch_does_not_exist_in_a_released_build() {
    assert_eq!(
        parse(["--deterministic-ai-test".to_owned()]).unwrap_err(),
        "unknown argument `--deterministic-ai-test`"
    );
    assert_eq!(
        parse([
            "--root".to_owned(),
            r"D:\rehearsal".to_owned(),
            "--deterministic-ai-test".to_owned(),
        ])
        .unwrap_err(),
        "unknown argument `--deterministic-ai-test`"
    );
}

/// The usage line names what this build accepts and nothing else — a released
/// build that advertised a switch it refuses would be telling the reader to
/// type something it will reject.
#[test]
fn the_usage_line_names_only_what_this_build_accepts() {
    assert!(usage().contains("--root <path>"));
    assert_eq!(
        usage().contains("--deterministic-ai-test"),
        cfg!(debug_assertions)
    );
}

#[test]
fn a_non_ascii_root_is_carried_through_unchanged() {
    // A path is bytes to us and characters to the person who named the folder.
    let root = "D:/rehearsals/Κόρου-日本語-café";
    assert_eq!(
        parse(["--root".to_owned(), root.to_owned()]).unwrap().root,
        Some(PathBuf::from(root))
    );
}

#[test]
fn a_flag_where_the_path_should_be_is_a_forgotten_path() {
    // Never a directory named `--supervised`. This is the class of bug that
    // silently serves the wrong root, and the sessions role had it.
    for line in [
        vec!["--root".to_owned()],
        vec!["--root".to_owned(), "--supervised".to_owned()],
        vec!["--root".to_owned(), "-r".to_owned()],
    ] {
        assert_eq!(parse(line).unwrap_err(), "--root needs a path");
    }
}

#[test]
fn an_argument_that_is_not_understood_is_not_run() {
    assert_eq!(
        parse(["--supervised".to_owned()]).unwrap_err(),
        "unknown argument `--supervised`"
    );
    // The host's own tokens are not the role's: the supervisor names the root
    // and hands over a bootstrap, and there is nothing else to say.
    assert_eq!(
        parse(["status".to_owned()]).unwrap_err(),
        "unknown argument `status`"
    );
    assert_eq!(
        parse(["--root".to_owned(), "r".to_owned(), "--json".to_owned()]).unwrap_err(),
        "unknown argument `--json`"
    );
}
