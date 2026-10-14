// SPDX-License-Identifier: MPL-2.0

//! Generate a missing release descriptor with a chosen assistant, then review it.

use crate::{confirm, read_answer, shell};
use koru_update::ReleaseChannel;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

// There is no default generator: with `KORU_NOTES_COMMAND` unset the publisher
// offers only the built-in assistants below, or stops so the descriptor can be
// written by hand.
//
// `exec` reads the prompt from stdin. Workspace writes create the descriptor;
// network access lets it read the published manifests named in the instructions.
const CODEX_COMMAND: &str =
    "codex exec --sandbox workspace-write -c sandbox_workspace_write.network_access=true -";
// Plain instructions. The built-in assistants receive the content directly, and
// a custom `KORU_NOTES_COMMAND` generator can read the same file.
const WRITING_INSTRUCTIONS: &str = include_str!("../../../releases/WRITING.md");

struct NotesCommand {
    command: String,
    input: Option<PromptInput>,
}

enum PromptInput {
    Stdin(String),
    Argument(String),
}

impl NotesCommand {
    fn new(template: &str, channel: ReleaseChannel, descriptor_path: &Path) -> Self {
        let assistant = template.trim().to_ascii_lowercase();
        if matches!(assistant.as_str(), "codex" | "kimi") {
            // Kimi Code 0.42.0 takes --prompt as an argument, not stdin.
            // Both transports keep checkout paths and prose out of shell syntax.
            let prompt = format!(
                "Generate release notes for the {channel} channel.\n\
                 The publisher has already selected the version. Write exactly this \
                 descriptor, even if the UTC date or published versions have changed:\n\
                 {path}\n\
                 Follow the release-writing instructions below.\n\n{WRITING_INSTRUCTIONS}",
                channel = channel.as_str(),
                path = descriptor_path.display(),
            );
            let (command, input) = if assistant == "kimi" {
                ("kimi", PromptInput::Argument(prompt))
            } else {
                (CODEX_COMMAND, PromptInput::Stdin(prompt))
            };
            Self {
                command: command.into(),
                input: Some(input),
            }
        } else {
            Self {
                command: template.replace("{channel}", channel.as_str()),
                input: None,
            }
        }
    }

    fn invocation(&self) -> Command {
        if let Some(PromptInput::Argument(prompt)) = &self.input {
            // Launch the executable directly so a prompt containing quotes,
            // newlines, or shell operators remains one literal argument.
            let mut invocation = Command::new(&self.command);
            invocation.arg("--prompt").arg(prompt);
            invocation
        } else {
            shell(&self.command)
        }
    }

    fn run(&self, root: &Path) -> Result<(), String> {
        let mut invocation = self.invocation();
        invocation.current_dir(root);
        let status = if let Some(PromptInput::Stdin(prompt)) = &self.input {
            let mut child = invocation
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|error| format!("could not start the release-notes command: {error}"))?;
            // Drop stdin to signal EOF, and reap the child even if writing fails.
            let sent = child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(prompt.as_bytes());
            let status = child.wait().map_err(|error| error.to_string())?;
            if status.success() {
                sent.map_err(|error| {
                    format!("could not send release-notes instructions: {error}")
                })?;
            }
            status
        } else {
            invocation
                .status()
                .map_err(|error| format!("could not start the release-notes command: {error}"))?
        };
        if status.success() {
            Ok(())
        } else {
            Err("the release-notes command failed; run again and choose codex or kimi, or set KORU_NOTES_COMMAND to another generator".into())
        }
    }
}

/// The generator an answer picks: `y` takes the configured command, if there
/// is one; `codex` and `kimi` are always on offer.
fn chosen_template<'a>(answer: &str, configured: Option<&'a str>) -> Option<&'a str> {
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => configured,
        "codex" => Some("codex"),
        "kimi" => Some("kimi"),
        _ => None,
    }
}

/// Generated copy is never published unseen: no assistant or `--yes`
/// can skip the review of text that becomes an immutable release page.
pub(crate) fn offer_generated_notes(
    root: &Path,
    channel: ReleaseChannel,
    descriptor_path: &Path,
    yes: bool,
) -> Result<(), String> {
    if yes {
        return Err(format!(
            "create {} before publishing; --yes cannot accept generated notes unseen",
            descriptor_path.display()
        ));
    }
    let configured = std::env::var("KORU_NOTES_COMMAND")
        .ok()
        .filter(|template| !template.trim().is_empty());
    println!("[Koru] no descriptor at {}", descriptor_path.display());
    let question = match &configured {
        Some(template) => format!(
            "Generate one with `{}`? [y/N, or codex/kimi] ",
            NotesCommand::new(template, channel, descriptor_path).command
        ),
        None => "Generate one with an assistant? [codex/kimi, or N to write it yourself] ".into(),
    };
    let answer = read_answer(&question)?;
    let template = chosen_template(&answer, configured.as_deref())
        .ok_or_else(|| format!("create {} before publishing", descriptor_path.display()))?;
    let selected = NotesCommand::new(template, channel, descriptor_path);
    println!(
        "[Koru] generating release notes with `{}`",
        selected.command
    );
    selected.run(root)?;

    // Custom generators can still land a neighboring filename. Read only the
    // exact descriptor selected by this publication, never whichever appeared.
    let notes = fs::read_to_string(descriptor_path).map_err(|_| {
        format!(
            "the release-notes command did not write {}; check releases/ for a near-miss filename",
            descriptor_path.display()
        )
    })?;
    println!("\n--- {} ---", descriptor_path.display());
    println!("{notes}");
    println!("--- end ---\n");
    if confirm("Use these release notes? [y/N] ")? {
        Ok(())
    } else {
        Err(format!(
            "release notes rejected; edit {} and run again, or delete it to regenerate",
            descriptor_path.display()
        ))
    }
}

#[cfg(test)]
mod tests;
