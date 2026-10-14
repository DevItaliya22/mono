// SPDX-License-Identifier: MPL-2.0

//! What the bundled text must say. D35 forbids checking a person's copy;
//! the copy we ship is ours to pin, so an edit of the repository file that
//! drops a tool name or a folder fails here rather than in a run.
//!
//! **The first rule is pinned hardest.** A real delegated run once ended
//! with its recommendation in the transcript and nothing registered, and
//! the person saw nothing at all: a run's result is what was registered
//! from `.artifacts/<RunId>/`, so an answer that exists only in the
//! conversation is thrown away when the session ends. The tests below hold
//! that rule to the top of the text and to saying both halves — put the
//! answer in a file, and the conversation is not read — so it cannot be
//! softened or pushed down the page by a later edit without failing here.

use super::*;

/// The six tools P1.11 serves on the per-run MCP endpoint, by name.
const TOOLS: [&str; 6] = [
    "`create`",
    "`create_version`",
    "`get`",
    "`read`",
    "`search`",
    "`attach_to_run`",
];

#[test]
fn the_bundled_text_names_every_tool() {
    for tool in TOOLS {
        assert!(BUNDLED.contains(tool), "the preamble does not name {tool}");
    }
}

#[test]
fn the_bundled_text_names_both_run_folders() {
    assert!(BUNDLED.contains("`.artifacts/<RunId>/`"));
    assert!(BUNDLED.contains("`.koru/context/<RunId>/`"));
}

#[test]
fn the_bundled_text_carries_the_four_rules_the_roadmap_fixes() {
    // D29: an unregistered file is not a result.
    assert!(BUNDLED.contains("not a result until you register it"));
    assert!(BUNDLED.contains("finished needing attention"));
    // §7.3: no provider-native sub-agents.
    assert!(BUNDLED.contains("Do not start other agents, sub-agents or child sessions"));
    // Register as you go, not at the end.
    assert!(BUNDLED.contains("Register each result as soon as it is finished, not at the end"));
    // The worker appends the concrete paths; the text says so to its editor.
    assert!(BUNDLED.contains("Koru appends one line after this text"));
}

#[test]
fn the_bundled_text_is_sound_by_its_own_objective_rules() {
    assert!(!BUNDLED.trim().is_empty());
    assert!(BUNDLED.len() <= SIZE_LIMIT_BYTES);
    assert!(
        BUNDLED.len() < SIZE_LIMIT_BYTES / 8,
        "the bundled text should leave most of the limit to a person's edits"
    );
    assert!(!BUNDLED.starts_with('\u{feff}'));
}

#[test]
fn the_bundled_text_says_it_is_english_on_purpose() {
    // The comment at the top is what stops the file being filed as an i18n
    // gap; it has to survive edits of the body.
    assert!(BUNDLED.starts_with("<!--"));
    assert!(BUNDLED.contains("written in English on purpose"));
}

// -------------------------------------------- the rule that has to be first

/// The body, past the editor's comment: what the model actually meets.
fn body() -> &'static str {
    let end = BUNDLED.find("-->").expect("the editor's comment is closed") + "-->".len();
    BUNDLED[end..].trim_start()
}

#[test]
fn the_first_instruction_is_that_the_answer_goes_in_a_file_under_the_results_folder() {
    let body = body();
    let folder = body
        .find("`.artifacts/<RunId>/`")
        .expect("the body names the results folder");
    for later in [
        "## The objective",
        "## Where things are",
        "## Registering results",
    ] {
        let at = body
            .find(later)
            .unwrap_or_else(|| panic!("the body has {later}"));
        assert!(
            folder < at,
            "the results-folder rule must come before {later}"
        );
    }
    assert!(
        folder < 320,
        "the results-folder rule is {folder} bytes into the body; it belongs at the top"
    );
}

#[test]
fn the_bundled_text_says_the_conversation_is_not_the_answer() {
    // Both halves of the rule, each in words a later edit cannot weaken
    // without noticing it is doing so.
    assert!(BUNDLED.contains("Nobody reads this conversation"));
    assert!(BUNDLED.contains("thrown away when the run ends"));
    assert!(BUNDLED.contains("write it into a file under `.artifacts/<RunId>/`"));
    // Not a summary of the answer, and not a pointer to it.
    assert!(BUNDLED.contains("not a summary of it, not a pointer to where the answer is"));
}

/// It is the whole point that this is short: a person edits it, and a
/// specification is not editable in practice.
#[test]
fn the_bundled_text_stays_short_enough_to_be_read_once() {
    assert!(
        BUNDLED.lines().count() < 120,
        "{} lines; the preamble is read once before a first turn",
        BUNDLED.lines().count()
    );
}
