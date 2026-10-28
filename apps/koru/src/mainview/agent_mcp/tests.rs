// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use super::super::context_menu::{self, MenuAction};
use super::super::harness::container;
use super::*;

fn key(name: &str) -> SessionProfileKey {
    SessionProfileKey::BuiltIn(name.parse().expect("a built-in key"))
}

#[test]
fn only_the_agents_with_no_per_launch_form_are_targets() {
    assert_eq!(Target::of(&key("grok")), Some(Target::Grok));
    assert_eq!(Target::of(&key("antigravity")), Some(Target::Antigravity));
    for other in [
        "claude", "codex", "copilot", "qwen", "opencode", "kimi", "bash",
    ] {
        assert_eq!(Target::of(&key(other)), None, "{other}");
    }
}

#[test]
fn the_command_is_the_agents_own_mcp_add_with_the_bridge_after_the_dashes() {
    let bridge = Path::new(r"C:\Program Files\Koru\koru-mcp.exe");
    assert_eq!(
        arguments(bridge, None),
        [
            "mcp",
            "add",
            "koru",
            "--",
            r"C:\Program Files\Koru\koru-mcp.exe",
            "serve",
            "--stdio"
        ]
    );
    let rooted = arguments(bridge, Some(Path::new(r"D:\dev root")));
    assert_eq!(rooted[rooted.len() - 2..], ["--root", r"D:\dev root"]);
    assert_eq!(Target::Grok.program(), "grok");
    assert_eq!(Target::Antigravity.program(), "agy");
}

#[test]
fn the_menu_row_is_offered_only_where_it_is_asked_for() {
    let with = context_menu::tab_items_for(false, false, false, false, true);
    assert!(with.contains(&MenuAction::InstallMcp));
    let at = with
        .iter()
        .position(|a| *a == MenuAction::InstallMcp)
        .unwrap();
    assert_eq!(with[at + 1], MenuAction::Divider);
    assert_eq!(
        with.last(),
        Some(&MenuAction::PopOut),
        "the window verb stays last"
    );
    assert!(!context_menu::tab_items(false, false, false, false).contains(&MenuAction::InstallMcp));
}

#[test]
fn a_document_tab_is_never_a_target_and_nothing_is_running_at_rest() {
    let (v, _workspace) = container();
    assert_eq!(v.agent_mcp_target(0), None);
    assert!(!v.agent_mcp_running());
}
