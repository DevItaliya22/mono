// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The whole of `main`, read as text: the ordering that keeps a worker
/// child from half-becoming an Agent has no type that could hold it.
const MAIN: &str = include_str!("../main.rs");

#[test]
fn the_delegate_role_is_registered_beside_the_other_two() {
    assert!(MAIN.contains("koru_agent::delegate_worker::role()"));
    assert!(MAIN.contains("koru_agent::gateway_worker::role()"));
    assert!(MAIN.contains("koru_agent::session_worker::role()"));
    assert_eq!(role().name, "delegate");
    let dispatch = MAIN.find("koru_agent_worker::dispatch(").unwrap();
    let parse = MAIN.find("cli::parse(").unwrap();
    assert!(dispatch < parse, "`dispatch` runs before the Agent's CLI");
}

#[test]
fn the_role_accepts_a_root_and_nothing_else() {
    assert_eq!(parse(Vec::<String>::new()).unwrap(), Options::default());
    assert_eq!(
        parse(["--root".to_owned(), r"D:\rehearsal".to_owned()]).unwrap(),
        Options {
            root: Some(PathBuf::from(r"D:\rehearsal")),
        }
    );
    assert_eq!(
        parse(["--supervised".to_owned()]).unwrap_err(),
        "unknown argument `--supervised`"
    );
    // A flag where the path belongs is a forgotten path.
    assert_eq!(
        parse(["--root".to_owned(), "--supervised".to_owned()]).unwrap_err(),
        "--root needs a path"
    );
    assert_eq!(
        parse(["--root".to_owned()]).unwrap_err(),
        "--root needs a path"
    );
    assert!(USAGE.starts_with("usage: koru-agent --worker delegate"));
}
