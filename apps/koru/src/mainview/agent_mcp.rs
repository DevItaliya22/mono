// SPDX-License-Identifier: MPL-2.0

//! *Install Koru MCP*, from the right-click menu of an agent's tab
//! ([ADR-471](../../../../docs/decisions/ADR-471-installmcp.md)).
//!
//! Claude Code, Codex, Copilot, Qwen Code and OpenCode are given Koru's bridge
//! for the one launch (ADR-469). Grok and Antigravity have no such form, only
//! a registration in their own configuration, which Koru makes **when asked**:
//! the person chose the tab's menu row, the command is the agent's own
//! `mcp add`, and nothing is written otherwise. The command runs on a worker
//! thread so the window never waits on another program, with a deadline, and
//! the answer is a toast.

use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use koru_i18n::{Key, fill, t};
use koru_session_protocol::{SessionProfileKey, built_in_profile};

use super::MainView;
use super::tab::TabView;

/// Longer than an agent's registration takes by a wide margin, short enough
/// that a hung one does not hold the menu row disabled for the session.
const DEADLINE: Duration = Duration::from_secs(60);
/// The name the bridge is registered under, the one a chat and a tab call it.
const SERVER: &str = "koru";

/// An agent that takes the bridge by registration.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Grok,
    Antigravity,
}

impl Target {
    pub(super) fn of(profile: &SessionProfileKey) -> Option<Self> {
        match profile {
            SessionProfileKey::BuiltIn(key) if key.as_str() == built_in_profile::GROK => {
                Some(Self::Grok)
            }
            SessionProfileKey::BuiltIn(key) if key.as_str() == built_in_profile::ANTIGRAVITY => {
                Some(Self::Antigravity)
            }
            _ => None,
        }
    }

    pub(super) fn program(self) -> &'static str {
        match self {
            Self::Grok => "grok",
            Self::Antigravity => "agy",
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Antigravity => "Antigravity",
        }
    }
}

/// The arguments after the program: `mcp add koru -- <bridge> serve --stdio
/// [--root <root>]`. Both agents document `--` for a command whose own
/// arguments begin with a dash, and both `add` verbs update an entry of the
/// same name, so asking twice leaves one entry.
pub(super) fn arguments(bridge: &Path, root: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "mcp".to_owned(),
        "add".to_owned(),
        SERVER.to_owned(),
        "--".to_owned(),
        bridge.display().to_string(),
        "serve".to_owned(),
        "--stdio".to_owned(),
    ];
    if let Some(root) = root {
        args.push("--root".to_owned());
        args.push(root.display().to_string());
    }
    args
}

/// What the worker found, as the words the toast says.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Installed,
    Failed(String),
}

#[derive(Default)]
pub(super) struct Install {
    running: Option<(Target, Receiver<Outcome>)>,
}

/// Run the registration to its end or its deadline.
fn run(target: Target, bridge: &Path, root: Option<&Path>) -> Outcome {
    let Some(program) = koru_session_profiles::resolve_executable(target.program()) else {
        return Outcome::Failed(format!("{} is not on PATH", target.program()));
    };
    let args = arguments(bridge, root);
    // The same boundary a tab's launch goes through, so a `.cmd` shim is run
    // the way Koru runs one and an argument it cannot trust is refused.
    let Some(argv) = koru_session_profiles::platform_launch_argv(program, &args) else {
        return Outcome::Failed("the command line cannot be passed safely".into());
    };
    let mut command = std::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    // The output is read on this worker thread; a second thread holds the
    // deadline, because `wait_with_output` blocks until the program ends.
    let id = child.id();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait_with_output());
    });
    let output = match finished.recv_timeout(DEADLINE) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Outcome::Failed(error.to_string()),
        Err(_) => {
            kill(id);
            return Outcome::Failed("it did not finish in a minute".into());
        }
    };
    if output.status.success() {
        return Outcome::Installed;
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = if said.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        said
    };
    let last = said
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it exited with an error");
    Outcome::Failed(last.chars().take(160).collect())
}

/// End a registration that outlived its deadline.
fn kill(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
}

impl MainView {
    /// The agent a tab is, when it is one that takes the bridge by
    /// registration.
    pub(super) fn agent_mcp_target(&self, tab: usize) -> Option<Target> {
        match &self.tabs().get(tab)?.view {
            TabView::Session(view) => view.profile_key.as_ref().and_then(Target::of),
            _ => None,
        }
    }

    pub(super) fn agent_mcp_running(&self) -> bool {
        self.agent_mcp.running.is_some()
    }

    /// *Install Koru MCP*: start the registration for the tab's agent.
    pub(super) fn install_agent_mcp(&mut self, tab: usize) -> bool {
        let Some(target) = self.agent_mcp_target(tab) else {
            return false;
        };
        if self.agent_mcp_running() {
            return true;
        }
        let (bridge, root) = crate::welcome::mcp::bridge();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(run(target, &bridge, root.as_deref()));
        });
        self.agent_mcp.running = Some((target, receiver));
        true
    }

    /// Collect the worker's answer and say it. Called every frame; a fast
    /// return while nothing runs.
    pub(super) fn poll_agent_mcp(&mut self) {
        let Some((target, receiver)) = &self.agent_mcp.running else {
            return;
        };
        let target = *target;
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Outcome::Failed("the worker stopped".into()),
        };
        self.agent_mcp.running = None;
        let lang = crate::format::lang();
        let message = match outcome {
            Outcome::Installed => fill(t(lang, Key::AgentMcpInstalled), &[target.name()]),
            Outcome::Failed(why) => fill(t(lang, Key::AgentMcpFailed), &[target.name(), &why]),
        };
        self.toast = Some((
            koru_ui::Toast::new(message, None, Instant::now()),
            super::ToastPayload::None,
        ));
    }
}

#[cfg(test)]
mod tests;
