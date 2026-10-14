// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn choice_keeps_custom_commands_and_requires_an_explicit_yes_or_assistant() {
    let custom = "my-generator {channel}";
    for answer in ["y", "Y", "yes", "YES"] {
        assert_eq!(chosen_template(answer, Some(custom)), Some(custom));
    }
    assert_eq!(chosen_template(" Codex ", Some(custom)), Some("codex"));
    assert_eq!(chosen_template(" KiMi ", Some(custom)), Some("kimi"));
    for answer in ["", "n", "no", "code", "maybe"] {
        assert_eq!(chosen_template(answer, Some(custom)), None);
    }
}

/// With no `KORU_NOTES_COMMAND` there is no default generator: `y` picks
/// nothing, and only a named assistant runs.
#[test]
fn without_a_configured_command_only_a_named_assistant_is_chosen() {
    for answer in ["y", "yes", "", "n"] {
        assert_eq!(chosen_template(answer, None), None);
    }
    assert_eq!(chosen_template("codex", None), Some("codex"));
    assert_eq!(chosen_template("kimi", None), Some("kimi"));
}

#[test]
fn custom_templates_keep_shell_syntax_and_channel_substitution() {
    let invocation = NotesCommand::new(
        r#""my tool" --channel {channel} > "notes log.txt""#,
        ReleaseChannel::Beta,
        Path::new("releases/0.1.0-beta.0.toml"),
    );
    assert_eq!(
        invocation.command,
        r#""my tool" --channel beta > "notes log.txt""#
    );
    assert!(invocation.input.is_none());
}

#[test]
fn codex_receives_the_exact_descriptor_as_input_not_shell_syntax() {
    let path = Path::new("a checkout & more/releases/0.1.0-nightly.20260913+abc1234.toml");
    let invocation = NotesCommand::new("codex", ReleaseChannel::Nightly, path);
    assert!(!invocation.command.contains("a checkout"));
    let Some(PromptInput::Stdin(prompt)) = invocation.input else {
        panic!("Codex must receive its instructions on stdin");
    };
    assert!(prompt.contains(&path.display().to_string()));
    assert!(prompt.contains("for the nightly channel"));
    assert!(prompt.contains(WRITING_INSTRUCTIONS));
    assert!(!prompt.contains(".claude/commands"));
    assert!(!prompt.contains("!`"));
    assert!(!prompt.contains("$1"));
    assert!(prompt.contains("Do not commit, tag, build, or publish"));
}

#[test]
fn kimi_receives_the_shared_instructions_as_one_literal_prompt_argument() {
    let path = Path::new("checkout & $(command) `quoted`/releases/0.1.0-beta.0.toml");
    let kimi = NotesCommand::new(" KiMi ", ReleaseChannel::Beta, path);
    let codex = NotesCommand::new("codex", ReleaseChannel::Beta, path);
    let Some(PromptInput::Stdin(expected)) = codex.input else {
        panic!("Codex prompt missing");
    };
    let invocation = kimi.invocation();
    assert_eq!(invocation.get_program(), "kimi");
    let args: Vec<_> = invocation.get_args().collect();
    assert_eq!(
        args,
        [
            std::ffi::OsStr::new("--prompt"),
            std::ffi::OsStr::new(&expected)
        ]
    );
    assert!(expected.contains(&path.display().to_string()));
    assert!(expected.contains("for the beta channel"));
    assert!(expected.contains("## Gather the release context"));
    assert!(expected.contains("## Write the descriptor"));
    assert!(!expected.contains(".claude/commands"));
    assert!(expected.contains("Do not commit, tag, build, or publish"));
}

#[test]
fn unattended_publication_refuses_generation_before_any_io() {
    let error = offer_generated_notes(
        Path::new("nonexistent-checkout"),
        ReleaseChannel::Stable,
        Path::new("releases/0.1.0.toml"),
        true,
    )
    .unwrap_err();
    assert!(error.contains("--yes cannot accept generated notes unseen"));
}

#[test]
fn runner_delivers_prompt_verbatim_and_closes_stdin() {
    // Read through EOF like `codex exec -`, without invoking an AI service or
    // writing a file. The ampersand must arrive as data, not a shell operator.
    let command = if cfg!(windows) {
        // Workspace-wide Cargo runs add native dependency directories to PATH;
        // do not depend on cmd resolving PowerShell from that environment.
        let windows = std::env::var("SystemRoot").expect("Windows system directory");
        format!(
            r#""{windows}\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -Command "if ([Console]::In.ReadToEnd() -cne 'release notes & exact path') {{ exit 7 }}""#
        )
    } else {
        r#"test "$(cat)" = 'release notes & exact path'"#.into()
    };
    NotesCommand {
        command,
        input: Some(PromptInput::Stdin("release notes & exact path".into())),
    }
    .run(Path::new(env!("CARGO_MANIFEST_DIR")))
    .unwrap();
}

#[test]
fn runner_propagates_failure_with_and_without_piped_input() {
    for input in [None, Some(PromptInput::Stdin("prompt".into()))] {
        let error = NotesCommand {
            command: "exit 7".into(),
            input,
        }
        .run(Path::new(env!("CARGO_MANIFEST_DIR")))
        .unwrap_err();
        assert!(error.contains("release-notes command failed"));
    }
}
